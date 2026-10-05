# fullemu

fullemu 是一个以可读性和教学为优先的 Rust 操作系统项目。当前实现从 QEMU RISC-V `virt` 平台启动，展示任务 A 如何改写内核调度状态并导致任务 B 无法运行，以说明当前缺少内存保护。

## 快速开始

需要 Rust 工具链、`riscv64gc-unknown-none-elf` target 和带 RISC-V system emulation 的 QEMU：

```sh
rustup target add riscv64gc-unknown-none-elf
make build
make run
```

启动后串口应显示 `fullemu: booted on QEMU virt (RISC-V)`、hart ID 和 DTB 地址，随后显示 `CURRENT_TASK` 被任务 A 改写、任务 B 无法取得进展，以及定时器 tick。按 `Ctrl-C` 结束 QEMU。

也可以直接运行 `cargo run --release`；Cargo 会调用 `.cargo/config.toml` 配置的 QEMU runner。若 QEMU 可执行文件不在 PATH，可运行 `make run QEMU=/path/to/qemu-system-riscv64`。

## 代码与阶段说明

- [第 0 阶段：QEMU RISC-V 启动与串口](docs/phase-0-qemu-riscv-boot.md) 记录启动契约、内存布局、运行命令、验证方式和当前限制。
- [第 1 阶段：致命异常报告](docs/phase-1-fatal-traps.md) 说明 S-mode 异常入口和当前验证限制。
- [第 2 阶段：FDT 头部校验](docs/phase-2-fdt-header.md) 记录头部格式和边界检查。
- [第 3 阶段：FDT 结构遍历](docs/phase-3-fdt-structure-walk.md) 记录 token 遍历器、节点摘要和当前限制，`make test-fdt` 可运行解析器测试。
- [第 4 阶段：FDT 完整格式读取器](docs/phase-4-fdt-reader.md) 记录完整 DTB 布局、内存保留表和借用式属性读取。
- [第 5 阶段：FDT 启动内存信息](docs/phase-5-boot-memory-info.md) 记录 `/memory` 解码、保留范围和 `BootInfo` 接口。
- [第 6 阶段：UART 单字节输入与回显](docs/phase-6-uart-echo.md) 记录轮询接收、回显行为和验证方法。
- [第 7 阶段：CPU 虚拟化的饥饿对照](docs/phase-7-cpu-starvation-baseline.md) 记录没有调度器时的单 hart 对照实验。
- [第 8 阶段：协作式上下文切换](docs/phase-8-cooperative-context-switch.md) 记录静态任务栈、RISC-V 上下文保存和显式让出。
- [第 9 阶段：定时器抢占](docs/phase-9-timer-preemption.md) 记录完整陷入帧、SBI 定时器和不主动让出的任务如何被轮转。
- [第 10 阶段：任务直接破坏内核调度状态](docs/phase-10-kernel-state-corruption.md) 记录任务如何改写内核关键数据、导致调度失常，并引出内核与用户程序之间的权限边界。
- [项目原则与代码规范](docs/development-principles.md) 是设计和协作规范。
