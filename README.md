# fullemu

fullemu 是一个以可读性和教学为优先的 Rust 操作系统项目。当前实现从 QEMU RISC-V `virt` 平台启动，将独立构建的 no_std 用户程序作为原始镜像加载后进入 U-mode。内核默认启动用户态 shell，并通过静态 Sv39 页表建立用户任务与内核之间的页面权限边界；也可选择多道程序批处理入口，让构建清单中的非 shell 程序同时驻留并由定时器调度。shell 支持输入、通过 `help` 查看内置命令，以及通过 `uptime` 显示单调运行时间。内核实现 Linux RISC-V `read`、`write`、`clock_gettime`、`exit` 和 `sched_yield` 的有限子集。每个批处理任务有独立页表，用户程序从共同虚拟入口运行，但仍使用静态物理镜像槽位、页表和任务栈。源码按内核职责和稳定概念组织，阶段编号只用于文档中的教学脉络。

## 快速开始

需要 Rust 工具链、`riscv64gc-unknown-none-elf` target、`rust-objcopy`（由 `cargo-binutils` 提供）和带 RISC-V system emulation 的 QEMU：

```sh
rustup target add riscv64gc-unknown-none-elf
make build
make run
```

内核默认以 shell 模式启动 `shell`。运行 `make run BOOT_MODE=batch` 可启动多道程序批处理；默认通过 `BATCH_EXCLUDED_BINS=shell` 在构建时排除交互式 shell。可调整该变量指定不参与批处理的程序。也可在 shell 模式下选择其他入口，例如 `make run BOOT_PROGRAM=memory_fault`。这两项配置分别选择运行模式和 shell 模式的入口程序。

`make build` 会先构建独立的 `user/` 程序，将每个 bin 链接到共同虚拟地址 `0x80400000`，再把 ELF 转换为原始二进制并由内核复制到各自的固定物理槽位。`user/target/riscv64gc-unknown-none-elf/release/user-programs.manifest` 记录程序名、虚拟基址和镜像路径；链接脚本保证入口位于虚拟基址，内核构建会校验所有程序使用统一虚拟基址，运行时页表将其映射到当前程序的物理槽位。以 `disabled-` 开头的 bin 源文件会被 `make build-user` 和用户构建脚本忽略；新增或移除其他 `user/src/bin/` 程序时，布局及镜像清单会随 `make build-user` 更新。若只需编译不含用户程序的内核，可运行 `make build-kernel` 或直接运行 `cargo build --target riscv64gc-unknown-none-elf`。当前 syscall 仅实现上述 Linux ABI 的有限子集，不代表已兼容完整 Linux 用户态。按 `Ctrl-C` 结束 QEMU。

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
- [第 15 阶段：内核日志基础设施](docs/phase-15-kernel-logging.md) 记录日志级别、模块来源、过滤策略，以及日志与用户控制台输出的边界。
- [第 16 阶段：多个嵌入式用户程序批量执行](docs/phase-16-user-program-batch.md) 记录独立用户二进制的构建、逐个装载、通过 `exit` 切换及当前批处理限制。
- [第 17 阶段：协作式多道用户程序](docs/phase-17-cooperative-multiprogramming.md) 记录多个用户程序同时驻留、`sched_yield` 轮转和当前调度限制。
- [第 18 阶段：用户任务定时器抢占](docs/phase-18-user-timer-preemption.md) 记录 SBI 定时器如何让不主动让出的 U-mode 任务被切出，避免其他就绪任务饥饿。
- [第 19 阶段：用户态简易 shell](docs/phase-19-user-shell.md) 记录内核默认启动 shell、UART 输入系统调用，以及 `help` 和 `uptime` 内置命令。
- [第 20 阶段：Sv39 用户/内核页面权限边界](docs/phase-20-sv39-user-protection.md) 记录静态恒等映射、U/S 页面权限和系统调用访问用户缓冲区的方式。
- [第 21 阶段：Sv39 下的多道程序批处理入口](docs/phase-21-sv39-multiprogram-batch.md) 记录批处理刚接入 Sv39 时的共享页表设计，以及禁用 bin 的构建约定。
- [第 22 阶段：每任务独立 Sv39 页表](docs/phase-22-per-task-page-tables.md) 记录任务地址空间隔离、调度时切换 `satp` 和跨任务访问验证。
- [第 23 阶段：统一用户程序虚拟入口](docs/phase-23-common-user-entry.md) 说明所有用户程序如何通过各自页表从共同虚拟地址映射到独立物理镜像。
- [第 24 阶段：有界内核堆分配器](docs/phase-24-kernel-heap.md) 记录固定 BSS 堆区域、可释放 first-fit 空闲链表、中断守卫和 `Vec` 全局分配实验。
- [第 25 阶段：固定物理页帧池](docs/phase-25-physical-frame-allocator.md) 记录 BSS 中固定页池、位图分配器和分配/释放复用实验。
- [项目原则与代码规范](docs/development-principles.md) 是设计和协作规范。
