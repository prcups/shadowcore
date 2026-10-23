/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Shadowcore ABI 1.0 discovery bank; no power-control commands yet. */
#include "qemu/osdep.h"
#include "qapi/error.h"
#include "qemu/log.h"
#include "hw/core/sysbus.h"
#include "hw/loongarch/shadowcore.h"
#include "migration/vmstate.h"

#define TYPE_SHADOWCORE "loongarch-shadowcore"
OBJECT_DECLARE_SIMPLE_TYPE(ShadowcoreState, SHADOWCORE)

struct ShadowcoreState {
    SysBusDevice parent_obj;
    MemoryRegion mmio;
    uint32_t records[SHADOWCORE_CPUS][SHADOWCORE_RECORD_SIZE / 4];
};

static uint64_t shadowcore_read(void *opaque, hwaddr addr, unsigned size)
{
    ShadowcoreState *s = opaque;

    switch (addr) {
    case 0x00:
        return SHADOWCORE_MAGIC;
    case 0x04:
        return SHADOWCORE_ABI_VERSION;
    case 0x08:
        return SHADOWCORE_CAP_DISCOVERY;
    case 0x0c:
        return SHADOWCORE_CPUS;
    case 0x10:
        return SHADOWCORE_RECORD_BASE;
    case 0x14:
        return SHADOWCORE_RECORD_SIZE;
    }
    if (addr >= SHADOWCORE_RECORD_BASE &&
        addr < SHADOWCORE_RECORD_BASE + sizeof(s->records)) {
        addr -= SHADOWCORE_RECORD_BASE;
        return s->records[addr / SHADOWCORE_RECORD_SIZE]
                         [(addr % SHADOWCORE_RECORD_SIZE) / 4];
    }
    return 0;
}

static void shadowcore_write(void *opaque, hwaddr addr, uint64_t val,
                             unsigned size)
{
    qemu_log_mask(LOG_GUEST_ERROR,
                  "shadowcore: write to read-only discovery bank at 0x%"
                  HWADDR_PRIx "\n", addr);
}

static const MemoryRegionOps shadowcore_ops = {
    .read = shadowcore_read,
    .write = shadowcore_write,
    .endianness = DEVICE_LITTLE_ENDIAN,
    .valid = { .min_access_size = 4, .max_access_size = 4,
               .unaligned = false },
    .impl = { .min_access_size = 4, .max_access_size = 4 },
};

static const VMStateDescription shadowcore_vmstate = {
    .name = TYPE_SHADOWCORE,
    .unmigratable = 1,
};

static void shadowcore_instance_init(Object *obj)
{
    ShadowcoreState *s = SHADOWCORE(obj);
    SysBusDevice *sbd = SYS_BUS_DEVICE(obj);

    memory_region_init_io(&s->mmio, obj, &shadowcore_ops, s,
                          TYPE_SHADOWCORE, SHADOWCORE_MMIO_SIZE);
    sysbus_init_mmio(sbd, &s->mmio);
}

static void shadowcore_class_init(ObjectClass *oc, const void *data)
{
    DeviceClass *dc = DEVICE_CLASS(oc);

    dc->desc = "Shadowcore read-only platform discovery (no power control)";
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

    for (i = 0; i < cpus->len; i++) {
        bool little = i == SHADOWCORE_BIG_THREADS;
        uint32_t *r = s->records[i];

        r[0] = i; /* ACPI UID, not a Linux logical CPU number */
        r[1] = cpus->cpus[i].arch_id;
        r[2] = cpus->cpus[i].props.core_id;
        r[3] = little; /* 0 = big, 1 = little */
        r[4] = little ? SHADOWCORE_TEST_MACHINE(lvms)->little_capacity : 1024;
        r[5] = cpus->cpus[i].props.cluster_id;
        r[6] = cpus->cpus[i].props.thread_id;
    }
    sysbus_realize_and_unref(SYS_BUS_DEVICE(s), &error_fatal);
    sysbus_mmio_map(SYS_BUS_DEVICE(s), 0, SHADOWCORE_MMIO_BASE);
}
