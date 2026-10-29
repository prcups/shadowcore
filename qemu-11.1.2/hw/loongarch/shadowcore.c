/* SPDX-License-Identifier: GPL-2.0-or-later */
/* ABI 1.0: compact CPU slots, shared secondary entry and reset trampoline. */
#include "qemu/osdep.h"
#include "qapi/error.h"
#include "qemu/log.h"
#include "qemu/main-loop.h"
#include "hw/core/cpu.h"
#include "hw/core/hw-error.h"
#include "hw/core/sysbus.h"
#include "hw/loongarch/shadowcore.h"
#include "hw/intc/loongarch_extioi.h"
#include "hw/intc/loongarch_ipi.h"
#include "system/address-spaces.h"
#include "qemu/bswap.h"
#include "migration/vmstate.h"
#include "system/reset.h"
#include "target/loongarch/cpu.h"
#include "exec/cputlb.h"

#define TYPE_SHADOWCORE "loongarch-shadowcore"
#define SHADOWCORE_CPU_FIELDS 7
OBJECT_DECLARE_SIMPLE_TYPE(ShadowcoreState, SHADOWCORE)

typedef struct ShadowcoreSlot {
    CPUState *cpu;
    CPUSysState reset_sys;
    uint64_t accepted_entry;
    uint32_t state, result, sequence, accepted, completed;
} ShadowcoreSlot;

struct ShadowcoreState {
    SysBusDevice parent_obj;
    MemoryRegion mmio;
    MemoryRegion boot_window;
    uint32_t boot_control, boot_result, boot_generation;
    uint64_t boot_entry;
    uint32_t records[SHADOWCORE_CPUS][SHADOWCORE_CPU_FIELDS];
    ShadowcoreSlot slots[SHADOWCORE_CPUS];
    int manager;
    LoongArchVirtMachineState *machine;
    bool shadow_enabled;
    uint32_t shadow_result;
    uint64_t generation;
};

typedef struct ShadowcoreWork {
    ShadowcoreState *controller;
    uint64_t generation;
    unsigned index;
    uint32_t command, sequence;
} ShadowcoreWork;

static void shadowcore_boot_control(ShadowcoreState *s, uint32_t val)
{
    unsigned i;

    if (current_cpu != s->slots[s->manager].cpu ||
        s->slots[s->manager].state != SHADOWCORE_STATE_ON ||
        (val != 0 && val != SHADOWCORE_BOOT_ENABLE &&
         val != SHADOWCORE_BOOT_READY)) {
        s->boot_result = SHADOWCORE_RESULT_INVALID;
        return;
    }
    if (val == s->boot_control) {
        s->boot_result = SHADOWCORE_RESULT_OK;
        return;
    }
    /* Seal once per machine-reset epoch. No live code replacement. */
    if (s->boot_control == SHADOWCORE_BOOT_READY ||
        (val == SHADOWCORE_BOOT_READY &&
         s->boot_control != SHADOWCORE_BOOT_ENABLE)) {
        s->boot_result = SHADOWCORE_RESULT_INVALID;
        return;
    }
    for (i = 0; i < SHADOWCORE_CPUS; i++) {
        if (s->slots[i].state != SHADOWCORE_STATE_ON) {
            s->boot_result = SHADOWCORE_RESULT_BUSY;
            return;
        }
    }
    if (val == SHADOWCORE_BOOT_READY) {
        memory_region_flush_rom_device(&s->boot_window, 0,
                                       SHADOWCORE_BOOT_WINDOW_SIZE);
        s->boot_generation++;
    }
    s->boot_control = val;
    memory_region_set_enabled(&s->boot_window, val != 0);
    s->boot_result = SHADOWCORE_RESULT_OK;
}

static void shadowcore_boot_write(void *opaque, hwaddr addr,
                                 uint64_t val, unsigned size)
{
    ShadowcoreState *s = opaque;
    uint8_t *ram = memory_region_get_ram_ptr(&s->boot_window);

    assert(bql_locked());
    if (current_cpu != s->slots[s->manager].cpu ||
        s->boot_control != SHADOWCORE_BOOT_ENABLE) {
        s->boot_result = SHADOWCORE_RESULT_INVALID;
        return;
    }
    stn_le_p(ram + addr, size, val);
    memory_region_flush_rom_device(&s->boot_window, addr, size);
    s->boot_result = SHADOWCORE_RESULT_OK;
}

static const MemoryRegionOps shadowcore_boot_ops = {
    .write = shadowcore_boot_write,
    .endianness = DEVICE_LITTLE_ENDIAN,
    .valid = { .min_access_size = 1, .max_access_size = 8, .unaligned = false },
    .impl = { .min_access_size = 1, .max_access_size = 8 },
};

static void shadowcore_power_work(CPUState *cs, run_on_cpu_data data)
{
    g_autofree ShadowcoreWork *work = data.host_ptr;
    ShadowcoreState *s = work->controller;
    ShadowcoreSlot *slot = &s->slots[work->index];
    LoongArchCPU *cpu = LOONGARCH_CPU(cs);
    CPULoongArchState *env = &cpu->env;

    assert(bql_locked());
    if (work->generation != s->generation ||
        work->sequence != slot->accepted) {
        return;
    }
    if (work->command == SHADOWCORE_CMD_CPU_OFF) {
        timer_del(&cpu->timer);
        env_sys(env)->CSR_TCFG = 0;
        qatomic_set(&cpu->shadowcore_powered_off, true);
        cs->halted = 1;
        cs->exception_index = EXCP_HLT;
        slot->state = SHADOWCORE_STATE_OFF;
    } else {
        /* Discard context; model capabilities and IOCSR wiring survive. */
        *env_sys(env) = slot->reset_sys;
        memset(env->gpr, 0, sizeof(env->gpr));
        memset(env->fpr, 0, sizeof(env->fpr));
        memset(env->cf, 0, sizeof(env->cf));
        memset(&env->lbt, 0, sizeof(env->lbt));
#ifdef CONFIG_TCG
        env->lladdr = 1;
        env->llval = env->llval_high = env->llbit_scq = 0;
#endif
        cpu_reset(cs);
        tlb_flush(cs);
        loongarch_ipi_reset_cpu(s->machine->ipi, cs);
        cpu_reset_interrupt(cs, CPU_INTERRUPT_HARD);
        assert(CPU_GET_CLASS(cs)->get_pc(cs) == SHADOWCORE_BOOT_VECTOR);
        cs->halted = 0;
        /* Remain STARTING until actual guest instructions in the reset
         * window report BOOT_ENTER. Reset is not Linux-online completion. */
        return;
    }
    slot->result = SHADOWCORE_RESULT_OK;
    slot->completed = work->sequence;
}

static bool shadowcore_slot_pending(ShadowcoreSlot *slot)
{
    return slot->state == SHADOWCORE_STATE_STARTING ||
           slot->state == SHADOWCORE_STATE_STOPPING;
}

static void shadowcore_command(ShadowcoreState *s, unsigned index, uint32_t cmd)
{
    ShadowcoreSlot *slot = &s->slots[index];
    ShadowcoreWork *work;
    unsigned pa_bits;

    assert(bql_locked());
    if (s->records[index][3] ||
        (cmd != SHADOWCORE_CMD_CPU_ON && cmd != SHADOWCORE_CMD_CPU_OFF) ||
        (cmd == SHADOWCORE_CMD_CPU_OFF && current_cpu != slot->cpu) ||
        (cmd == SHADOWCORE_CMD_CPU_ON &&
         current_cpu != s->slots[s->manager].cpu)) {
        slot->result = SHADOWCORE_RESULT_INVALID;
        return;
    }
    if (shadowcore_slot_pending(slot)) {
        slot->result = SHADOWCORE_RESULT_BUSY;
        return;
    }
    if (!slot->sequence || slot->sequence <= slot->accepted ||
        (cmd == SHADOWCORE_CMD_CPU_OFF && slot->sequence == UINT32_MAX)) {
        slot->result = SHADOWCORE_RESULT_STALE;
        return;
    }
    if (cmd == SHADOWCORE_CMD_CPU_ON &&
        slot->state == SHADOWCORE_STATE_ON) {
        slot->accepted = slot->completed = slot->sequence;
        slot->result = SHADOWCORE_RESULT_ALREADY;
        return;
    }
    if (s->boot_control != SHADOWCORE_BOOT_READY) {
        slot->result = SHADOWCORE_RESULT_INVALID;
        return;
    }
    pa_bits = FIELD_EX32(LOONGARCH_CPU(slot->cpu)->env.cpucfg[1],
                         CPUCFG1, PALEN) + 1;
    if ((cmd == SHADOWCORE_CMD_CPU_OFF &&
         slot->state != SHADOWCORE_STATE_ON) ||
        (cmd == SHADOWCORE_CMD_CPU_ON &&
         (!s->boot_entry || (s->boot_entry & 3) ||
          (pa_bits < 64 && (s->boot_entry >> pa_bits))))) {
        slot->result = SHADOWCORE_RESULT_INVALID;
        return;
    }

    work = g_new0(ShadowcoreWork, 1);
    work->controller = s;
    work->generation = s->generation;
    work->index = index;
    work->command = cmd;
    work->sequence = slot->sequence;
    if (cmd == SHADOWCORE_CMD_CPU_ON) {
        slot->accepted_entry = s->boot_entry;
    }
    slot->accepted = slot->sequence;
    slot->result = SHADOWCORE_RESULT_OK;
    slot->state = cmd == SHADOWCORE_CMD_CPU_OFF ?
                  SHADOWCORE_STATE_STOPPING : SHADOWCORE_STATE_STARTING;
    /* Match ARM PSCI: stop/start outside translated guest execution. */
    async_run_on_cpu(slot->cpu, shadowcore_power_work, RUN_ON_CPU_HOST_PTR(work));
}

static uint64_t shadowcore_read(void *opaque, hwaddr addr, unsigned size)
{
    ShadowcoreState *s = opaque;
    unsigned index, reg;
    ShadowcoreSlot *slot;

    assert(bql_locked());
    switch (addr) {
    case 0x00: return SHADOWCORE_MAGIC;
    case 0x04: return SHADOWCORE_ABI_VERSION;
    case 0x08: return SHADOWCORE_CAPABILITIES;
    case 0x0c: return SHADOWCORE_CPUS;
    case 0x10: return SHADOWCORE_SLOT_BASE;
    case 0x14: return SHADOWCORE_SLOT_STRIDE;
    case SHADOWCORE_MANAGER_HARDWARE_ID:
        return s->records[s->manager][1];
    case SHADOWCORE_MANAGER_SLOT: return s->manager;
    case SHADOWCORE_BOOT_ENTRY_LO: return (uint32_t)s->boot_entry;
    case SHADOWCORE_BOOT_ENTRY_HI: return s->boot_entry >> 32;
    case SHADOWCORE_SHADOW_CONTROL: return s->shadow_enabled;
    case SHADOWCORE_SHADOW_RESULT: return s->shadow_result;
    case SHADOWCORE_BOOT_CONTROL: return s->boot_control;
    case SHADOWCORE_BOOT_RESULT: return s->boot_result;
    case SHADOWCORE_BOOT_SIZE: return SHADOWCORE_BOOT_WINDOW_SIZE;
    case SHADOWCORE_BOOT_GENERATION: return s->boot_generation;
    }
    if (addr < SHADOWCORE_SLOT_BASE ||
        addr >= SHADOWCORE_SLOT_BASE + SHADOWCORE_SLOT_STRIDE * SHADOWCORE_CPUS) {
        return 0;
    }
    index = (addr - SHADOWCORE_SLOT_BASE) / SHADOWCORE_SLOT_STRIDE;
    reg = (addr - SHADOWCORE_SLOT_BASE) % SHADOWCORE_SLOT_STRIDE;
    slot = &s->slots[index];
    switch (reg) {
    case SHADOWCORE_SLOT_STATE: return slot->state;
    case SHADOWCORE_SLOT_RESULT: return slot->result;
    case SHADOWCORE_SLOT_HARDWARE_ID: return s->records[index][1];
    case SHADOWCORE_SLOT_SEQUENCE: return slot->sequence;
    case SHADOWCORE_SLOT_COMPLETED: return slot->completed;
    default: return 0;
    }
}

static void shadowcore_write(void *opaque, hwaddr addr, uint64_t val, unsigned size)
{
    ShadowcoreState *s = opaque;
    ShadowcoreSlot *slot;
    unsigned index, reg;

    assert(bql_locked());
    if (addr == SHADOWCORE_BOOT_ENTRY_LO ||
        addr == SHADOWCORE_BOOT_ENTRY_HI) {
        unsigned i;

        if (current_cpu != s->slots[s->manager].cpu) {
            goto invalid;
        }
        for (i = 0; i < SHADOWCORE_CPUS; i++) {
            if (shadowcore_slot_pending(&s->slots[i])) {
                goto invalid;
            }
        }
        if (addr == SHADOWCORE_BOOT_ENTRY_LO) {
            s->boot_entry = deposit64(s->boot_entry, 0, 32, val);
        } else {
            s->boot_entry = deposit64(s->boot_entry, 32, 32, val);
        }
        return;
    }
    if (addr == SHADOWCORE_BOOT_CONTROL) {
        shadowcore_boot_control(s, val);
        return;
    }
    if (addr >= SHADOWCORE_BOOT_RESULT && addr <= SHADOWCORE_BOOT_GENERATION) {
        s->boot_result = SHADOWCORE_RESULT_INVALID;
        return;
    }
    if (addr == SHADOWCORE_SHADOW_CONTROL) {
        if (val > 1 || current_cpu != s->slots[s->manager].cpu ||
            s->slots[s->manager].state != SHADOWCORE_STATE_ON) {
            s->shadow_result = SHADOWCORE_RESULT_INVALID;
            return;
        }
        if (!loongarch_extioi_set_shadow(s->machine->extioi,
                                         s->records[s->manager][1], val)) {
            s->shadow_result = SHADOWCORE_RESULT_INVALID;
            return;
        }
        s->shadow_enabled = val;
        s->shadow_result = SHADOWCORE_RESULT_OK;
        return;
    }
    if (addr < SHADOWCORE_SLOT_BASE ||
        addr >= SHADOWCORE_SLOT_BASE + SHADOWCORE_SLOT_STRIDE * SHADOWCORE_CPUS) {
        goto invalid;
    }
    index = (addr - SHADOWCORE_SLOT_BASE) / SHADOWCORE_SLOT_STRIDE;
    reg = (addr - SHADOWCORE_SLOT_BASE) % SHADOWCORE_SLOT_STRIDE;
    slot = &s->slots[index];
    if (reg == SHADOWCORE_SLOT_BOOT_ENTER) {
        uint64_t pc = current_cpu ? CPU_GET_CLASS(current_cpu)->get_pc(current_cpu) : 0;

        if (s->records[index][3] || current_cpu != slot->cpu || val != 1 ||
            slot->state != SHADOWCORE_STATE_STARTING ||
            s->boot_control != SHADOWCORE_BOOT_READY ||
            slot->accepted_entry != s->boot_entry ||
            pc < SHADOWCORE_BOOT_VECTOR ||
            pc >= SHADOWCORE_BOOT_VECTOR + SHADOWCORE_BOOT_WINDOW_SIZE) {
            slot->result = SHADOWCORE_RESULT_INVALID;
            return;
        }
        slot->completed = slot->accepted;
        slot->result = SHADOWCORE_RESULT_OK;
        slot->state = SHADOWCORE_STATE_ON;
        return;
    }
    if (s->records[index][3] ||
        (current_cpu != slot->cpu &&
         current_cpu != s->slots[s->manager].cpu)) {
        goto invalid;
    }
    switch (reg) {
    case SHADOWCORE_SLOT_COMMAND:
        shadowcore_command(s, index, val);
        return;
    }
    if (shadowcore_slot_pending(slot)) {
        goto invalid;
    }
    switch (reg) {
    case SHADOWCORE_SLOT_SEQUENCE:
        slot->sequence = val;
        return;
    }
invalid:
    qemu_log_mask(LOG_GUEST_ERROR,
                  "shadowcore: denied MMIO write at 0x%" HWADDR_PRIx "\n", addr);
}

static const MemoryRegionOps shadowcore_ops = {
    .read = shadowcore_read,
    .write = shadowcore_write,
    .endianness = DEVICE_LITTLE_ENDIAN,
    .valid = { .min_access_size = 4, .max_access_size = 4, .unaligned = false },
    .impl = { .min_access_size = 4, .max_access_size = 4 },
};

static const VMStateDescription shadowcore_vmstate = {
    .name = TYPE_SHADOWCORE,
    .unmigratable = 1,
};

static void shadowcore_reset(void *opaque)
{
    ShadowcoreState *s = opaque;
    unsigned i;

    s->generation++;
    memory_region_set_enabled(&s->boot_window, false);
    memset(memory_region_get_ram_ptr(&s->boot_window), 0,
           SHADOWCORE_BOOT_WINDOW_SIZE);
    memory_region_flush_rom_device(&s->boot_window, 0,
                                   SHADOWCORE_BOOT_WINDOW_SIZE);
    s->boot_control = s->boot_result = s->boot_generation = 0;
    s->boot_entry = 0;
    loongarch_extioi_set_shadow(s->machine->extioi, 0, false);
    s->shadow_enabled = false;
    s->shadow_result = 0;
    for (i = 0; i < SHADOWCORE_CPUS; i++) {
        ShadowcoreSlot *slot = &s->slots[i];

        slot->accepted_entry = 0;
        slot->state = SHADOWCORE_STATE_ON;
        slot->result = SHADOWCORE_RESULT_OK;
        slot->sequence = slot->accepted = slot->completed = 0;
        qatomic_set(&LOONGARCH_CPU(slot->cpu)->shadowcore_powered_off, false);
    }
}

static void shadowcore_instance_init(Object *obj)
{
    ShadowcoreState *s = SHADOWCORE(obj);

    memory_region_init_io(&s->mmio, obj, &shadowcore_ops, s,
                          TYPE_SHADOWCORE, SHADOWCORE_MMIO_SIZE);
    sysbus_init_mmio(SYS_BUS_DEVICE(obj), &s->mmio);
    memory_region_init_rom_device(&s->boot_window, obj, &shadowcore_boot_ops, s,
                                  "shadowcore-boot-sram",
                                  SHADOWCORE_BOOT_WINDOW_SIZE, &error_fatal);
    memory_region_set_enabled(&s->boot_window, false);
}

static void shadowcore_class_init(ObjectClass *oc, const void *data)
{
    DeviceClass *dc = DEVICE_CLASS(oc);

    dc->desc = "Shadowcore CPU power, Shadow routing and reset trampoline";
    dc->vmsd = &shadowcore_vmstate;
    dc->user_creatable = false;
}

static const TypeInfo shadowcore_type = {
    .name = TYPE_SHADOWCORE,
    .parent = TYPE_SYS_BUS_DEVICE,
    .instance_size = sizeof(ShadowcoreState),
    .instance_init = shadowcore_instance_init,
    .class_init = shadowcore_class_init,
};

static void shadowcore_register_types(void)
{
    type_register_static(&shadowcore_type);
}
type_init(shadowcore_register_types)

void virt_shadowcore_init(LoongArchVirtMachineState *lvms)
{
    MachineState *ms = MACHINE(lvms);
    const CPUArchIdList *cpus = MACHINE_GET_CLASS(ms)->possible_cpu_arch_ids(ms);
    ShadowcoreState *s = SHADOWCORE(qdev_new(TYPE_SHADOWCORE));
    int i;

    assert(cpus->len == SHADOWCORE_CPUS);
    s->machine = lvms;
    s->manager = -1;
    for (i = 0; i < cpus->len; i++) {
        bool little = cpus->cpus[i].props.cluster_id == SHADOWCORE_LITTLE_CLUSTER;
        uint32_t *r = s->records[i];

        r[0] = i;
        r[1] = cpus->cpus[i].arch_id;
        r[2] = cpus->cpus[i].props.core_id;
        r[3] = little;
        r[4] = little ? SHADOWCORE_TEST_MACHINE(lvms)->little_capacity : 1024;
        r[5] = cpus->cpus[i].props.cluster_id;
        r[6] = cpus->cpus[i].props.thread_id;
        s->slots[i].cpu = cpus->cpus[i].cpu;
        s->slots[i].reset_sys = *env_sys(&LOONGARCH_CPU(cpus->cpus[i].cpu)->env);
        if (little) {
            assert(s->manager < 0);
            s->manager = i;
        }
    }
    assert(s->manager >= 0);
    sysbus_realize_and_unref(SYS_BUS_DEVICE(s), &error_fatal);
    sysbus_mmio_map(SYS_BUS_DEVICE(s), 0, SHADOWCORE_MMIO_BASE);
    memory_region_add_subregion_overlap(get_system_memory(), SHADOWCORE_BOOT_VECTOR,
                                        &s->boot_window, 1);
    shadowcore_reset(s);
    qemu_register_reset(shadowcore_reset, s);
}
