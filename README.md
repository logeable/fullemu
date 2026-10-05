# fullemu

fullemu 是一个以可读性和教学为优先的 Rust 操作系统项目。当前实现从 QEMU RISC-V `virt` 平台启动，单独构建一个 no_std 用户程序，将其作为原始镜像加载后进入 U-mode。用户程序通过 Linux RISC-V syscall ABI 调用 `write` 向串口输出；实验也展示特权指令限制，以及 `satp=BARE` 时 U-mode 仍能读写内核数据的事实。源码按内核职责和稳定概念组织，阶段编号只用于文档中的教学脉络。

## 快速开始

需要 Rust 工具链、`riscv64gc-unknown-none-elf` target、`rust-objcopy`（由 `cargo-binutils` 提供）和带 RISC-V system emulation 的 QEMU：

```sh
rustup target add riscv64gc-unknown-none-elf
make build
make run
```

`make build` 会先构建独立的 `user/` 程序，将 ELF 转换为原始二进制，再把二进制镜像作为数据嵌入内核。启动后串口会显示用户程序输出、`write` 返回值、内核数据读写结果，以及读取 `sstatus` 时发生的非法指令陷入。当前 syscall 仅支持串口 `write` 子集，不代表已兼容完整 Linux ABI。按 `Ctrl-C` 结束 QEMU。

若 QEMU 可执行文件不在 PATH，可运行 `make run QEMU=/path/to/qemu-system-riscv64`。也可以单独运行 `make build-user` 构建用户程序。

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
- [第 11 阶段：U-mode 的限制与内存访问边界](docs/phase-11-user-mode-limits.md) 对照 U-mode 的特权限制与 `satp=BARE` 下尚未建立的内存保护。
- [第 12 阶段：独立用户程序与最小加载器](docs/phase-12-user-program-loader.md) 记录独立构建的用户程序、原始镜像加载和当前固定地址限制。
- [第 13 阶段：Linux RISC-V `write` 系统调用](docs/phase-13-user-write-syscall.md) 记录 `ecall` 陷入、Linux syscall 寄存器约定，以及用户程序向串口输出的最小实现。
- [第 14 阶段：Linux RISC-V `exit` 系统调用](docs/phase-14-user-exit-syscall.md) 记录用户任务如何通过系统调用结束，以及单任务阶段的终止行为。
- [项目原则与代码规范](docs/development-principles.md) 是设计和协作规范。
