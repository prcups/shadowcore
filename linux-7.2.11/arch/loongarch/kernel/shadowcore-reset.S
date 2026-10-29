/* SPDX-License-Identifier: GPL-2.0 */
/*
 * Copied verbatim to the 4-KiB reset window at physical 0x1c000000.
 * No stack, virtual address, external PC-relative reference or old context
 * is used. The register/slot offsets are the fixed Shadowcore ABI 1.0.
 */
#include <linux/linkage.h>
#include <asm/loongarch.h>
#include <asm/regdef.h>

	.text
	.balign 4
SYM_CODE_START(shadowcore_reset_entry)
	li.w	t0, 0x100f0000
	csrrd	t1, LOONGARCH_CSR_CPUID
	ld.wu	t2, t0, 0x0c	/* CPU slot count */
	ld.wu	t3, t0, 0x10	/* CPU slot base */
	ld.wu	t7, t0, 0x14	/* CPU slot stride */
	move	t4, zero		/* slot index, not a Linux CPU ID */
1:
	mul.d	t5, t4, t7
	add.d	t5, t5, t3
	add.d	t5, t5, t0
	ld.wu	t6, t5, 0x0c	/* slot hardware CPU ID */
	beq	t1, t6, 2f
	addi.d	t4, t4, 1
	bltu	t4, t2, 1b
3:
	idle	0
	b	3b			/* bad mapping: manager timeout is fatal */
2:
	ld.wu	t6, t0, 0x20	/* global secondary entry, low half */
	ld.wu	t7, t0, 0x24	/* global secondary entry, high half */
	slli.d	t7, t7, 32
	or	t6, t6, t7
	addi.w	t1, zero, 1
	dbar	0
	st.w	t1, t5, 0x18	/* BOOT_ENTER: acknowledge reset-vector execution */
	ld.wu	t1, t5, 0x08	/* proceed only if acknowledgement was accepted */
	bnez	t1, 3b
	jirl	zero, t6, 0	/* smpboot_entry sets modes, DMW, stack and per-CPU */
SYM_INNER_LABEL(shadowcore_reset_entry_end, SYM_L_GLOBAL)
SYM_CODE_END(shadowcore_reset_entry)
