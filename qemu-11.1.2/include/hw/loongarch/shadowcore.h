/* SPDX-License-Identifier: GPL-2.0-or-later */
#ifndef HW_LOONGARCH_SHADOWCORE_H
#define HW_LOONGARCH_SHADOWCORE_H

#include "hw/loongarch/virt.h"
#include "hw/acpi/aml-build.h"

#define TYPE_SHADOWCORE_TEST_MACHINE MACHINE_TYPE_NAME("shadowcore-test")
OBJECT_DECLARE_SIMPLE_TYPE(ShadowcoreTestMachineState, SHADOWCORE_TEST_MACHINE)

struct ShadowcoreTestMachineState {
    LoongArchVirtMachineState parent_obj;
    uint32_t little_capacity;
};

static inline bool virt_is_shadowcore(const LoongArchVirtMachineState *lvms)
{
    return object_dynamic_cast(OBJECT(lvms), TYPE_SHADOWCORE_TEST_MACHINE) != NULL;
}

#define SHADOWCORE_BIG_CORES       8
#define SHADOWCORE_BIG_THREADS     (SHADOWCORE_BIG_CORES * 2)
#define SHADOWCORE_CPUS            (SHADOWCORE_BIG_THREADS + 1)
#define SHADOWCORE_ABI_VERSION     0x00010000
#define SHADOWCORE_CAP_DISCOVERY   (1U << 0)
#define SHADOWCORE_CAP_CPU_POWER   (1U << 1)
#define SHADOWCORE_CAPABILITIES    (SHADOWCORE_CAP_DISCOVERY | SHADOWCORE_CAP_CPU_POWER)
#define SHADOWCORE_MMIO_BASE       0x100f0000
#define SHADOWCORE_MMIO_SIZE       0x1000
#define SHADOWCORE_MAGIC           0x53484352
#define SHADOWCORE_RECORD_BASE     0x100
#define SHADOWCORE_RECORD_SIZE     0x20
#define SHADOWCORE_REG_CPU_STATE   0x400
#define SHADOWCORE_REG_CORE_STATE  0x480
#define SHADOWCORE_SLOT_BASE       0x500
#define SHADOWCORE_SLOT_STRIDE     0x40
#define SHADOWCORE_SLOT_COMMAND    0x00
#define SHADOWCORE_SLOT_STATE      0x04
#define SHADOWCORE_SLOT_RESULT     0x08
#define SHADOWCORE_SLOT_ENTRY_LO   0x0c
#define SHADOWCORE_SLOT_ENTRY_HI   0x10
#define SHADOWCORE_SLOT_SEQUENCE   0x14
#define SHADOWCORE_SLOT_COMPLETED  0x18
#define SHADOWCORE_SLOT_OFF_COUNT  0x1c
#define SHADOWCORE_SLOT_ON_COUNT   0x20
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
void virt_shadowcore_dsdt(Aml *dsdt, LoongArchVirtMachineState *lvms);
void virt_shadowcore_pptt(GArray *data, BIOSLinker *linker,
                         LoongArchVirtMachineState *lvms);

#endif
