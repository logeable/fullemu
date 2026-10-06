# 第 15 阶段：内核日志基础设施

## 目标关联

用户程序已经能通过 `write` 输出，内核也会报告启动、陷入和任务退出信息。随着 syscall、地址空间和调度机制增加，未结构化的串口文本会让人难以分辨消息来源和严重程度。本阶段在现有 UART console 之上增加一层轻量内核日志，为后续观察内核路径和诊断错误提供统一入口。

## 实现范围

- `arch::riscv64::console` 继续负责 UART 字节传输；`kernel::logging` 负责日志级别、调用模块名称和统一前缀。
- `klog_error!`、`klog_warn!`、`klog_info!`、`klog_debug!` 和 `klog_trace!` 宏通过 `module_path!()` 自动记录来源。
- 日志格式为 `[LEVEL tick=N module::path] message`，使用 `core::fmt` 同步写入 UART，不申请堆内存，也不引入第三方依赖。每条通过级别过滤的日志读取一次 RISC-V `time` 计数器，避免被过滤的日志产生额外计时读取。
- `tick` 是单调递增的硬件时间计数，不是调度器周期 tick，也不是日历时间。QEMU `virt` 的 `timebase-frequency` 为 10 MHz，因此计数差 10,000,000 约对应 1 秒；当前日志直接打印原始计数，不做平台相关的频率换算。
- 日志阈值通过编译时环境变量 `FULLEMU_LOG_LEVEL` 选择，级别为 `off`、`error`、`warn`、`info`、`debug`、`trace`；未设置时默认使用 `info`。较高级别自动包含较低级别，例如 `debug` 同时保留错误、警告和信息日志；未识别的值按 `info` 处理。
- 启动摘要、致命错误、U-mode 启动/退出和未处理陷入已改为使用日志。FDT 头部与内存范围使用 `Debug`，结构节点与属性使用 `Trace`，所以默认启动不显示这些明细；指定 `LOG_LEVEL=debug` 或 `LOG_LEVEL=trace` 可重新查看。用户 `write` 始终输出不带内核前缀的原始字节。

## 暂不实现

- 不提供日历时间或统一换算后的毫秒时间；显示的原始计数单位取决于平台 `timebase-frequency`。
- 没有日志缓冲区、串口锁或跨 hart 同步。当前基准运行仅使用一个 hart，日志通过轮询串口同步输出；多个可抢占写入者可能让一条记录交错。
- 阈值只在编译时选择，尚无启动参数或运行时配置。设置为 `off` 可关闭全部内核日志。
- UART 驱动仍固定适配 QEMU `virt`；更换平台时需要替换底层 console，而日志策略接口保持不变。

## 运行与验收

```sh
make fmt-check
make build
make run
```

Makefile 构建可通过 `LOG_LEVEL` 指定阈值，例如 `make LOG_LEVEL=debug run`、`make LOG_LEVEL=trace build` 或 `make LOG_LEVEL=off run`。直接使用 Cargo 时，可设置相同的编译时环境变量：

```sh
FULLEMU_LOG_LEVEL=debug cargo build --release --target riscv64gc-unknown-none-elf
```

QEMU 串口应出现所选阈值允许的日志级别，用户程序的 `write` 文本不应出现日志前缀。默认 `Info` 构建隐藏 FDT 明细；使用 `debug` 可查看 FDT 头部和内存范围，使用 `trace` 还可查看结构节点与属性。当前用户程序正常调用 `exit`，所以未处理陷入日志不会出现在正常启动路径中。

## 后续连接

后续支持其他平台时，应读取平台提供的 `timebase-frequency`，再决定是否在日志中统一显示换算后的时间单位。启用多个 hart 或允许日志从中断上下文输出前，还应确定整条日志记录的原子性与中断安全规则。
