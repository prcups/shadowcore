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
#define SHADOWCORE_CAP_DISCOVERY   1
#define SHADOWCORE_MMIO_BASE       0x100f0000
#define SHADOWCORE_MMIO_SIZE       0x1000
#define SHADOWCORE_MAGIC           0x53484352
#define SHADOWCORE_RECORD_BASE     0x100
#define SHADOWCORE_RECORD_SIZE     0x20

void virt_shadowcore_init(LoongArchVirtMachineState *lvms);
void virt_shadowcore_dsdt(Aml *dsdt, LoongArchVirtMachineState *lvms);
void virt_shadowcore_pptt(GArray *data, BIOSLinker *linker,
                         LoongArchVirtMachineState *lvms);

#endif
