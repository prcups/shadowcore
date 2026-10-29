.. SPDX-License-Identifier: GPL-2.0

Shadowcore S3 suspend
====================

The shadowcore-test machine exposes ACPI S3 through the standard Sleep Control
and Sleep Status registers at the GED register address. QEMU implements these
in the LoongArch platform; the generic GED implementation is unchanged.
The little CPU is the boot CPU, Linux CPU0 by default, and CPU1--CPU16 are the
eight big SMT2 cores, paired as {1,2} through {15,16}. CPU roles and control
requests are resolved through ACPI UID and hardware-ID mappings.

During early boot, PID 1 runs kernel initialization on the management CPU.
LoongArch smp_cpus_done() enables the user-task CPU restriction after confirming
that a user-capable CPU is active; boot stops explicitly if none is available.
The following sched_init_smp() applies the restricted affinity and moves PID 1
off the manager before it enters userspace. The restriction remains enabled
through S3 even while all user-capable CPUs are offline.

Linux runs the suspend transaction on a worker pinned
to the management CPU, which remains online throughout the transaction. Its
execution context and guest memory survive QEMU's platform suspension.

The existing PM helpers retain CPU0 without an architecture-specific primary
selection interface. After device noirq suspend and before syscore suspend,
CPU hotplug takes down
all other online CPUs. Each target submits CPU_OFF after task, interrupt and
per-CPU teardown. LoongArch hotplug hooks record the original online set and
permit the last user CPU to go offline only in this frozen S3 transaction.
The management CPU verifies the OFF states and command
completion sequences before entering S3. A failure restores CPUs already
taken offline before returning through device resume and task thawing.

QEMU resumes the manager after setting the ACPI WAK_STS bit. Linux resumes
syscore services, then restarts exactly the CPUs taken offline by this
transaction. CPU_ON resets their architecture state and starts them through
the sealed reset window at 0x1c000000 and the Linux secondary entry. The old
CPU register contexts are not resumed. CPUs offline before suspend stay
offline. Device resume and task thawing follow CPU restoration.

The manager enters the standard ACPI extended sleep-register operation directly;
it does not require a firmware SADR routine or reset its own architecture state.
The Shadowcore MMIO ABI remains 1.0. Registers 0x50--0x70 remain reserved; there
is no private wake doorbell or always-on timer.

Manual operation
----------------

Use Linux with CONFIG_SUSPEND, CONFIG_SUSPEND_FREEZER and
CONFIG_LOONGARCH_SHADOWCORE enabled, and matching QEMU source. Start QEMU with
``-machine shadowcore-test`` and an ACPI-capable firmware. Add a QMP socket or
use an HMP monitor for host wake requests.

In the guest, as root::

    cat /sys/power/mem_sleep
    SC_DIR=$(dirname "$(find /sys/devices -name s3_status -print -quit)")
    cat "$SC_DIR/s3_status"
    cat /sys/devices/system/cpu/online
    echo deep > /sys/power/mem_sleep
    echo mem > /sys/power/state

Wait for QMP query-status to report ``suspended`` (or HMP ``info status``),
then issue either QMP command after qmp_capabilities negotiation::

    {"execute":"system_wakeup"}
    {"execute":"system_powerdown"}

These are alternative wake requests; issue only one per cycle. HMP accepts the
same command names. A powerdown request while S3 is active is consumed as a wake
request, so it does not leave a second power-button notification to shut down
the resumed guest. In the running state, powerdown retains its usual meaning.

After the suspend write returns, inspect ``s3_status``, ``power_state``, the
online CPU mask and the kernel log. A completed cycle should return phase to 0
and error to 0, increment attempts/all_off, and restore the original online
mask. The phases are 0 idle, 1 offlining, 2 all other CPUs off, 3 restoring.
The all_off counter records CPU-state verification at the S3 entry callback,
not proof that QEMU entered S3; use the host suspended state to confirm platform
entry.

This describes the implementation and manual checks, not recorded runtime
validation. The generic s2idle path does not perform Shadowcore CPU_OFF/CPU_ON.
USB, RTC and network wake of the QEMU S3 platform require separate integration;
ordinary guest IRQ injection alone does not resume a QEMU suspended VM.
