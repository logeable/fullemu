# fullemu

fullemu 是一个以可读性和教学为优先的 Rust 操作系统项目。当前仓库实现第一个可运行里程碑：从 QEMU RISC-V `virt` 平台的 OpenSBI 固件进入内核，并通过串口打印启动信息。

## 快速开始

需要 Rust 工具链、`riscv64gc-unknown-none-elf` target 和带 RISC-V system emulation 的 QEMU：

```sh
rustup target add riscv64gc-unknown-none-elf
make build
make run
```

启动后串口应显示 `fullemu: booted on QEMU virt (RISC-V)`、hart ID 和 DTB 地址。按 `Ctrl-C` 结束 QEMU。

也可以直接运行 `cargo run --release`；Cargo 会调用 `.cargo/config.toml` 配置的 QEMU runner。若 QEMU 可执行文件不在 PATH，可运行 `make run QEMU=/path/to/qemu-system-riscv64`。

## 代码与阶段说明

- [第 0 阶段：QEMU RISC-V 启动与串口](docs/phase-0-qemu-riscv-boot.md) 记录启动契约、内存布局、运行命令、验证方式和当前限制。
- [项目原则与代码规范](docs/development-principles.md) 是设计和协作规范。
