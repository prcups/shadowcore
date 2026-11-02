Shadowcore CPU power protocol (ABI 1.0)
=======================================

This is a software-visible TCG power model. Linux CPU hotplug quiesces a
CPU before it powers itself off. A dedicated kthread worker, bound to the CPU
identified by the little/manager role (CPU0 in the default enumeration),
performs CPU_ON and checks OFF completion. The manager remains online, has no
per-CPU sysfs online control, and is rejected by both the architecture
CPU-disable hook and the device CPU_OFF command. The manager is also the
boot IO master. Its bound worker enables Shadow routing without transferring
software IO-master ownership or publishing a CPU0 offline control.
At least one user-capable CPU is retained outside system freezing, because
user tasks cannot fall back to the manager.
The generic hotplug core also rejects management-CPU teardown before parking
threads; its hotplug/target and hotplug/fail controls are hidden. The driver
removes controls already created before platform discovery, and subsequent
CPU-hotplug attribute creation honors the architecture protection hook.

Register layout
---------------

All accesses are aligned, little-endian, 32-bit accesses in the 4-KiB SHDW0001
resource. Each CPU has an independent command slot.
Discovery records at 0x100 are unchanged. Header registers 0x18 and 0x1c
contain the manager hardware ID and the slot stride (0x40). Global 32-bit
modulo counters are at 0x30 (OFF) and 0x38 (ON). At 0x400 + 4*n is a read-only
alias of slot n's state. At 0x480 + 4*n is the physical-domain state for record
n: 0 if every thread sharing its cluster/core is OFF, otherwise 1.

Each record n owns a slot at 0x500 + 0x40*n. The slot index is not a Linux CPU
number: software uses the UID/hardware-ID mapping. All writes to the manager's
own slot are denied; it exposes status only.

===========  ==========  ======================================================
Offset       Access      Meaning
===========  ==========  ======================================================
0x00         WO          Command: 1 OFF_SELF, 2 ON
0x04         RO          State: 0 OFF, 1 ON, 2 STARTING, 3 STOPPING
0x08         RO          Result: 0 success, 1 invalid/denied, 2 already ON,
                         3 busy, 4 stale sequence
0x0c, 0x10   RW          ON physical entry address, low/high halves
0x14         RW          Proposed nonzero increasing request sequence
0x18         RO          Completed sequence, published with final state/result
0x1c, 0x20   RO          Per-thread OFF/ON counters (32-bit modulo)
===========  ==========  ======================================================

Only the manager may stage an ON request; only a big CPU itself may stage its
OFF_SELF request. Staging while STARTING/STOPPING is ignored and logged.
Submission snapshots the entry and sequence under QEMU's BQL. Queued CPU work
retains this snapshot and a reset generation so obsolete work cannot change a
later reset's state. Invalid, unauthorized and read-only writes do not change
power state. Command and unused reads return zero. Reset sets all slots ON,
clears entries/sequences/results/counters, and preserves discovery identity.
VM migration is blocked by the device. Sequences must not wrap within an epoch;
OFF at the final sequence is rejected to reserve a sequence for the next ON.

OFF_SELF and ON
---------------

Linux checks availability before CPU-disable commits, completes teardown,
disables local interrupts and orders its software-dead report before OFF_SELF.
No guest lock is held at the final command write. Successful OFF_SELF cannot
depend on a subsequent read, unlock or cleanup. Brief execution before QEMU
reaches its event loop is limited to an interrupt-disabled idle loop; it cannot
return to the old mailbox or task execution path.

As in ARM PSCI, QEMU uses async_run_on_cpu(). The MMIO callback returns normally
to unwind QEMU locks. The queued callback disables the local timer, marks the
CPU powered off, sets halted/EXCP_HLT, then publishes OFF and the completed
sequence. The CPU has-work check refuses IRQ/IPI wakeups while powered off.
The manager polls OFF with a bounded timeout: Linux CPU_DEAD alone is not a
hardware acknowledgement. Failure to confirm OFF is a fatal hotplug error.

For ON, Linux prepares cpuboot_data and submits physical smpboot_entry through
the manager. QEMU discards mutable architectural context, preserves CPU
capabilities, resets translation state, installs the requested PC and releases
execution, then publishes ON/sequence. This bypasses firmware mailbox re-entry
for CPUs actually powered off. Initial boot before driver initialization, and
CPUs already powered on, still use the native mailbox/IPI path. Linux-online
is established later by start_secondary() and CPU-hotplug callbacks.

The worker never acquires the CPU-hotplug lock: hotplug callers wait for it.
Generic hotplug callers may run on other CPUs; the device ON command and OFF
acknowledgement polling always run on the bound management worker. OFF_SELF
always runs on the target CPU. This distinction also applies to sysfs requests.
Queued caller work is kept alive until flushed; hardware polling is bounded.
This provides manual hotplug. Automatic idle-based offline policy, an always-on
broadcast timer and event-driven CPU_ON are separate work. Hotplug migrates
normal timers and IRQs to online CPUs.

Verification boundary
---------------------

The CPU power implementation is source-reviewed only, without compilation or runtime
tests at the user's request. Acceptance still requires repeated hotplug,
manager offline rejection, concurrent requests, IRQ/IPI while OFF, SMT states,
reset with queued work and plain virt regression checks with bounded timeouts.
The boot manager cannot be offlined. Linux coordination of all-big-cores-off
system sleep requires the matching S3 lifecycle implementation.
Missing hardware acknowledgements or a managed CPU's startup timeout stop the
kernel explicitly; recoverable cancellation of a late CPU_ON is not implemented.
The driver's read-only manager_cpu and power_state attributes expose the
resolved manager and per-thread/domain states and counters. Multiple reads
during a transition are not an atomic snapshot; use completed sequences to
identify finished operations.

Shadow routing and user tasks
-----------------------------

Capability bit 2 advertises the following global registers, all aligned
little-endian 32-bit MMIO: 0x40 CONTROL (0 normal, 1 Shadow), 0x44 OWNER (read-only
manager hardware ID), 0x48 RESULT (0 success, 1 denied/unsupported), and 0x4c
SWITCHES (read-only modulo counter). Only the running manager vCPU may write
CONTROL. Other bits/values are rejected. Repeated requests are idempotent;
reset disables Shadow and clears counters. Switching happens under the BQL.

In Shadow, the final EXTIOI destination of every external vector is the manager.
All instantiated PIC, PCI INTx, MSI, GED, UART and RTC interrupt paths converge
on that controller. IPI and local timer/CPU interrupts keep their per-CPU target
semantics. DMSI uses per-CPU message-vector namespaces and is disabled on this
fixed research machine; explicit dmsi=on fails before boot. Plain virt keeps its
existing model. Traditional EXTIOI decoding retains virt's single-bank rule:
the least significant set bit selects CPU0 through CPU3 (other values select
CPU0). Only CPU-encoded mode interprets a route byte as a full hardware ID.
Shadow directly selects the mapped manager and does not require that extension.

Guest route registers remain the requested normal-mode routes. Pending latched
unacknowledged vectors are moved on enable/disable; masked input status remains
pending until unmask. Acknowledged vectors are not recreated from input levels.
Linux effective_affinity records the Shadow target, while requested affinity
can still differ. The mode is activated by the bound kernel worker only after
the boot manager is online and already owns the IO fabric.
The driver exposes read-only shadow_mode and user_cpus for inspection.

A revision-1 SHCT ACPI table is consumed after MADT parsing and before user-task
creation. Its 20-byte body is ABI version (u32), manager UID (u32), hardware ID
(u32), policy flags (u32, currently 1), and reserved zero (u32). AML and MMIO
identity must agree with this early declaration. User task possible/fallback
masks exclude the manager; fork, affinity and cpuset use this domain. Kernel
threads retain the full possible mask so bound power/IRQ services can execute.
This policy persists for the boot epoch and does not implement ISA emulation.

These Shadow and scheduling changes are source-reviewed only. Compilation and
runtime verification are deferred at the user's request. Needed checks include
pending IRQ movement, masks and acknowledgement, normal-mode route restoration,
reset, manager offline rejection, user task exclusion, and plain virt boot.
