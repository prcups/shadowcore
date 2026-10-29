# Shadowcore 项目说明

## 范围

- 本仓库是研究与开发的唯一资料范围。除系统编译工具外，不读取或引用仓库外源码、实验数据或网页资料。
- 当前源码树为 linux-7.2.11 和 qemu-11.1.2；开始代码修改前检查实际版本、配置和工作区状态。
- 保留用户已有改动和实验数据。不擅自切版本、清理构建产物、初始化仓库或覆盖数据。
- 描述状态时区分源码实现、构建、运行验证和模型假设。不要把历史结果套用于新的接口或代码状态。

## 平台约定

- 目标拓扑为 8 个双线程大核及 1 个单线程小核；管理小核为启动 CPU，默认 Linux CPU0 为管理核，CPU1–CPU16 为大核线程，SMT 对为 {1,2} 至 {15,16}。实际控制通过 ACPI UID、硬件 ID 和角色映射，不把 Linux CPU 编号当硬件 ID。
- CPU_OFF 必须在 Linux CPU hotplug 完成任务、IRQ 和 per-CPU 资源迁移后由目标 CPU 提交。CPU_ON 由管理核发起，目标从复位窗口跳板进入 Linux secondary 启动入口；不恢复旧架构上下文。
- 管理核保持 online。用户任务执行范围排除管理核；内核线程仍可运行于管理核。
- 用户调度限制在 LoongArch smp_cpus_done() 确认有 active 大核后启用，由紧随其后的 sched_init_smp() 设置 PID 1 affinity 并迁移；此前允许 PID 1 在 CPU0 执行内核初始化。限制启用后在 S3 期间保持有效。
- S3 由管理核上的协调线程发起。其他 online CPU 经 hotplug 执行 CPU_OFF；恢复时通过 CPU_ON 和复位跳板重新启动。管理核保留恢复上下文，其他 CPU 不恢复关断前架构上下文；原先 offline 的 CPU 保持 offline。
- QEMU 的 OFF 是停止 guest vCPU 执行并更新虚拟状态，不是宿主物理断电。容量是调度输入，不是实测性能；能耗结论仅限明示模型与负载条件。

## 当前 Shadowcore MMIO 布局

- ABI 标识保持 1.0（0x00010000）。设备基址 0x100f0000，范围 4 KiB；寄存器为小端、32 位、4 字节对齐。
- 全局头部：0x00 magic，0x04 ABI，0x08 capabilities，0x0c CPU 数，0x10 槽基址，0x14 槽步长，0x18 管理核硬件 ID，0x1c 管理核槽索引；0x20/0x24 是管理核设置的全局 64 位 secondary 入口。
- Shadow 外部中断路由：0x40 控制，0x44 结果。IPI 和本地定时器仍定向到各 CPU。
- 0x50–0x70 保留；Shadowcore 不提供专用宿主 doorbell 或常开唤醒定时器。
- 复位窗口控制：0x74–0x80，管理核开启并封存 0x1c000000 的 4 KiB SRAM 跳板。
- 每 CPU 槽位于 0x100 + 0x20*n。n 是 ACPI CPU 属性数组中的槽序号，不是 Linux CPU 编号。槽含 COMMAND、STATE、RESULT、硬件 ID、请求序号、完成序号和 BOOT_ENTER 确认。
- CPU 映射由 ACPI 属性数组提供；MMIO 每槽硬件 ID用于核对。物理域状态由 ACPI cluster/core 映射和全部线程状态推导，不另设状态镜像寄存器。
- CPU_ON 共用全局入口。管理核串行提交请求；有启动或关断事务未完成时禁止修改入口。请求序号和 BOOT_ENTER 确认用于关联启动完成状态。
- Shadowcore S3 使用标准 ACPI Sleep Control/Status，地址保持在 GED 寄存器区，由 hw/loongarch 平台实现，SLP_TYP=3、SLP_EN=bit5、WAK_STS=bit7（写 1 清除）。QEMU 挂起整机执行，在唤醒通知中置 WAK_STS 后恢复执行；管理核始终为 Linux-online。
- Linux 复用通用 PM 默认保留 CPU0 的 CPU 冻结/恢复流程，不增加主核选择接口、不替换 pm_sleep_disable_secondary_cpus()。事务状态和恢复检查由 LoongArch 的 CPU hotplug/恢复钩子完成，睡眠协调仍复用项目已有 arch_suspend_dispatch 入口。
- S3 可由 QMP system_wakeup 或睡眠期间的 system_powerdown 唤醒；后者被消费为唤醒按钮，不在恢复后再生成关机通知。进入 S3 前已经锁存的 GED 事件使本次睡眠立即返回。
- 不提供 Shadowcore 专用宿主 doorbell、常开定时器、运行时自动关核策略或常开广播定时器；VM migration 不支持。s2idle 使用 Linux 通用空闲流程，不执行 Shadowcore 睡眠关核事务；当前未补齐 GED 的 s2idle wake IRQ 支持。USB 键盘、RTC 等设备触发 S3 的通路不在本次实现范围内。

## 修改与构建

- 按用户当次请求控制范围。协议布局变化时同步检查 Linux 驱动、QEMU 设备、复位跳板和本文。
- 不因代码尚未完成而编造验收结果或保存预期结果。未获用户要求时不运行测试；若构建或运行，只报告实际执行的项目和观察结果。
- Linux 从 linux-7.2.11 根目录使用 .config 执行 make ARCH=loongarch。QEMU 从 qemu-11.1.2 根目录使用 ./configure 和 make；不手工清理或覆盖既有构建目录。
