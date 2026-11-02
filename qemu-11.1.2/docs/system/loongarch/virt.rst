:orphan:

==========================================
loongson3 virt generic platform (``virt``)
==========================================

The ``virt`` machine use gpex host bridge, and there are some
emulated devices on virt board, such as loongson7a RTC device,
IOAPIC device, ACPI device and so on.

Supported devices
-----------------

The ``virt`` machine supports:

* Gpex host bridge
* Ls7a RTC device
* Ls7a IOAPIC device
* ACPI GED device
* Fw_cfg device
* PCI/PCIe devices
* Memory device
* CPU device. Type: la464.

CPU and machine Type
--------------------

The ``qemu-system-loongarch64`` provides emulation for virt
machine. You can specify the machine type ``virt`` and
cpu type ``la464``.

Boot options
------------

We can boot the LoongArch virt machine by specifying the uefi bios,
initrd, and linux kernel. And those source codes and binary files
can be accessed by following steps.

(1) Build qemu-system-loongarch64:

.. code-block:: bash

  ./configure --target-list="loongarch64-softmmu"
  make -j8

(2) Set cross tools:

.. code-block:: bash

  wget https://github.com/loongson/build-tools/releases/download/2022.09.06/loongarch64-clfs-6.3-cross-tools-gcc-glibc.tar.xz
  tar -vxf loongarch64-clfs-6.3-cross-tools-gcc-glibc.tar.xz  -C /opt
  export PATH=/opt/cross-tools/bin:$PATH
  export LD_LIBRARY_PATH=/opt/cross-tools/lib:$LD_LIBRARY_PATH
  export LD_LIBRARY_PATH=/opt/cross-tools/loongarch64-unknown-linux-gnu/lib/:$LD_LIBRARY_PATH

Note: You need get the latest cross-tools at https://github.com/loongson/build-tools

(3) Build BIOS:

    See: https://github.com/tianocore/edk2/tree/master/OvmfPkg/LoongArchVirt#readme

Note: To build the release version of the bios,  set --buildtarget=RELEASE,
      the bios file path:  Build/LoongArchQemu/RELEASE_GCC5/FV/QEMU_EFI.fd

(4) Build kernel:

.. code-block:: bash

  git clone https://github.com/loongson/linux.git
  cd linux
  git checkout loongarch-next
  make ARCH=loongarch CROSS_COMPILE=loongarch64-unknown-linux-gnu- loongson3_defconfig
  make ARCH=loongarch CROSS_COMPILE=loongarch64-unknown-linux-gnu- -j32

Note: The branch of linux source code is loongarch-next.
      the kernel file: arch/loongarch/boot/vmlinuz.efi

(5) Get initrd:

  You can use busybox tool and the linux modules to make a initrd file. Or you can access the
  binary files: https://github.com/yangxiaojuan-loongson/qemu-binary

.. code-block:: bash

  git clone https://github.com/yangxiaojuan-loongson/qemu-binary

Note: the initrd file is ramdisk

(6) Booting LoongArch:

.. code-block:: bash

  $ ./build/qemu-system-loongarch64 -machine virt -m 4G -cpu la464 \
      -smp 1 -bios QEMU_EFI.fd -kernel vmlinuz.efi -initrd ramdisk \
      -serial stdio   -monitor telnet:localhost:4495,server,nowait \
      -append "root=/dev/ram rdinit=/sbin/init console=ttyS0,115200" \
      --nographic

Shadowcore S3 platform
---------------------

The derived ``shadowcore-test`` machine advertises ACPI ``_S3``. Its LoongArch
platform implements the standard Sleep Control/Status registers at the GED
register address; the generic GED code is unchanged. Ordinary ``virt`` keeps
its existing sleep state declarations. This platform interface requires matching guest suspend
support; it does not by itself coordinate Linux CPU hotplug.

The first CPU is the little boot/management CPU. The remaining sixteen CPU
slots describe eight big SMT2 cores. ACPI UID, hardware ID and role properties
identify the CPUs; the default Linux numbering is CPU0 for the manager and
CPU1--CPU16 for the big threads.

Writing SLP_TYP=3 with SLP_EN set in the ACPI Sleep Control register suspends
the VM through QEMU's standard suspend framework. Memory and the execution
context of the management CPU survive. CPUs powered off by the guest remain
in the Shadowcore OFF state until the guest issues CPU_ON.

After QMP ``query-status`` reports ``suspended``, either ``system_wakeup`` or
``system_powerdown`` resumes the platform. A wakeup notifier sets WAK_STS
(bit 7) in the ACPI Sleep Status register before guest execution resumes.
WAK_STS is cleared by writing one. A powerdown request received during S3 is
consumed as a wake request and does not enqueue a second power-button event.
Powerdown requests received while running retain their usual GED semantics.

A GED event already latched at S3 entry causes immediate return with WAK_STS
set. Guest memory and manager execution context are retained without a firmware
reset on wakeup. Other CPUs use the Shadowcore CPU_OFF/CPU_ON protocol defined
by the matching Linux driver; they must not resume their previous contexts.
No private Shadowcore wake register or host doorbell is used. USB, RTC and
network events are not connected to this platform wake mechanism, and
Shadowcore VM migration remains unsupported.
