/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _ASM_LOONGARCH_SHADOWCORE_H
#define _ASM_LOONGARCH_SHADOWCORE_H

#include <linux/types.h>
#include <linux/errno.h>
#include <linux/init.h>

struct task_struct;
struct cpumask;

#ifdef CONFIG_LOONGARCH_SHADOWCORE
void __init shadowcore_early_init(void);
const struct cpumask *shadowcore_task_cpus(struct task_struct *task);
const struct cpumask *shadowcore_fallback_cpus(struct task_struct *task);
int shadowcore_irq_target(void);
void loongarch_eiointc_shadow_changed(void);
int shadowcore_cpu_off(unsigned int cpu);
int shadowcore_cpu_on(unsigned int cpu, unsigned long entry);
int shadowcore_cpu_wait_off(unsigned int cpu);
int shadowcore_cpu_disable(unsigned int cpu);
void shadowcore_cpu_start_failed(unsigned int cpu);
bool shadowcore_cpu_is_manager(unsigned int cpu);
bool shadowcore_cpu_hotplug_protected(unsigned int cpu);
#else
static inline void shadowcore_early_init(void) { }
static inline int shadowcore_irq_target(void) { return -1; }
static inline int shadowcore_cpu_off(unsigned int cpu) { return -ENODEV; }
static inline int shadowcore_cpu_on(unsigned int cpu, unsigned long entry)
{
	return -ENODEV;
}
static inline int shadowcore_cpu_wait_off(unsigned int cpu) { return -ENODEV; }
static inline int shadowcore_cpu_disable(unsigned int cpu) { return 0; }
static inline void shadowcore_cpu_start_failed(unsigned int cpu) { }
static inline bool shadowcore_cpu_is_manager(unsigned int cpu) { return false; }
static inline bool shadowcore_cpu_hotplug_protected(unsigned int cpu) { return false; }
#endif

#endif
