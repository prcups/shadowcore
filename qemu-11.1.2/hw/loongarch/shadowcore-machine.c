/* SPDX-License-Identifier: GPL-2.0-or-later */
/* A fixed research platform, deliberately independent of the virt alias. */
#include "qemu/osdep.h"
#include "qapi/error.h"
#include "qapi/visitor.h"
#include "qemu/error-report.h"
#include "system/tcg.h"
#include "hw/loongarch/shadowcore.h"

static const CPUArchIdList *shadowcore_possible_cpu_arch_ids(MachineState *ms)
{
    int i;

    if (ms->possible_cpus) {
        return ms->possible_cpus;
    }
    ms->possible_cpus = g_malloc0(sizeof(CPUArchIdList) +
                                 SHADOWCORE_CPUS * sizeof(CPUArchId));
    ms->possible_cpus->len = SHADOWCORE_CPUS;
    for (i = 0; i < SHADOWCORE_CPUS; i++) {
        CPUArchId *slot = &ms->possible_cpus->cpus[i];
        bool little = i == SHADOWCORE_BIG_THREADS;

        slot->type = ms->cpu_type;
        slot->arch_id = i;
        slot->vcpus_count = 1;
        slot->props.has_socket_id = true;
        slot->props.socket_id = 0;
        slot->props.has_cluster_id = true;
        slot->props.cluster_id = little ? 1 : 0;
        slot->props.has_core_id = true;
        slot->props.core_id = little ? SHADOWCORE_BIG_CORES : i / 2;
        slot->props.has_thread_id = true;
        slot->props.thread_id = little ? 0 : i % 2;
    }
    return ms->possible_cpus;
}

static void shadowcore_test_init(MachineState *ms)
{
    LoongArchVirtMachineState *lvms = LOONGARCH_VIRT_MACHINE(ms);
    MachineClass *parent = MACHINE_CLASS(
        object_class_get_parent(OBJECT_CLASS(MACHINE_GET_CLASS(ms))));

    /*
     * MachineState::smp describes only symmetric layouts. Keep a canonical
     * 17-slot envelope there; per-slot properties and PPTT carry the actual
     * asymmetric hardware topology. Never derive this board from smp.threads.
     */
    if (ms->smp.cpus != SHADOWCORE_CPUS ||
        ms->smp.max_cpus != SHADOWCORE_CPUS ||
        ms->smp.sockets != 1 || ms->smp.cores != SHADOWCORE_CPUS ||
        ms->smp.threads != 1 || ms->smp.clusters != 1 ||
        ms->smp.dies != 1 || ms->smp.modules != 1 ||
        ms->smp.books != 1 || ms->smp.drawers != 1) {
        error_report("shadowcore-test fixes 8 SMT2 big cores + 1 little core; "
                     "omit -smp or use -smp 17 without topology overrides");
        exit(EXIT_FAILURE);
    }
    if (lvms->acpi == ON_OFF_AUTO_OFF) {
        error_report("shadowcore-test requires ACPI; acpi=off is not supported");
        exit(EXIT_FAILURE);
    }
    if (!tcg_enabled()) {
        error_report("shadowcore-test currently supports TCG only");
        exit(EXIT_FAILURE);
    }
    parent->init(ms);
    /* Direct kernel boot publishes only FDT, not the firmware-loaded ACPI. */
    if (!lvms->bios_loaded) {
        error_report("shadowcore-test requires -bios or pflash firmware "
                     "to install ACPI tables; firmware-less boot is unsupported");
        exit(EXIT_FAILURE);
    }
}

static void shadowcore_get_capacity(Object *obj, Visitor *v, const char *name,
                                    void *opaque, Error **errp)
{
    uint32_t value = SHADOWCORE_TEST_MACHINE(obj)->little_capacity;

    visit_type_uint32(v, name, &value, errp);
}

static void shadowcore_set_capacity(Object *obj, Visitor *v, const char *name,
                                    void *opaque, Error **errp)
{
    uint32_t value;

    if (!visit_type_uint32(v, name, &value, errp)) {
        return;
    }
    if (!value || value >= 1024) {
        error_setg(errp, "little-capacity must be in [1, 1023]");
        return;
    }
    SHADOWCORE_TEST_MACHINE(obj)->little_capacity = value;
}

static void shadowcore_test_instance_init(Object *obj)
{
    SHADOWCORE_TEST_MACHINE(obj)->little_capacity = 256;
    MACHINE(obj)->smp.cores = SHADOWCORE_CPUS;
}

static void shadowcore_test_class_init(ObjectClass *oc, const void *data)
{
    MachineClass *mc = MACHINE_CLASS(oc);

    mc->desc = "Shadowcore fixed 8xSMT2+1 LoongArch ACPI research platform";
    mc->is_default = false;
    mc->alias = NULL;
    mc->min_cpus = SHADOWCORE_CPUS;
    mc->max_cpus = SHADOWCORE_CPUS;
    mc->default_cpus = SHADOWCORE_CPUS;
    mc->has_hotpluggable_cpus = false;
    mc->init = shadowcore_test_init;
    mc->possible_cpu_arch_ids = shadowcore_possible_cpu_arch_ids;
    object_class_property_add(oc, "little-capacity", "uint32",
                              shadowcore_get_capacity, shadowcore_set_capacity,
                              NULL, NULL);
    object_class_property_set_description(oc, "little-capacity",
        "Fixed scheduler capacity advertised by ACPI (not measured performance)");
}

static const TypeInfo shadowcore_test_machine_type = {
    .name = TYPE_SHADOWCORE_TEST_MACHINE,
    .parent = TYPE_LOONGARCH_VIRT_MACHINE,
    .instance_size = sizeof(ShadowcoreTestMachineState),
    .instance_init = shadowcore_test_instance_init,
    .class_init = shadowcore_test_class_init,
};

static void shadowcore_test_register_types(void)
{
    type_register_static(&shadowcore_test_machine_type);
}
type_init(shadowcore_test_register_types)
