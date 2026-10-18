# fullemu

fullemu 是一个以可读性和教学为优先的 Rust 操作系统项目。当前实现从 QEMU RISC-V `virt` 平台启动，将独立构建的 no_std RV64 ELF 用户程序装入 U-mode。内核默认启动用户态 `init`，由它启动并监督 shell；shell 支持输入、内置命令，以及通过 `fork`、`exec`、`wait` 前台运行嵌入式用户程序。内核解析静态 `ET_EXEC` 的 `PT_LOAD` 段，为代码、只读数据和可写数据建立对应的 Sv39 权限，并实现 Linux RISC-V syscall 的有限子集。程序镜像页由进程单独持有。源码按内核职责和稳定概念组织，阶段编号只用于文档中的教学脉络。

## 快速开始

需要 Rust 工具链、`riscv64gc-unknown-none-elf` target 和带 RISC-V system emulation 的 QEMU：

```sh
rustup target add riscv64gc-unknown-none-elf
make build
make run
```

内核默认启动 `init`，再由它启动 shell。可以通过 `make run BOOT_PROGRAM=fork_wait` 选择其他用户程序作为内核入口，便于直接运行单项实验。当前 shell 可通过输入程序名启动其他嵌入式用户程序；批量启动所有用户程序的旧 batch 模式已移除。

`make build` 会先构建独立的 `user/` 程序，再由内核构建脚本把 ELF 文件嵌入内核。用户程序使用共同虚拟区域 `0x40000000`，链接脚本按页分离可执行代码、只读数据和可写数据；内核装载器解析 ELF 程序头并将各段复制到独立页帧。当前只支持静态 RV64 little-endian `ET_EXEC`，不支持动态链接、PIE、文件系统或命令行参数。`user/target/riscv64gc-unknown-none-elf/release/user-programs.manifest` 记录程序名和 ELF 路径。以 `disabled-` 开头的 bin 源文件会被 `make build-user` 忽略；新增或移除其他 `user/src/bin/` 程序时，镜像清单会随 `make build-user` 更新。若只需编译不含用户程序的内核，可运行 `make build-kernel` 或直接运行 `cargo build --target riscv64gc-unknown-none-elf`。当前 syscall 仅实现有限 Linux ABI 子集，不代表已兼容完整 Linux 用户态。按 `Ctrl-C` 结束 QEMU。

若 QEMU 可执行文件不在 PATH，可运行 `make run QEMU=/path/to/qemu-system-riscv64`。也可以单独运行 `make build-user` 构建用户程序。

查看内核 ELF 的低地址启动入口以及内核主体的高 VMA/低 LMA，可运行 `make inspect-kernel-elf`。

## 代码与阶段说明

- [模块职责与依赖](docs/architecture.md) 说明当前内核模块边界、依赖方向和后续架构重构工作。
- [第 0 阶段：QEMU RISC-V 启动与串口](docs/phase-0-qemu-riscv-boot.md) 记录启动契约、内存布局、运行命令、验证方式和当前限制。
- [第 1 阶段：致命异常报告](docs/phase-1-fatal-traps.md) 说明 S-mode 异常入口和当前验证限制。
- [第 2 阶段：FDT 头部校验](docs/phase-2-fdt-header.md) 记录头部格式和边界检查。
- [第 3 阶段：FDT 结构遍历](docs/phase-3-fdt-structure-walk.md) 记录 token 遍历器、节点摘要和当前限制，`make test-fdt` 可运行解析器测试。
- [第 4 阶段：FDT 完整格式读取器](docs/phase-4-fdt-reader.md) 记录完整 DTB 布局、内存保留表和借用式属性读取。
- [第 5 阶段：FDT 启动内存信息](docs/phase-5-boot-memory-info.md) 记录 `/memory` 解码、保留范围和 `FdtMemoryInfo` 接口。
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
- [第 21 阶段：Sv39 下的多道程序批处理入口](docs/phase-21-sv39-multiprogram-batch.md) 记录历史上的批处理启动实验；该启动模式已随用户态 init、shell 和动态进程创建路径成熟而移除。
- [第 22 阶段：每任务独立 Sv39 页表](docs/phase-22-per-task-page-tables.md) 记录任务地址空间隔离、调度时切换 `satp` 和跨任务访问验证。
- [第 23 阶段：统一用户程序虚拟入口](docs/phase-23-common-user-entry.md) 说明所有用户程序如何通过各自页表从共同虚拟地址映射到独立物理镜像。
- [第 24 阶段：有界内核堆分配器](docs/phase-24-kernel-heap.md) 记录固定 BSS 堆区域、可释放 first-fit 空闲链表、中断守卫和 `Vec` 全局分配自检。
- [第 25 阶段：固定物理页帧池](docs/phase-25-physical-frame-allocator.md) 记录 BSS 中固定页池、位图分配器和分配/释放复用自检。
- [第 26 阶段：地址空间页表回收](docs/phase-26-address-space-reclamation.md) 记录非活动地址空间的页表树销毁和页帧归还。
- [第 27 阶段：用容器管理地址空间记录](docs/phase-27-address-space-container.md) 记录内核 `Vec` 对根页表记录的管理，以及它与物理页帧分配器的职责边界。
- [第 28 阶段：用容器管理任务记录](docs/phase-28-task-record-container.md) 记录任务调度元数据如何从平行数组收拢到 `Vec<Task>`，并说明静态栈与任务上限仍保留。
- [第 29 阶段：任务拥有独立栈](docs/phase-29-task-owned-stacks.md) 记录内核栈和用户栈的稳定页帧 backing、Sv39 权限映射，以及安全回收顺序。
- [第 30 阶段：FDT 驱动的物理页帧与堆后备区](docs/phase-30-fdt-backed-physical-memory.md) 记录按 FDT RAM 范围构造可用页帧区间，并由页帧分配器为内核堆提供后备页。
- [第 31 阶段：从 FDT 发现 UART 控制台](docs/phase-31-fdt-uart-discovery.md) 记录启动时读取 UART 寄存器范围并配置控制台驱动。
- [第 32 阶段：单任务创建与失败回滚](docs/phase-32-single-task-creation.md) 记录批处理和 shell 共用的单任务准备路径、追加式 `TaskId` 及资源回收边界。
- [第 33 阶段：Sv39 内核与用户地址空间规划](docs/phase-33-sv39-address-space-plan.md) 盘点当前实际映射，明确低半部用户区、高半部内核区的目标约定及后续迁移顺序。
- [第 34 阶段：分离用户镜像虚拟地址与物理槽位](docs/phase-34-user-image-virtual-address.md) 将用户程序共同入口迁到 `0x40000000`，避免覆盖 QEMU RAM 恒等映射，并保留固定物理镜像槽位。
- [第 35 阶段：按链接段设置内核页权限](docs/phase-35-kernel-section-permissions.md) 按 4 KiB 页边界将内核代码、只读数据和可写数据映射为不同权限。
- [第 36 阶段：检查高半区迁移前的 ELF 布局](docs/phase-36-high-half-elf-baseline.md) 建立可重复检查内核入口与加载段的工具。
- [第 37 阶段：为内核段建立高地址别名](docs/phase-37-kernel-high-alias.md) 在任务页表中将内核物理页额外映射到高地址。
- [第 38 阶段：显式区分内核段虚拟与物理起点](docs/phase-38-kernel-section-addresses.md) 为后续分离 VMA/LMA 明确页表映射输入。
- [第 39 阶段：从链接器提供内核段装载地址](docs/phase-39-kernel-load-address-symbols.md) 让任务地址空间创建路径区分链接地址与物理装载位置。
- [第 40 阶段：从高地址别名进入陷入入口](docs/phase-40-high-alias-trap-entry.md) 验证 syscall 与定时器陷入可从高地址进入内核。
- [第 41 阶段：从低地址启动并在高半区运行内核](docs/phase-41-high-half-kernel-runtime.md) 在早期启用 Sv39，将 Rust 内核代码、全局数据和栈迁移到高半区 VMA。
- [第 47 阶段：用户态 init 启动并监督 shell](docs/phase-47-user-init.md) 记录 PID 1 的用户态服务启动、shell 重启和孤儿进程回收。
- [第 48 阶段：shell 前台执行嵌入式程序](docs/phase-48-shell-program-execution.md) 记录 shell 如何通过 fork、exec、wait 启动命令程序并返回提示符。
- [第 49 阶段：嵌入式 RV64 ELF 装载](docs/phase-49-elf-loading.md) 记录静态 ELF 程序头校验、段复制、BSS 清零和按段权限建立用户映射。
- [项目原则与代码规范](docs/development-principles.md) 是设计和协作规范。
