// SPDX-License-Identifier: GPL-2.0
/* SHDW0001 ABI 1.0: self-off and a pinned power-management worker. */
#include <linux/acpi.h>
#include <linux/atomic.h>
#include <linux/cpu.h>
#include <linux/cpuset.h>
#include <linux/io.h>
#include <linux/platform_device.h>
#include <linux/property.h>
#include <linux/sched.h>
#include <linux/sched/topology.h>
#include <linux/slab.h>
#include <linux/mutex.h>
#include <linux/iopoll.h>
#include <linux/kthread.h>
#include <linux/sched/task.h>
#include <linux/topology.h>
#include <linux/sched/isolation.h>
#include <linux/unaligned.h>
#include <linux/freezer.h>
#include <linux/interrupt.h>
#include <asm/bootinfo.h>
#include <asm/smp.h>
#include <asm/shadowcore.h>
#include <asm/topology.h>

#define SC_ABI             0x00010000
#define SC_MAGIC           0x53484352
#define SC_DISCOVERY       BIT(0)
#define SC_CPU_POWER       BIT(1)
#define SC_SHADOW          BIT(2)
#define SC_CAPABILITIES    (SC_DISCOVERY | SC_CPU_POWER | SC_SHADOW)
#define SC_SHADOW_CONTROL  0x40
#define SC_SHADOW_OWNER    0x44
#define SC_SHADOW_RESULT   0x48
#define SC_SHADOW_SWITCHES 0x4c
#define SC_RECORD_BASE     0x100
#define SC_RECORD_SIZE     0x20
#define SC_MMIO_SIZE       0x1000
#define SC_FIELDS          7
#define SC_SLOT_BASE       0x500
#define SC_SLOT_STRIDE     0x40
#define SC_COMMAND         0x00
#define SC_STATE           0x04
#define SC_RESULT          0x08
#define SC_ENTRY_LO        0x0c
#define SC_ENTRY_HI        0x10
#define SC_SEQUENCE        0x14
#define SC_COMPLETED       0x18
#define SC_OFF_COUNT       0x1c
#define SC_ON_COUNT        0x20
#define SC_CMD_CPU_OFF     1
#define SC_CMD_CPU_ON      2
#define SC_RESULT_OK       0
#define SC_RESULT_ALREADY  2
#define SC_STATE_OFF       0
#define SC_STATE_ON        1
#define SC_TIMEOUT_US      5000000

enum sc_field { SC_UID, SC_HWID, SC_CORE, SC_ROLE, SC_CAPACITY,
	       SC_CLUSTER, SC_THREAD };

struct sc_platform {
	u32 count;
	u32 *field[SC_FIELDS];
	int *cpu;
	void __iomem *regs;
	unsigned int manager;
	struct kthread_worker *worker;
	unsigned long saved_io_master[NR_WORDS];
	bool boot_online_file;
};

static atomic_t topology_changed = ATOMIC_INIT(0);
static DEFINE_MUTEX(sc_lock);
static bool installed;
static struct sc_platform *sc_live;
static int sc_manager_cpu = -1;
static cpumask_t sc_user_cpus __ro_after_init;
static bool sc_early_ready __ro_after_init;
static bool sc_shadow_active;

void __init shadowcore_early_init(void)
{
	struct acpi_table_header *table;
	const u8 *body;
	u32 uid, hwid, found_uid;
	unsigned int cpu;

	if (ACPI_FAILURE(acpi_get_table("SHCT", 0, &table)))
		return;
	body = (const u8 *)(table + 1);
	if (table->revision != 1 || table->length != sizeof(*table) + 20 ||
	    get_unaligned_le32(body) != SC_ABI ||
	    get_unaligned_le32(body + 12) != 1 || get_unaligned_le32(body + 16))
		panic("Shadowcore: invalid early service policy table");
	uid = get_unaligned_le32(body + 4);
	hwid = get_unaligned_le32(body + 8);
	for_each_possible_cpu(cpu)
		if (cpu_logical_map(cpu) == hwid &&
		    !acpi_get_cpu_uid(cpu, &found_uid) && uid == found_uid)
			break;
	if (cpu >= nr_cpu_ids || cpu == 0)
		panic("Shadowcore: management CPU must be a mapped secondary");
	sc_manager_cpu = cpu;
	cpumask_copy(&sc_user_cpus, cpu_possible_mask);
	cpumask_clear_cpu(cpu, &sc_user_cpus);
	if (cpumask_empty(&sc_user_cpus))
		panic("Shadowcore: no CPU available for user tasks");
	sc_early_ready = true;
	acpi_put_table(table);
}

const struct cpumask *shadowcore_task_cpus(struct task_struct *task)
{
	if (sc_early_ready && !(task->flags & PF_KTHREAD))
		return &sc_user_cpus;
	return cpu_possible_mask;
}

const struct cpumask *shadowcore_fallback_cpus(struct task_struct *task)
{
	if (sc_early_ready && !(task->flags & PF_KTHREAD))
		return &sc_user_cpus;
	return housekeeping_cpumask(HK_TYPE_DOMAIN);
}

int shadowcore_irq_target(void)
{
	return smp_load_acquire(&sc_shadow_active) ? READ_ONCE(sc_manager_cpu) : -1;
}

bool shadowcore_cpu_is_manager(unsigned int cpu)
{
	return READ_ONCE(sc_manager_cpu) == cpu;
}

bool shadowcore_cpu_hotplug_protected(unsigned int cpu)
{
	unsigned int other;
	bool frozen = false;

#ifdef CONFIG_FREEZER
	frozen = pm_freezing;
#endif

	if (shadowcore_cpu_is_manager(cpu))
		return true;
	if (!sc_early_ready || frozen || !cpumask_test_cpu(cpu, &sc_user_cpus))
		return false;
	/* User threads have no legal fallback on the manager. Keep a user CPU
	 * online unless the system freezer has quiesced those threads. */
	for_each_cpu_and(other, &sc_user_cpus, cpu_online_mask)
		if (other != cpu)
			return false;
	return true;
}

static int sc_cpu_index(struct sc_platform *sc, unsigned int cpu)
{
	unsigned int i;

	for (i = 0; i < sc->count; i++)
		if (sc->cpu[i] == cpu)
			return i;
	return -EINVAL;
}

static void __iomem *sc_slot(struct sc_platform *sc, unsigned int index)
{
	return sc->regs + SC_SLOT_BASE + SC_SLOT_STRIDE * index;
}

/* All manager work is bounded and must not acquire the CPU-hotplug lock. */
struct sc_manager_request {
	struct kthread_work work;
	struct sc_platform *sc;
	unsigned int index;
	unsigned long entry;
	bool start;
	int result;
};

static void sc_manager_work(struct kthread_work *work)
{
	struct sc_manager_request *request = container_of(work,
					struct sc_manager_request, work);
	struct sc_platform *sc = request->sc;
	void __iomem *slot = sc_slot(sc, request->index);
	u32 state, sequence, result;
	int ret;

	if (WARN_ON_ONCE(raw_smp_processor_id() != sc->manager)) {
		request->result = -EPERM;
		return;
	}
	if (!request->start) {
		ret = readl_poll_timeout(slot + SC_STATE, state,
			state == SC_STATE_OFF && readl(slot + SC_SEQUENCE) &&
			readl(slot + SC_COMPLETED) == readl(slot + SC_SEQUENCE),
			100, SC_TIMEOUT_US);
		request->result = ret;
		return;
	}

	state = readl(slot + SC_STATE);
	if (state == SC_STATE_ON) {
		/* Initial boot or a CPU using the original firmware wait path. */
		request->result = -EALREADY;
		return;
	}
	if (state != SC_STATE_OFF) {
		request->result = -EBUSY;
		return;
	}
	sequence = readl(slot + SC_SEQUENCE);
	if (sequence == U32_MAX) {
		request->result = -EOVERFLOW;
		return;
	}
	sequence++;
	writel(lower_32_bits(request->entry), slot + SC_ENTRY_LO);
	writel(upper_32_bits(request->entry), slot + SC_ENTRY_HI);
	writel(sequence, slot + SC_SEQUENCE);
	writel(SC_CMD_CPU_ON, slot + SC_COMMAND);
	result = readl(slot + SC_RESULT);
	if (result != SC_RESULT_OK && result != SC_RESULT_ALREADY) {
		request->result = -EIO;
		return;
	}
	ret = readl_poll_timeout(slot + SC_COMPLETED, result, result == sequence,
				100, SC_TIMEOUT_US);
	/* A late start after returning to hotplug rollback would be unsafe. */
	if (ret)
		panic("Shadowcore: CPU%d CPU_ON acknowledgement timed out",
		      sc->cpu[request->index]);
	request->result = readl(slot + SC_STATE) == SC_STATE_ON ? 0 : -EIO;
}

static int sc_manager_request(unsigned int cpu, bool start, unsigned long entry)
{
	struct sc_platform *sc = smp_load_acquire(&sc_live);
	struct sc_manager_request request;
	int index;

	if (!sc)
		return -ENODEV;
	index = sc_cpu_index(sc, cpu);
	if (index < 0 || sc->field[SC_ROLE][index])
		return -EPERM;
	request.sc = sc;
	request.index = index;
	request.entry = entry;
	request.start = start;
	request.result = -EIO;
	kthread_init_work(&request.work, sc_manager_work);
	/* Avoid self-flush if a future policy runs directly on the manager. */
	if (current == sc->worker->task) {
		sc_manager_work(&request.work);
	} else {
		kthread_queue_work(sc->worker, &request.work);
		kthread_flush_work(&request.work);
	}
	return request.result;
}

int shadowcore_cpu_on(unsigned int cpu, unsigned long entry)
{
	return sc_manager_request(cpu, true, entry);
}

int shadowcore_cpu_wait_off(unsigned int cpu)
{
	return sc_manager_request(cpu, false, 0);
}

void shadowcore_cpu_start_failed(unsigned int cpu)
{
	struct sc_platform *sc = smp_load_acquire(&sc_live);

	/* Do not reuse cpuboot_data while a late secondary might still read it. */
	if (sc && sc_cpu_index(sc, cpu) >= 0)
		panic("Shadowcore: CPU%u startup timed out; refusing unsafe rollback", cpu);
}

int shadowcore_cpu_disable(unsigned int cpu)
{
	struct sc_platform *sc = smp_load_acquire(&sc_live);
	int index;

	if (shadowcore_cpu_is_manager(cpu))
		return -EPERM;
	if (!sc)
		return 0;
	index = sc_cpu_index(sc, cpu);
	if (index < 0 || sc->field[SC_ROLE][index])
		return -EPERM;
	if (!cpu_online(sc->manager))
		return -EBUSY;
	if (readl(sc_slot(sc, index) + SC_STATE) != SC_STATE_ON)
		return -EBUSY;
	/* Reserve a sequence for both OFF and the following ON. */
	if (readl(sc_slot(sc, index) + SC_SEQUENCE) >= U32_MAX - 1)
		return -EOVERFLOW;
	return 0;
}

/* Returns only when no controller exists or preparation fails. */
int shadowcore_cpu_off(unsigned int cpu)
{
	struct sc_platform *sc = smp_load_acquire(&sc_live);
	void __iomem *slot;
	u32 sequence;
	int index;

	if (!sc)
		return -ENODEV;
	index = sc_cpu_index(sc, cpu);
	if (index < 0 || sc->field[SC_ROLE][index] ||
	    cpu != raw_smp_processor_id() || !irqs_disabled())
		return -EPERM;
	slot = sc_slot(sc, index);
	sequence = readl(slot + SC_SEQUENCE);
	if (sequence >= U32_MAX - 1)
		return -EOVERFLOW;
	/* No lock or cleanup may depend on this CPU after the command write. */
	writel(sequence + 1, slot + SC_SEQUENCE);
	wmb();
	writel(SC_CMD_CPU_OFF, slot + SC_COMMAND);
	for (;;)
		asm volatile("idle 0" : : : "memory");
}

/* Force capacity/asymmetry rescan even when the cpuset masks are unchanged. */
int arch_update_cpu_topology(void)
{
	return atomic_xchg(&topology_changed, 0);
}

static ssize_t abi_version_show(struct device *dev,
				struct device_attribute *attr, char *buf)
{
	return sysfs_emit(buf, "0x%08x\n", SC_ABI);
}
static DEVICE_ATTR_RO(abi_version);

static ssize_t capabilities_show(struct device *dev,
				 struct device_attribute *attr, char *buf)
{
	return sysfs_emit(buf, "0x%08lx\n", SC_CAPABILITIES);
}
static DEVICE_ATTR_RO(capabilities);

static ssize_t cpu_map_show(struct device *dev,
			   struct device_attribute *attr, char *buf)
{
	struct sc_platform *sc = dev_get_drvdata(dev);
	int i, len = 0;

	len += sysfs_emit_at(buf, len,
		"cpu uid hwid core cluster thread role capacity\n");
	for (i = 0; i < sc->count; i++)
		len += sysfs_emit_at(buf, len, "%d %u %u %u %u %u %u %u\n",
			sc->cpu[i], sc->field[SC_UID][i], sc->field[SC_HWID][i],
			sc->field[SC_CORE][i], sc->field[SC_CLUSTER][i],
			sc->field[SC_THREAD][i], sc->field[SC_ROLE][i],
			sc->field[SC_CAPACITY][i]);
	return len;
}
static DEVICE_ATTR_RO(cpu_map);

static ssize_t manager_cpu_show(struct device *dev,
			       struct device_attribute *attr, char *buf)
{
	struct sc_platform *sc = dev_get_drvdata(dev);

	return sysfs_emit(buf, "%u\n", sc->manager);
}
static DEVICE_ATTR_RO(manager_cpu);

static ssize_t shadow_mode_show(struct device *dev,
			       struct device_attribute *attr, char *buf)
{
	struct sc_platform *sc = dev_get_drvdata(dev);

	return sysfs_emit(buf, "enabled=%u owner_hwid=%u result=%u switches=%u\n",
		readl(sc->regs + SC_SHADOW_CONTROL),
		readl(sc->regs + SC_SHADOW_OWNER),
		readl(sc->regs + SC_SHADOW_RESULT),
		readl(sc->regs + SC_SHADOW_SWITCHES));
}
static DEVICE_ATTR_RO(shadow_mode);

static ssize_t user_cpus_show(struct device *dev,
			     struct device_attribute *attr, char *buf)
{
	return sysfs_emit(buf, "%*pbl\n", cpumask_pr_args(&sc_user_cpus));
}
static DEVICE_ATTR_RO(user_cpus);

static ssize_t power_state_show(struct device *dev,
			       struct device_attribute *attr, char *buf)
{
	struct sc_platform *sc = dev_get_drvdata(dev);
	unsigned int i;
	int len;

	len = sysfs_emit(buf, "cpu hwid state domain sequence completed off_count on_count\n");
	for (i = 0; i < sc->count; i++) {
		void __iomem *slot = sc_slot(sc, i);

		len += sysfs_emit_at(buf, len, "%d %u %u %u %u %u %u %u\n",
			sc->cpu[i], sc->field[SC_HWID][i], readl(slot + SC_STATE),
			readl(sc->regs + 0x480 + 4 * i),
			readl(slot + SC_SEQUENCE), readl(slot + SC_COMPLETED),
			readl(slot + SC_OFF_COUNT), readl(slot + SC_ON_COUNT));
	}
	return len;
}
static DEVICE_ATTR_RO(power_state);

static struct attribute *sc_attrs[] = {
	&dev_attr_abi_version.attr, &dev_attr_capabilities.attr,
	&dev_attr_cpu_map.attr, &dev_attr_manager_cpu.attr,
	&dev_attr_power_state.attr, &dev_attr_shadow_mode.attr,
	&dev_attr_user_cpus.attr, NULL,
};
static const struct attribute_group sc_group = { .attrs = sc_attrs };

static int sc_validate(struct device *dev, struct sc_platform *sc)
{
	unsigned int i, j, cpu, big = 0, little = 0;
	u32 uid;
	cpumask_t seen;

	cpumask_clear(&seen);
	for (i = 0; i < sc->count; i++) {
		unsigned int siblings = 0;
		u32 role = sc->field[SC_ROLE][i];
		u32 capacity = sc->field[SC_CAPACITY][i];

		if (role > 1 || !capacity || capacity > SCHED_CAPACITY_SCALE ||
		    (!role && capacity != SCHED_CAPACITY_SCALE) ||
		    (role && capacity == SCHED_CAPACITY_SCALE))
			return -EINVAL;
		sc->cpu[i] = -1;
		for_each_possible_cpu(cpu) {
			if (!acpi_get_cpu_uid(cpu, &uid) &&
			    uid == sc->field[SC_UID][i]) {
				sc->cpu[i] = cpu;
				break;
			}
		}
		if (sc->cpu[i] < 0 || cpumask_test_cpu(cpu, &seen) ||
		    cpu_logical_map(cpu) != sc->field[SC_HWID][i])
			return -EINVAL;
		cpumask_set_cpu(cpu, &seen);
		for (j = 0; j < sc->count; j++) {
			bool core = sc->field[SC_CORE][i] == sc->field[SC_CORE][j] &&
				sc->field[SC_CLUSTER][i] == sc->field[SC_CLUSTER][j];

			if (i != j && (sc->field[SC_UID][i] == sc->field[SC_UID][j] ||
				      sc->field[SC_HWID][i] == sc->field[SC_HWID][j]))
				return -EINVAL;
			if (!core)
				continue;
			siblings++;
			if (role != sc->field[SC_ROLE][j] ||
			    capacity != sc->field[SC_CAPACITY][j] ||
			    (i != j && sc->field[SC_THREAD][i] == sc->field[SC_THREAD][j]))
				return -EINVAL;
		}
		if (siblings != (role ? 1 : 2) ||
		    sc->field[SC_THREAD][i] >= (role ? 1 : 2))
			return -EINVAL;
		if (role)
			little++;
		else
			big++;
	}
	if (big != 16 || little != 1 || !cpumask_equal(&seen, cpu_possible_mask))
		return -EINVAL;

	/* Compare equivalence classes, not PPTT byte offsets with board IDs. */
	for (i = 0; i < sc->count; i++) {
		for (j = 0; j < sc->count; j++) {
			bool cluster = sc->field[SC_CLUSTER][i] == sc->field[SC_CLUSTER][j];
			bool core = cluster && sc->field[SC_CORE][i] == sc->field[SC_CORE][j];
			int a = sc->cpu[i], b = sc->cpu[j];

			if (topology_physical_package_id(a) != topology_physical_package_id(b) ||
			    cluster != (topology_cluster_id(a) == topology_cluster_id(b)) ||
			    core != (topology_core_id(a) == topology_core_id(b)) ||
			    cluster != (sc->field[SC_ROLE][i] == sc->field[SC_ROLE][j]))
				return dev_err_probe(dev, -EINVAL, "PPTT/role grouping mismatch\n");
		}
	}
	return 0;
}

/* The manager's protection lasts for this fixed-inventory machine's lifetime. */
static int sc_pin_manager(struct sc_platform *sc)
{
	static const struct attribute online_attr = { .name = "online" };
	static const struct attribute target_attr = { .name = "target" };
	static const struct attribute fail_attr = { .name = "fail" };
	struct device *dev = get_cpu_device(sc->manager);
	int ret = 0;

	if (!dev)
		return -EPROBE_DEFER;
	lock_device_hotplug();
	device_lock(dev);
	cpus_read_lock();
	if (!cpu_online(sc->manager)) {
		ret = -ENODEV;
	} else {
		WRITE_ONCE(sc_manager_cpu, sc->manager);
		container_of(dev, struct cpu, dev)->hotpluggable = false;
		dev_set_offline_disabled(dev);
	}
	cpus_read_unlock();
	device_unlock(dev);
	unlock_device_hotplug();
	/* Removing an attribute may wait for its callback: hold no hotplug lock. */
	if (!ret) {
		sysfs_remove_file(&dev->kobj, &online_attr);
		sysfs_remove_file_from_group(&dev->kobj, &target_attr, "hotplug");
		sysfs_remove_file_from_group(&dev->kobj, &fail_attr, "hotplug");
	}
	return ret;
}

static void sc_destroy_worker(void *data)
{
	kthread_destroy_worker(data);
}

struct sc_takeover {
	struct kthread_work work;
	struct sc_platform *sc;
	bool enable;
	int result;
};

static void sc_takeover_work(struct kthread_work *work)
{
	struct sc_takeover *request = container_of(work, struct sc_takeover, work);
	struct sc_platform *sc = request->sc;

	if (WARN_ON_ONCE(raw_smp_processor_id() != sc->manager)) {
		request->result = -EPERM;
		return;
	}
	if (request->enable) {
		/* The caller holds cpus_write_lock: no CPU teardown can race this
		 * software ownership handoff. The EIO/PCH fabric remains always on. */
		bitmap_copy(sc->saved_io_master, loongson_sysconf.cores_io_master, NR_CPUS);
		clear_bit(0, loongson_sysconf.cores_io_master);
		set_bit(sc->manager, loongson_sysconf.cores_io_master);
	}
	wmb();
	writel(request->enable, sc->regs + SC_SHADOW_CONTROL);
	if (readl(sc->regs + SC_SHADOW_RESULT) != SC_RESULT_OK ||
	    readl(sc->regs + SC_SHADOW_CONTROL) != request->enable) {
		if (request->enable)
			bitmap_copy(loongson_sysconf.cores_io_master, sc->saved_io_master, NR_CPUS);
		request->result = -EIO;
		return;
	}
	smp_store_release(&sc_shadow_active, request->enable);
	if (!request->enable)
		bitmap_copy(loongson_sysconf.cores_io_master, sc->saved_io_master, NR_CPUS);
	loongarch_eiointc_shadow_changed();
	request->result = 0;
}

static int sc_takeover(struct sc_platform *sc, bool enable)
{
	struct sc_takeover request = { .sc = sc, .enable = enable, .result = -EIO };

	cpus_write_lock();
	if (!cpu_online(sc->manager) || !cpu_online(0)) {
		cpus_write_unlock();
		return -ENODEV;
	}
	kthread_init_work(&request.work, sc_takeover_work);
	kthread_queue_work(sc->worker, &request.work);
	kthread_flush_work(&request.work);
	if (!request.result)
		smp_store_release(&sc_live, enable ? sc : NULL);
	cpus_write_unlock();
	return request.result;
}

/* CPU0 originally has no online control because it was the boot IO master. */
static ssize_t online_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	return sysfs_emit(buf, "%u\n", cpu_online(dev->id));
}

static ssize_t online_store(struct device *dev, struct device_attribute *attr,
			    const char *buf, size_t count)
{
	bool online;
	int ret = kstrtobool(buf, &online);

	if (ret)
		return ret;
	ret = online ? add_cpu(dev->id) : remove_cpu(dev->id);
	return ret ? ret : count;
}
static DEVICE_ATTR_RW(online);

static int sc_boot_cpu_hotplug(struct sc_platform *sc, bool enable)
{
	struct device *dev = get_cpu_device(0);
	int ret = 0;

	if (!dev)
		return -ENODEV;
	lock_device_hotplug();
	device_lock(dev);
	container_of(dev, struct cpu, dev)->hotpluggable = enable;
	dev_assign_offline_disabled(dev, !enable);
	if (enable) {
		ret = device_create_file(dev, &dev_attr_online);
		if (!ret)
			sc->boot_online_file = true;
		else {
			container_of(dev, struct cpu, dev)->hotpluggable = false;
			dev_set_offline_disabled(dev);
		}
	}
	device_unlock(dev);
	unlock_device_hotplug();
	if (!enable && sc->boot_online_file) {
		device_remove_file(dev, &dev_attr_online);
		sc->boot_online_file = false;
	}
	return ret;
}

static int sc_probe(struct platform_device *pdev)
{
	static const char * const names[SC_FIELDS] = {
		"shadowcore,cpu-uids", "shadowcore,hw-ids", "shadowcore,core-ids",
		"shadowcore,roles", "shadowcore,capacities", "shadowcore,cluster-ids",
		"shadowcore,thread-ids",
	};
	struct device *dev = &pdev->dev;
	struct sc_platform *sc;
	struct resource *res;
	void __iomem *regs;
	u32 abi, caps, manager_uid;
	int i, j, ret;

	guard(mutex)(&sc_lock);
	if (installed)
		return -EBUSY;
	if (!pptt_enabled)
		return dev_err_probe(dev, -EINVAL, "valid PPTT is required\n");
	sc = devm_kzalloc(dev, sizeof(*sc), GFP_KERNEL);
	if (!sc)
		return -ENOMEM;
	if (device_property_read_u32(dev, "shadowcore,abi-version", &abi) ||
	    device_property_read_u32(dev, "shadowcore,capabilities", &caps) ||
	    device_property_read_u32(dev, "shadowcore,cpu-count", &sc->count) ||
	    device_property_read_u32(dev, "shadowcore,manager-uid", &manager_uid) ||
	    abi != SC_ABI || caps != SC_CAPABILITIES || sc->count != 17 ||
	    sc->count != num_possible_cpus())
		return dev_err_probe(dev, -EINVAL, "unsupported or incomplete discovery ABI\n");
	sc->cpu = devm_kcalloc(dev, sc->count, sizeof(*sc->cpu), GFP_KERNEL);
	if (!sc->cpu)
		return -ENOMEM;
	for (i = 0; i < SC_FIELDS; i++) {
		if (device_property_count_u32(dev, names[i]) != sc->count)
			return dev_err_probe(dev, -EINVAL, "invalid %s length\n", names[i]);
		sc->field[i] = devm_kcalloc(dev, sc->count, sizeof(u32), GFP_KERNEL);
		if (!sc->field[i])
			return -ENOMEM;
		ret = device_property_read_u32_array(dev, names[i], sc->field[i], sc->count);
		if (ret)
			return ret;
	}
	regs = devm_platform_get_and_ioremap_resource(pdev, 0, &res);
	if (IS_ERR(regs))
		return PTR_ERR(regs);
	sc->regs = regs;
	if (resource_size(res) != SC_MMIO_SIZE ||
	    readl(regs) != SC_MAGIC || readl(regs + 4) != abi ||
	    readl(regs + 8) != caps || readl(regs + 12) != sc->count ||
	    readl(regs + 16) != SC_RECORD_BASE || readl(regs + 20) != SC_RECORD_SIZE)
		return dev_err_probe(dev, -EINVAL, "MMIO/ACPI header or resources mismatch\n");
	for (i = 0; i < sc->count; i++) {
		void __iomem *record = regs + SC_RECORD_BASE + i * SC_RECORD_SIZE;

		for (j = 0; j < SC_FIELDS; j++)
			if (readl(record + j * sizeof(u32)) != sc->field[j][i])
				return dev_err_probe(dev, -EINVAL, "MMIO/ACPI CPU record mismatch\n");
		if (readl(record + 28))
			return -EINVAL;
	}
	ret = sc_validate(dev, sc);
	if (ret)
		return dev_err_probe(dev, ret, "CPU identity or topology validation failed\n");
	for (i = 0; i < sc->count; i++)
		if (sc->field[SC_UID][i] == manager_uid)
			break;
	if (i == sc->count || !sc->field[SC_ROLE][i] ||
	    readl(regs + 0x18) != sc->field[SC_HWID][i] ||
	    readl(regs + 0x1c) != SC_SLOT_STRIDE)
		return dev_err_probe(dev, -EINVAL, "invalid management CPU mapping\n");
	sc->manager = sc->cpu[i];
	if (!sc_early_ready || sc->manager != sc_manager_cpu)
		return dev_err_probe(dev, -EINVAL, "early management policy mismatch\n");
	ret = sc_pin_manager(sc);
	if (ret)
		return dev_err_probe(dev, ret, "management CPU must be online\n");
	sc->worker = kthread_create_worker_on_cpu(sc->manager, 0, "shadowcore/%u");
	if (IS_ERR(sc->worker))
		return PTR_ERR(sc->worker);
	wake_up_process(sc->worker->task);
	ret = devm_add_action_or_reset(dev, sc_destroy_worker, sc->worker);
	if (ret)
		return ret;
	platform_set_drvdata(pdev, sc);
	ret = devm_device_add_group(dev, &sc_group);
	if (ret)
		return ret;
	if (!io_master(0) || bitmap_weight(loongson_sysconf.cores_io_master, NR_CPUS) != 1)
		return dev_err_probe(dev, -EINVAL, "unsupported initial IO-master layout\n");
	ret = sc_takeover(sc, true);
	if (ret)
		return dev_err_probe(dev, ret, "management CPU Shadow takeover failed\n");
	ret = sc_boot_cpu_hotplug(sc, true);
	if (ret) {
		sc_takeover(sc, false);
		return ret;
	}

	/* All checks completed before publishing any scheduler input. */
	cpus_read_lock();
	for (i = 0; i < sc->count; i++)
		topology_set_cpu_scale(sc->cpu[i], sc->field[SC_CAPACITY][i]);
	smp_store_release(&sc_live, sc);
	cpus_read_unlock();
	installed = true;
	atomic_set(&topology_changed, 1);
	rebuild_sched_domains();
	dev_info(dev, "ABI 1.0: %u CPUs; CPU%u owns IO master/Shadow/power; user tasks excluded\n",
		 sc->count, sc->manager);
	return 0;
}

static void sc_remove(struct platform_device *pdev)
{
	struct sc_platform *sc = platform_get_drvdata(pdev);
	int i;

	guard(mutex)(&sc_lock);
	/* Restore boot service before allowing the fixed platform to unbind. */
	for (i = 0; i < sc->count; i++)
		if (!cpu_online(sc->cpu[i]) && add_cpu(sc->cpu[i]))
			panic("Shadowcore: cannot restore CPU%d during removal", sc->cpu[i]);
	sc_boot_cpu_hotplug(sc, false);
	if (sc_takeover(sc, false))
		panic("Shadowcore: cannot relinquish Shadow routing during removal");
	cpus_write_lock();
	smp_store_release(&sc_live, NULL);
	for (i = 0; i < sc->count; i++)
		topology_set_cpu_scale(sc->cpu[i], SCHED_CAPACITY_SCALE);
	cpus_write_unlock();
	atomic_set(&topology_changed, 1);
	rebuild_sched_domains();
	installed = false;
}

static const struct acpi_device_id sc_acpi_ids[] = {
	{ "SHDW0001", 0 }, { }
};

static struct platform_driver sc_driver = {
	.probe = sc_probe,
	.remove = sc_remove,
	.driver = {
		.name = "shadowcore",
		.acpi_match_table = sc_acpi_ids,
		.suppress_bind_attrs = true,
	},
};
builtin_platform_driver(sc_driver);
