/* SPDX-License-Identifier: GPL-2.0-or-later */
/* ABI 1.0: per-thread slots, self-off and manager-only CPU_ON. */
#include "qemu/osdep.h"
#include "qapi/error.h"
#include "qemu/log.h"
#include "qemu/main-loop.h"
#include "hw/core/cpu.h"
#include "hw/core/sysbus.h"
#include "hw/loongarch/shadowcore.h"
#include "migration/vmstate.h"
#include "system/reset.h"
#include "target/loongarch/cpu.h"
#include "exec/cputlb.h"

#define TYPE_SHADOWCORE "loongarch-shadowcore"
OBJECT_DECLARE_SIMPLE_TYPE(ShadowcoreState, SHADOWCORE)

typedef struct ShadowcoreSlot {
    CPUState *cpu;
    CPUSysState reset_sys;
    uint64_t entry;
    uint32_t state, result, sequence, accepted, completed;
    uint32_t off_count, on_count;
} ShadowcoreSlot;

struct ShadowcoreState {
    SysBusDevice parent_obj;
    MemoryRegion mmio;
    uint32_t records[SHADOWCORE_CPUS][SHADOWCORE_RECORD_SIZE / 4];
    ShadowcoreSlot slots[SHADOWCORE_CPUS];
    int manager;
    uint64_t generation;
    uint32_t off_count, on_count;
};

typedef struct ShadowcoreWork {
    ShadowcoreState *controller;
    uint64_t generation, entry;
    unsigned index;
    uint32_t command, sequence;
} ShadowcoreWork;

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
        slot->off_count++;
        s->off_count++;
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
        cpu_reset_interrupt(cs, CPU_INTERRUPT_HARD);
        cpu_set_pc(cs, work->entry);
        cs->halted = 0;
        slot->on_count++;
        s->on_count++;
        slot->state = SHADOWCORE_STATE_ON;
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
    pa_bits = FIELD_EX32(LOONGARCH_CPU(slot->cpu)->env.cpucfg[1],
                         CPUCFG1, PALEN) + 1;
    if ((cmd == SHADOWCORE_CMD_CPU_OFF &&
         slot->state != SHADOWCORE_STATE_ON) ||
        (cmd == SHADOWCORE_CMD_CPU_ON &&
         (!slot->entry || (slot->entry & 3) ||
          (pa_bits < 64 && (slot->entry >> pa_bits))))) {
        slot->result = SHADOWCORE_RESULT_INVALID;
        return;
    }

    work = g_new0(ShadowcoreWork, 1);
    work->controller = s;
    work->generation = s->generation;
    work->index = index;
    work->command = cmd;
    work->entry = slot->entry;
    work->sequence = slot->sequence;
    slot->accepted = slot->sequence;
    slot->result = SHADOWCORE_RESULT_OK;
    slot->state = cmd == SHADOWCORE_CMD_CPU_OFF ?
                  SHADOWCORE_STATE_STOPPING : SHADOWCORE_STATE_STARTING;
    /* Match ARM PSCI: stop/start outside translated guest execution. */
    async_run_on_cpu(slot->cpu, shadowcore_power_work, RUN_ON_CPU_HOST_PTR(work));
}

static uint32_t shadowcore_domain_state(ShadowcoreState *s, unsigned index)
{
    unsigned i;

    for (i = 0; i < SHADOWCORE_CPUS; i++) {
        if (s->records[index][2] == s->records[i][2] &&
            s->records[index][5] == s->records[i][5] &&
            s->slots[i].state != SHADOWCORE_STATE_OFF) {
            return 1;
        }
    }
    return 0;
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
    case 0x10: return SHADOWCORE_RECORD_BASE;
    case 0x14: return SHADOWCORE_RECORD_SIZE;
    case 0x18: return s->records[s->manager][1];
    case 0x1c: return SHADOWCORE_SLOT_STRIDE;
    case 0x30: return s->off_count;
    case 0x38: return s->on_count;
    }
    if (addr >= SHADOWCORE_RECORD_BASE &&
        addr < SHADOWCORE_RECORD_BASE + sizeof(s->records)) {
        addr -= SHADOWCORE_RECORD_BASE;
        return s->records[addr / SHADOWCORE_RECORD_SIZE]
                         [(addr % SHADOWCORE_RECORD_SIZE) / 4];
    }
    if (addr >= SHADOWCORE_REG_CPU_STATE &&
        addr < SHADOWCORE_REG_CPU_STATE + SHADOWCORE_CPUS * 4) {
        return s->slots[(addr - SHADOWCORE_REG_CPU_STATE) / 4].state;
    }
    if (addr >= SHADOWCORE_REG_CORE_STATE &&
        addr < SHADOWCORE_REG_CORE_STATE + SHADOWCORE_CPUS * 4) {
        return shadowcore_domain_state(s, (addr - SHADOWCORE_REG_CORE_STATE) / 4);
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
    case SHADOWCORE_SLOT_ENTRY_LO: return (uint32_t)slot->entry;
    case SHADOWCORE_SLOT_ENTRY_HI: return slot->entry >> 32;
    case SHADOWCORE_SLOT_SEQUENCE: return slot->sequence;
    case SHADOWCORE_SLOT_COMPLETED: return slot->completed;
    case SHADOWCORE_SLOT_OFF_COUNT: return slot->off_count;
    case SHADOWCORE_SLOT_ON_COUNT: return slot->on_count;
    default: return 0;
    }
}

static void shadowcore_write(void *opaque, hwaddr addr, uint64_t val, unsigned size)
{
    ShadowcoreState *s = opaque;
    ShadowcoreSlot *slot;
    unsigned index, reg;

    assert(bql_locked());
    if (addr < SHADOWCORE_SLOT_BASE ||
        addr >= SHADOWCORE_SLOT_BASE + SHADOWCORE_SLOT_STRIDE * SHADOWCORE_CPUS) {
        goto invalid;
    }
    index = (addr - SHADOWCORE_SLOT_BASE) / SHADOWCORE_SLOT_STRIDE;
    reg = (addr - SHADOWCORE_SLOT_BASE) % SHADOWCORE_SLOT_STRIDE;
    slot = &s->slots[index];
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
    case SHADOWCORE_SLOT_ENTRY_LO:
        if (current_cpu != s->slots[s->manager].cpu) {
            goto invalid;
        }
        slot->entry = deposit64(slot->entry, 0, 32, val);
        return;
    case SHADOWCORE_SLOT_ENTRY_HI:
        if (current_cpu != s->slots[s->manager].cpu) {
            goto invalid;
        }
        slot->entry = deposit64(slot->entry, 32, 32, val);
        return;
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
    s->off_count = s->on_count = 0;
    for (i = 0; i < SHADOWCORE_CPUS; i++) {
        ShadowcoreSlot *slot = &s->slots[i];

        slot->entry = 0;
        slot->state = SHADOWCORE_STATE_ON;
        slot->result = SHADOWCORE_RESULT_OK;
        slot->sequence = slot->accepted = slot->completed = 0;
        slot->off_count = slot->on_count = 0;
        qatomic_set(&LOONGARCH_CPU(slot->cpu)->shadowcore_powered_off, false);
    }
}

static void shadowcore_instance_init(Object *obj)
{
    ShadowcoreState *s = SHADOWCORE(obj);

    memory_region_init_io(&s->mmio, obj, &shadowcore_ops, s,
                          TYPE_SHADOWCORE, SHADOWCORE_MMIO_SIZE);
    sysbus_init_mmio(SYS_BUS_DEVICE(obj), &s->mmio);
}

static void shadowcore_class_init(ObjectClass *oc, const void *data)
{
    DeviceClass *dc = DEVICE_CLASS(oc);

    dc->desc = "Shadowcore self-off and manager CPU_ON controller";
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
    s->manager = -1;
    for (i = 0; i < cpus->len; i++) {
        bool little = cpus->cpus[i].props.cluster_id !=
                      cpus->cpus[0].props.cluster_id;
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
    shadowcore_reset(s);
    qemu_register_reset(shadowcore_reset, s);
}
