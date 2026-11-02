/* SPDX-License-Identifier: GPL-2.0-or-later */
#include "qemu/osdep.h"
#include "hw/loongarch/shadowcore.h"
#include "hw/acpi/bios-linker-loader.h"

/* Return offsets relative to the PPTT header, never to the aggregate blob. */
static uint32_t processor(GArray *data, unsigned start, uint32_t flags,
                          uint32_t parent, uint32_t uid)
{
    uint32_t offset = data->len - start;

    build_append_int_noprefix(data, 0, 1); /* Processor hierarchy */
    build_append_int_noprefix(data, 20, 1);
    build_append_int_noprefix(data, 0, 2);
    build_append_int_noprefix(data, flags, 4);
    build_append_int_noprefix(data, parent, 4);
    build_append_int_noprefix(data, uid, 4);
    build_append_int_noprefix(data, 0, 4); /* No invented cache resources */
    return offset;
}

void virt_shadowcore_pptt(GArray *data, BIOSLinker *linker,
                         LoongArchVirtMachineState *lvms)
{
    unsigned start = data->len;
    uint32_t package, cluster, core;
    int i, thread;
    AcpiTable table = { .sig = "PPTT", .rev = 2,
        .oem_id = lvms->oem_id, .oem_table_id = lvms->oem_table_id };

    acpi_table_begin(&table, data);
    /* The heterogeneous package must NOT claim identical implementation. */
    package = processor(data, start, 1 << 0, 0, 0);
    cluster = processor(data, start, 1 << 4, package, 0);
    for (i = 0; i < SHADOWCORE_BIG_CORES; i++) {
        core = processor(data, start, 1 << 4, cluster, 0);
        for (thread = 0; thread < 2; thread++) {
            processor(data, start, (1 << 1) | (1 << 2) | (1 << 3),
                      core, SHADOWCORE_FIRST_BIG_SLOT + i * 2 + thread);
        }
    }
    cluster = processor(data, start, 1 << 4, package, 0);
    /* Single-threaded core is itself the ID-valid leaf, not an SMT thread. */
    processor(data, start, (1 << 1) | (1 << 3), cluster,
              SHADOWCORE_MANAGER_SLOT_ID);
    acpi_table_end(linker, &table);
}

static void property(Aml *properties, const char *name, Aml *value)
{
    Aml *pair = aml_package(2);

    aml_append(pair, aml_string("%s", name));
    aml_append(pair, value);
    aml_append(properties, pair);
}

/* Early service policy: available before AML/device probing or user tasks. */
void virt_shadowcore_control_table(GArray *data, BIOSLinker *linker,
                                  LoongArchVirtMachineState *lvms)
{
    const CPUArchIdList *cpus = MACHINE_GET_CLASS(lvms)->possible_cpu_arch_ids(MACHINE(lvms));
    unsigned manager = SHADOWCORE_MANAGER_SLOT_ID;
    AcpiTable table = { .sig = "SHCT", .rev = 1,
        .oem_id = lvms->oem_id, .oem_table_id = lvms->oem_table_id };

    acpi_table_begin(&table, data);
    build_append_int_noprefix(data, SHADOWCORE_ABI_VERSION, 4);
    build_append_int_noprefix(data, manager, 4); /* ACPI UID */
    build_append_int_noprefix(data, cpus->cpus[manager].arch_id, 4);
    build_append_int_noprefix(data, 1, 4); /* manager excluded from user ISA domain */
    build_append_int_noprefix(data, 0, 4); /* reserved */
    acpi_table_end(linker, &table);
}

void virt_shadowcore_dsdt(Aml *dsdt, LoongArchVirtMachineState *lvms)
{
    MachineState *ms = MACHINE(lvms);
    const CPUArchIdList *cpus = MACHINE_GET_CLASS(ms)->possible_cpu_arch_ids(ms);
    Aml *scope = aml_scope("\\_SB");
    Aml *dev = aml_device("SCPM");
    Aml *crs = aml_resource_template();
    Aml *dsd = aml_package(2);
    Aml *props = aml_package(11);
    Aml *uids = aml_package(cpus->len);
    Aml *hwids = aml_package(cpus->len);
    Aml *cores = aml_package(cpus->len);
    Aml *clusters = aml_package(cpus->len);
    Aml *threads = aml_package(cpus->len);
    Aml *roles = aml_package(cpus->len);
    Aml *capacities = aml_package(cpus->len);
    int i;

    Aml *cpu_container = aml_device("CPUS");

    aml_append(cpu_container, aml_name_decl("_HID", aml_string("ACPI0010")));
    for (i = 0; i < cpus->len; i++) {
        Aml *cpu_dev = aml_device("C%03X", i);
        GArray *mat = g_array_new(false, true, 1);

        /* LoongArch Core PIC, exactly matching this CPU's MADT entry. */
        build_append_int_noprefix(mat, 17, 1);
        build_append_int_noprefix(mat, 15, 1);
        build_append_int_noprefix(mat, 1, 1);
        build_append_int_noprefix(mat, i, 4);
        build_append_int_noprefix(mat, cpus->cpus[i].arch_id, 4);
        build_append_int_noprefix(mat, 1, 4);
        aml_append(cpu_dev, aml_name_decl("_HID", aml_string("ACPI0007")));
        aml_append(cpu_dev, aml_name_decl("_UID", aml_int(i)));
        aml_append(cpu_dev, aml_name_decl("_STA", aml_int(0x0f)));
        aml_append(cpu_dev, aml_name_decl("_MAT",
                   aml_buffer(mat->len, (uint8_t *)mat->data)));
        g_array_free(mat, true);
        /* No _EJ0 or hotplug scan methods on a fixed-inventory machine. */
        aml_append(cpu_container, cpu_dev);
    }
    aml_append(scope, cpu_container);

    aml_append(dev, aml_name_decl("_HID", aml_string("SHDW0001")));
    aml_append(dev, aml_name_decl("_UID", aml_int(0)));
    aml_append(dev, aml_name_decl("_STA", aml_int(0x0f)));
    aml_append(crs, aml_memory32_fixed(SHADOWCORE_MMIO_BASE,
                                      SHADOWCORE_MMIO_SIZE, AML_READ_WRITE));
    aml_append(dev, aml_name_decl("_CRS", crs));
    for (i = 0; i < cpus->len; i++) {
        bool little = cpus->cpus[i].props.cluster_id == SHADOWCORE_LITTLE_CLUSTER;

        aml_append(uids, aml_int(i));
        aml_append(hwids, aml_int(cpus->cpus[i].arch_id));
        aml_append(cores, aml_int(cpus->cpus[i].props.core_id));
        aml_append(clusters, aml_int(cpus->cpus[i].props.cluster_id));
        aml_append(threads, aml_int(cpus->cpus[i].props.thread_id));
        aml_append(roles, aml_int(little));
        aml_append(capacities, aml_int(little ?
                    SHADOWCORE_TEST_MACHINE(lvms)->little_capacity : 1024));
    }
    property(props, "shadowcore,abi-version", aml_int(SHADOWCORE_ABI_VERSION));
    property(props, "shadowcore,capabilities", aml_int(SHADOWCORE_CAPABILITIES));
    property(props, "shadowcore,cpu-count", aml_int(cpus->len));
    property(props, "shadowcore,manager-uid", aml_int(SHADOWCORE_MANAGER_SLOT_ID));
    property(props, "shadowcore,cpu-uids", uids);
    property(props, "shadowcore,hw-ids", hwids);
    property(props, "shadowcore,core-ids", cores);
    property(props, "shadowcore,cluster-ids", clusters);
    property(props, "shadowcore,thread-ids", threads);
    property(props, "shadowcore,roles", roles);
    property(props, "shadowcore,capacities", capacities);
    aml_append(dsd, aml_touuid("DAFFD814-6EBA-4D8C-8A91-BC9BBF4AA301"));
    aml_append(dsd, props);
    aml_append(dev, aml_name_decl("_DSD", dsd));
    aml_append(scope, dev);
    aml_append(dsdt, scope);
}
