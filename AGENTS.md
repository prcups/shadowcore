# Shadowcore 项目说明

## 范围

- 本仓库是研究与开发的唯一资料范围。除系统编译工具外，不读取或引用仓库外源码、实验数据或网页资料。
- 源码为 linux-7.2.11 和 qemu-11.1.2；修改前核对实际版本、配置和工作区状态。
- 保留用户改动和实验数据；区分源码实现、构建与运行验证。

## 平台约定

- 管理小核为启动 CPU，默认 Linux CPU0 是管理核，CPU1–CPU16 是 8 个 SMT2 大核线程；通过 ACPI UID、硬件 ID 和角色映射控制。
- 管理核保持 online 并拥有初始 IO master，用户任务排除管理核，内核线程仍可在管理核运行。
- CPU_OFF 由目标在 Linux hotplug 完成迁移和拆卸后提交。CPU_ON 由管理核发起，丢弃旧架构上下文并进入 Linux secondary 入口。
- QEMU OFF 表示停止 guest vCPU 执行，不表示宿主物理断电；容量是调度输入，不是实测性能。

## 当前接口

- Shadowcore ABI 为 1.0，MMIO 基址 0x100f0000，4 KiB，小端对齐 32 位访问。
- 本提交保留 discovery records 0x100、线程状态 0x400、物理域状态 0x480、CPU power slots 0x500 + 0x40*n；全局入口与复位窗口尚未加入。
- Shadow 控制位于 0x40，管理核硬件 ID 位于 0x44，结果 0x48，切换计数 0x4c；IPI 和本地定时器保持定向语义。
- QEMU S3 平台接口使用 GED 地址的标准 Sleep Control/Status，由 hw/loongarch 实现。SLP_TYP=3，SLP_EN=bit5，WAK_STS=bit7（写 1 清除）。
- QMP system_wakeup 或睡眠期间的 system_powerdown 唤醒 S3，先置 WAK_STS 再恢复执行；睡眠前锁存的 GED 事件使进入立即返回。没有专用宿主 doorbell 或常开唤醒定时器。
- Linux 完整 S3 CPU 生命周期需要匹配的睡眠协调实现；本接口说明不代表端到端运行验证。VM migration 不支持。

## 修改与构建

- 协议变化同步检查 Linux、QEMU、跳板和本文。未获用户要求时不运行测试，不编造验收结果。
- Linux 从源码根目录使用 .config 执行 make ARCH=loongarch；QEMU 从根目录使用 ./configure 和 make，不手工清理或覆盖构建目录。
