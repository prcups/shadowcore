/* SPDX-License-Identifier: GPL-2.0-or-later */
/* Shadowcore's standard ACPI extended sleep registers, at the GED address. */
#include "qemu/osdep.h"
#include "hw/acpi/acpi.h"
#include "hw/acpi/generic_event_device.h"
#include "hw/loongarch/shadowcore.h"
#include "system/address-spaces.h"
#include "system/reset.h"
#include "system/runstate.h"

#define SC_WAK_STS    0x80

static uint64_t shadowcore_sleep_read(void *opaque, hwaddr addr, unsigned size)
{
    ShadowcoreTestMachineState *s = opaque;

    return addr == ACPI_GED_REG_SLEEP_STS ? s->sleep_status : 0;
}

static void shadowcore_sleep_write(void *opaque, hwaddr addr, uint64_t data,
                                  unsigned size)
{
    ShadowcoreTestMachineState *s = opaque;
    LoongArchVirtMachineState *lvms = LOONGARCH_VIRT_MACHINE(s);
    unsigned type = (data >> ACPI_GED_SLP_TYP_POS) & ACPI_GED_SLP_TYP_MASK;

    if (addr == ACPI_GED_REG_SLEEP_STS) {
        s->sleep_status &= ~(data & SC_WAK_STS);
        return;
    }
    if (!(data & ACPI_GED_SLP_EN)) {
        return;
    }
    if (type == ACPI_GED_SLP_TYP_S5) {
        qemu_system_shutdown_request(SHUTDOWN_CAUSE_GUEST_SHUTDOWN);
    } else if (type == SHADOWCORE_SLP_TYP_S3) {
        /* A guest event latched before S3 must not strand the sleeping VM. */
        if (ACPI_GED(lvms->acpi_ged)->ged_state.sel) {
            s->sleep_status |= SC_WAK_STS;
            return;
        }
        s->sleeping = true;
        qemu_system_suspend_request();
    }
}

static const MemoryRegionOps shadowcore_sleep_ops = {
    .read = shadowcore_sleep_read,
    .write = shadowcore_sleep_write,
    .endianness = DEVICE_LITTLE_ENDIAN,
    .valid = { .min_access_size = 1, .max_access_size = 1 },
};

static void shadowcore_s3_wakeup(Notifier *notifier, void *data)
{
    ShadowcoreTestMachineState *s = container_of(notifier,
                                              ShadowcoreTestMachineState,
                                              wakeup);

    if (s->sleeping) {
        /* QEMU invokes wakeup notifiers before resuming any guest CPU. */
        s->sleep_status |= SC_WAK_STS;
        s->sleeping = false;
    }
}

static void shadowcore_powerdown(Notifier *notifier, void *data)
{
    LoongArchVirtMachineState *lvms = container_of(notifier,
                                                LoongArchVirtMachineState,
                                                powerdown_notifier);
    ShadowcoreTestMachineState *s = SHADOWCORE_TEST_MACHINE(lvms);

    if (s->sleeping && runstate_check(RUN_STATE_SUSPENDED)) {
        /* Consume this button; do not deliver another shutdown after wake. */
        qemu_system_wakeup_request(QEMU_WAKEUP_REASON_OTHER, NULL);
    } else {
        acpi_send_event(lvms->acpi_ged, ACPI_POWER_DOWN_STATUS);
    }
}

static void shadowcore_s3_reset(void *opaque)
{
    ShadowcoreTestMachineState *s = opaque;

    s->sleeping = false;
    s->sleep_status = 0;
}

void shadowcore_s3_init(ShadowcoreTestMachineState *s)
{
    LoongArchVirtMachineState *lvms = LOONGARCH_VIRT_MACHINE(s);

    /* Overlay only Sleep Control/Status. GED reset and event registers remain
     * at their standard addresses and use their existing implementation. */
    memory_region_init_io(&s->sleep_regs, OBJECT(s), &shadowcore_sleep_ops, s,
                          "shadowcore-acpi-sleep", 2);
    memory_region_add_subregion_overlap(get_system_memory(), VIRT_GED_REG_ADDR,
                                       &s->sleep_regs, 1);
    lvms->powerdown_notifier.notify = shadowcore_powerdown;
    s->wakeup.notify = shadowcore_s3_wakeup;
    qemu_register_wakeup_notifier(&s->wakeup);
    qemu_register_wakeup_support();
    qemu_register_reset(shadowcore_s3_reset, s);
    s->s3_initialized = true;
}

void shadowcore_s3_cleanup(ShadowcoreTestMachineState *s)
{
    if (s->s3_initialized) {
        notifier_remove(&s->wakeup);
        qemu_unregister_reset(shadowcore_s3_reset, s);
    }
}
