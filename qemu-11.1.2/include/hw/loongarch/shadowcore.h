/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef HW_LOONGARCH_SHADOWCORE_H
#define HW_LOONGARCH_SHADOWCORE_H

#include "hw/loongarch/virt.h"
#include "hw/acpi/aml-build.h"
#include "qemu/notify.h"

#define TYPE_SHADOWCORE_TEST_MACHINE MACHINE_TYPE_NAME("shadowcore-test")
OBJECT_DECLARE_SIMPLE_TYPE(ShadowcoreTestMachineState, SHADOWCORE_TEST_MACHINE)

struct ShadowcoreTestMachineState {
    LoongArchVirtMachineState parent_obj;
    uint32_t little_capacity;
    MemoryRegion sleep_regs;
    Notifier wakeup;
    bool s3_initialized;
    bool sleeping;
    uint8_t sleep_status;
};

static inline bool virt_is_shadowcore(const LoongArchVirtMachineState *lvms)
{
    return object_dynamic_cast(OBJECT(lvms), TYPE_SHADOWCORE_TEST_MACHINE) != NULL;
}

#define SHADOWCORE_BIG_CORES       8
#define SHADOWCORE_BIG_THREADS     (SHADOWCORE_BIG_CORES * 2)
#define SHADOWCORE_CPUS            (SHADOWCORE_BIG_THREADS + 1)
#define SHADOWCORE_MANAGER_SLOT_ID 0
#define SHADOWCORE_FIRST_BIG_SLOT  1
#define SHADOWCORE_LITTLE_CLUSTER  1
#define SHADOWCORE_SLP_TYP_S3      3
#define SHADOWCORE_ABI_VERSION     0x00010000
#define SHADOWCORE_CAP_DISCOVERY   (1U << 0)
#define SHADOWCORE_CAP_CPU_POWER   (1U << 1)
#define SHADOWCORE_CAP_SHADOW      (1U << 2)
#define SHADOWCORE_CAP_BOOT_REMAP  (1U << 4)
#define SHADOWCORE_CAPABILITIES    (SHADOWCORE_CAP_DISCOVERY | SHADOWCORE_CAP_CPU_POWER | SHADOWCORE_CAP_SHADOW | SHADOWCORE_CAP_BOOT_REMAP)
#define SHADOWCORE_MMIO_BASE       0x100f0000
#define SHADOWCORE_MMIO_SIZE       0x1000
#define SHADOWCORE_MAGIC           0x53484352
#define SHADOWCORE_SLOT_BASE       0x100
#define SHADOWCORE_SLOT_STRIDE     0x20
#define SHADOWCORE_MANAGER_HARDWARE_ID 0x18
#define SHADOWCORE_MANAGER_SLOT    0x1c
#define SHADOWCORE_BOOT_ENTRY_LO   0x20
#define SHADOWCORE_BOOT_ENTRY_HI   0x24
#define SHADOWCORE_SHADOW_CONTROL  0x40
#define SHADOWCORE_SHADOW_RESULT   0x44
#define SHADOWCORE_BOOT_CONTROL    0x74
#define SHADOWCORE_BOOT_RESULT     0x78
#define SHADOWCORE_BOOT_SIZE       0x7c
#define SHADOWCORE_BOOT_GENERATION 0x80
#define SHADOWCORE_BOOT_VECTOR     0x1c000000ULL
#define SHADOWCORE_BOOT_WINDOW_SIZE 0x1000
#define SHADOWCORE_BOOT_ENABLE     1
#define SHADOWCORE_BOOT_SEAL       2
#define SHADOWCORE_BOOT_READY      (SHADOWCORE_BOOT_ENABLE | SHADOWCORE_BOOT_SEAL)
#define SHADOWCORE_SLOT_COMMAND    0x00
#define SHADOWCORE_SLOT_STATE      0x04
#define SHADOWCORE_SLOT_RESULT     0x08
#define SHADOWCORE_SLOT_HARDWARE_ID 0x0c
#define SHADOWCORE_SLOT_SEQUENCE   0x10
#define SHADOWCORE_SLOT_COMPLETED  0x14
#define SHADOWCORE_SLOT_BOOT_ENTER 0x18
#define SHADOWCORE_CMD_CPU_OFF     1
#define SHADOWCORE_CMD_CPU_ON      2
#define SHADOWCORE_RESULT_OK       0
#define SHADOWCORE_RESULT_INVALID  1
#define SHADOWCORE_RESULT_ALREADY  2
#define SHADOWCORE_RESULT_BUSY     3
#define SHADOWCORE_RESULT_STALE    4
#define SHADOWCORE_STATE_OFF      0
#define SHADOWCORE_STATE_ON       1
#define SHADOWCORE_STATE_STARTING 2
#define SHADOWCORE_STATE_STOPPING 3

void virt_shadowcore_init(LoongArchVirtMachineState *lvms);
void shadowcore_s3_init(ShadowcoreTestMachineState *s);
void shadowcore_s3_cleanup(ShadowcoreTestMachineState *s);
void virt_shadowcore_dsdt(Aml *dsdt, LoongArchVirtMachineState *lvms);
void virt_shadowcore_pptt(GArray *data, BIOSLinker *linker,
                         LoongArchVirtMachineState *lvms);
void virt_shadowcore_control_table(GArray *data, BIOSLinker *linker,
                                  LoongArchVirtMachineState *lvms);

#endif
