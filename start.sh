#!/bin/bash
qemu-11.1.2/build/qemu-system-loongarch64 \
	-M shadowcore-test \
	-kernel linux-7.2.11/arch/loongarch/boot/vmlinux.efi \
	-bios /usr/share/qemu/edk2-loongarch64-code.fd \
	-hda rootfs.img \
	-append "root=/dev/vda console=ttyS0" \
	-nographic -s \
	-m 1G
