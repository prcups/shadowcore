/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _ASM_LOONGARCH_SHADOWCORE_H
#define _ASM_LOONGARCH_SHADOWCORE_H

#include <linux/types.h>
#include <linux/errno.h>

#ifdef CONFIG_LOONGARCH_SHADOWCORE
int shadowcore_cpu_off(unsigned int cpu);
int shadowcore_cpu_on(unsigned int cpu, unsigned long entry);
int shadowcore_cpu_wait_off(unsigned int cpu);
int shadowcore_cpu_disable(unsigned int cpu);
void shadowcore_cpu_start_failed(unsigned int cpu);
bool shadowcore_cpu_is_manager(unsigned int cpu);
#else
static inline int shadowcore_cpu_off(unsigned int cpu) { return -ENODEV; }
static inline int shadowcore_cpu_on(unsigned int cpu, unsigned long entry)
{
	return -ENODEV;
}
static inline int shadowcore_cpu_wait_off(unsigned int cpu) { return -ENODEV; }
static inline int shadowcore_cpu_disable(unsigned int cpu) { return 0; }
static inline void shadowcore_cpu_start_failed(unsigned int cpu) { }
static inline bool shadowcore_cpu_is_manager(unsigned int cpu) { return false; }
#endif

#endif
