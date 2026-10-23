// SPDX-License-Identifier: GPL-2.0
/* SHDW0001 ABI 1.0: immutable discovery, not a power-control driver. */
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
#include <linux/topology.h>
#include <asm/smp.h>
#include <asm/topology.h>

#define SC_ABI             0x00010000
#define SC_MAGIC           0x53484352
#define SC_DISCOVERY       BIT(0)
#define SC_RECORD_BASE     0x100
#define SC_RECORD_SIZE     0x20
#define SC_MMIO_SIZE       0x1000
#define SC_FIELDS          7

enum sc_field { SC_UID, SC_HWID, SC_CORE, SC_ROLE, SC_CAPACITY,
	       SC_CLUSTER, SC_THREAD };

struct sc_platform {
	u32 count;
	u32 *field[SC_FIELDS];
	int *cpu;
};

static atomic_t topology_changed = ATOMIC_INIT(0);
static DEFINE_MUTEX(sc_lock);
static bool installed;

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
	return sysfs_emit(buf, "0x%08lx\n", SC_DISCOVERY);
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

static struct attribute *sc_attrs[] = {
	&dev_attr_abi_version.attr, &dev_attr_capabilities.attr,
	&dev_attr_cpu_map.attr, NULL,
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
	u32 abi, caps;
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
	    abi != SC_ABI || caps != SC_DISCOVERY || sc->count != 17 ||
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
	platform_set_drvdata(pdev, sc);
	ret = devm_device_add_group(dev, &sc_group);
	if (ret)
		return ret;

	/* All checks completed before publishing any scheduler input. */
	cpus_read_lock();
	for (i = 0; i < sc->count; i++)
		topology_set_cpu_scale(sc->cpu[i], sc->field[SC_CAPACITY][i]);
	cpus_read_unlock();
	installed = true;
	atomic_set(&topology_changed, 1);
	rebuild_sched_domains();
	dev_info(dev, "ABI 1.0: validated %u CPUs; fixed capacities installed; discovery only\n",
		 sc->count);
	return 0;
}

static void sc_remove(struct platform_device *pdev)
{
	struct sc_platform *sc = platform_get_drvdata(pdev);
	int i;

	guard(mutex)(&sc_lock);
	cpus_read_lock();
	for (i = 0; i < sc->count; i++)
		topology_set_cpu_scale(sc->cpu[i], SCHED_CAPACITY_SCALE);
	cpus_read_unlock();
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
