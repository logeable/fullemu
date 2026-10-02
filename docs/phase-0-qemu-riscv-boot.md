# 第 0 阶段：QEMU RISC-V 启动与串口

## 目标与结果

本阶段建立一个最小但真实可启动的内核纵向切片：QEMU 加载 ELF，经 OpenSBI 将控制权交给 Rust 内核；内核在串口输出启动信息，然后停在空闲循环中。

验收结果是可以复现以下串口输出（DTB 地址会因运行环境而异）：

```text
fullemu: booted on QEMU virt (RISC-V)
hart: 0x0000000000000000
DTB:  0x0000000087e00000
```

## 固定的平台契约

| 项目 | 当前约定 |
| --- | --- |
| QEMU 机器 | `virt` 通用虚拟平台 |
| CPU | RV64，QEMU `rv64` CPU |
| hart 数 | 1；尚未实现 SMP 初始化 |
| 固件 | `-bios default`，使用 QEMU 附带的默认 OpenSBI |
| 内核特权级 | S-mode，由 OpenSBI 移交 |
| 加载方式 | `-kernel` 加载 ELF；链接入口和内核基址为 `0x80200000` |
| 固件入口参数 | `a0` 为 hart ID，`a1` 为 FDT/DTB 地址 |
| 调试输出 | QEMU `virt` 的第一个 NS16550 兼容 UART，暂时固定地址 `0x10000000` |
| 设备发现 | 本阶段尚未解析 FDT；后续设备驱动必须从 FDT 发现地址和中断信息 |

本次验证环境：QEMU 10.2.2、Rust/Cargo 1.98.1，OpenSBI 1.7（由当前 QEMU 默认固件提供）。版本用于记录已验证组合，不代表其他版本不支持；遇到启动差异时先记录实际版本和命令行。

QEMU 官方资料将 `virt` 描述为通用虚拟平台，并说明它支持通过默认 OpenSBI 固件加载内核。QEMU 自动生成并传递设备树；平台设备的长期发现机制应基于该设备树，而不是把本阶段的串口常量扩散到其他模块。参见 [QEMU RISC-V `virt` 文档](https://www.qemu.org/docs/master/system/riscv/virt.html)、[QEMU RISC-V 系统仿真文档](https://www.qemu.org/docs/master/system/target-riscv.html) 和 [QEMU `virt` 机器映射源码](https://github.com/qemu/qemu/blob/master/hw/riscv/virt.c)。

RISC-V `unknown-none-elf` target 提供无操作系统运行时的 ELF 构建目标。内核不依赖第三方 crate；链接脚本、入口、串口 MMIO 和最小 panic 输出均由本项目直接实现。参见 [Rust `riscv64gc-unknown-none-elf` target](https://doc.rust-lang.org/stable/nightly-rustc/rustc_target/spec/targets/riscv64gc_unknown_none_elf/index.html)。

## 目录职责

- `src/arch/riscv64/boot.S`：汇编入口，处理单 hart 策略、栈、`gp` 和 `.bss`，再调用 Rust。
- `src/main.rs`：内核入口和 panic 停机路径。
- `src/arch/riscv64/console.rs`：仅用于当前 QEMU 平台的轮询式 UART 输出。
- `linker.ld`：ELF 入口、内存布局和 16 KiB 引导栈。
- `.cargo/config.toml`：默认 target、链接脚本参数和 `cargo run` runner。
- `scripts/run-qemu.sh`：集中维护平台参数，并检查 QEMU 与内核 ELF 是否存在。
- `Makefile`：提供 `build`、`run`、`clean` 和格式化目标。

## 构建与运行

安装 Rust target：

```sh
rustup target add riscv64gc-unknown-none-elf
```

检查工具：

```sh
rustc --version
cargo --version
qemu-system-riscv64 --version
```

构建、运行和清理：

```sh
make build
make run
make clean
```

`make run` 使用默认发布构建。直接使用 Cargo 时，项目 target 默认来自 `.cargo/config.toml`：

```sh
cargo build --release
cargo run --release
```

运行器采用 `-machine virt -cpu rv64 -smp 1 -m 128M -bios default -kernel <ELF> -display none -serial stdio -monitor none`。使用其他 QEMU 可执行文件：

```sh
make run QEMU=/path/to/qemu-system-riscv64
```

## 启动路径说明

1. QEMU 的默认 OpenSBI 固件启动后，将控制权交给 S-mode 内核，并按 RISC-V 启动约定提供 hart ID 和设备树地址。
2. `_start` 暂时只接受 hart 0；其他 hart 进入 `wfi` 停机循环。runner 固定 `-smp 1`，因此正常路径不会触发该分支。
3. 入口设置引导栈和 `gp`，然后将 `.bss` 按 8 字节清零。清零在进入 Rust 前完成，避免依赖 ELF 加载器处理 `NOLOAD` 区域的行为。
4. 汇编入口保留 `a0`/`a1`，尾调用 `kernel_main(hart_id, device_tree)`。
5. Rust 代码向 UART 输出状态，然后通过 `wfi` 保持内核运行。没有启用中断、调度器或关机服务。

## 当前不支持

- 多 hart 初始化、同步和调度。
- FDT 解析及设备自动发现；UART 地址只对当前 `virt` 平台成立。
- 异常/中断处理、内存管理、堆分配、任务、系统调用、文件系统和 shell。
- 从真实开发板启动，或任何 Linux ABI 行为。
- 基于 UART 接收输入；当前只有输出路径。

## 排查

- `make build` 报 target 缺失：运行 `rustup target add riscv64gc-unknown-none-elf`。
- 找不到 QEMU：安装支持 RISC-V system emulation 的 QEMU，或通过 `QEMU=...` 指定可执行文件。
- QEMU 启动但无串口输出：确认使用本阶段规定的 `virt` + 默认 OpenSBI 启动参数；检查 ELF 的入口和段地址，可用 `rust-objdump -h target/riscv64gc-unknown-none-elf/release/fullemu` 查看。
- 退出模拟器：在终端按 `Ctrl-C`。
