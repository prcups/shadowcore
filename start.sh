#!/bin/bash
qemu-11.1.2/build/qemu-system-loongarch64 \
	-kernel linux-7.2.11/build/arch/loongarch/boot/vmlinux.efi \
	-machine shadowcore-test \
	-bios /usr/share/qemu/edk2-loongarch64-code.fd \
	-hda rootfs.img \
	-append "root=/dev/vda console=ttyS0" \
	-nographic -s \
	-m 1G
