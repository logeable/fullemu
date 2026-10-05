# 第 12 阶段：独立用户程序与最小加载器

## 目标关联

U-mode 已证明 CPU 特权级会限制 CSR 访问，但此前执行的入口函数仍编译在内核中。要逐步运行 shell 和 Linux 用户程序，内核必须能执行独立构建的用户程序。本阶段建立“构建用户程序、生成镜像、加载并进入 U-mode”的第一条纵向路径。

## 实验问题

**内核如何启动一个不属于内核代码的用户程序？** 本阶段不读取磁盘或文件系统；用户程序由单独的 Cargo 包构建，构建系统将它转换为原始二进制，并作为数据嵌入内核镜像。内核启动时把镜像复制到固定加载区，再通过 `sret` 进入其入口。

这种先构建独立应用、再由内核装入运行的教学顺序参考 rCore 用户程序章节。rCore 的早期批处理阶段也把应用二进制嵌入内核数据段，之后再扩展多应用和 ELF 加载；本项目当前先实现一个应用和固定加载地址。[rCore：应用程序](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/2application.html)、[rCore：批处理系统](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter2/3batch-system.html)、[rCore：多道程序加载](https://rcore-os.cn/rCore-Tutorial-Book-v3/chapter3/1multi-loader.html)。blog_os 的分页材料可作为后续内存映射与权限实验的概念参考，但其实现面向 x86_64，不能直接复用为本项目的 RISC-V 加载器。[blog_os：分页介绍](https://os.phil-opp.com/paging-introduction/)

## 实现范围

- `user/` 是独立的 no_std Cargo 包，使用相同的 `riscv64gc-unknown-none-elf` 目标和独立链接脚本；用户入口汇编不会链接成内核函数。
- 用户程序链接到固定地址 `0x80400000`。Makefile 先构建用户 ELF，再用 `rust-objcopy` 提取原始二进制；内核通过 `include_bytes!` 将字节镜像作为只读数据打包。
- 内核加载器检查镜像不超过链接脚本预留的 64 KiB 区域，将字节复制到固定地址并执行 `fence.i`，随后以该地址作为 U-mode 入口。
- 本阶段曾通过启动参数传递内核内存探针，让独立用户程序读写内核数据，以观察 `satp=BARE` 下缺少内存隔离的事实。该探针仅服务于本阶段的边界观察，后续阶段已移除，不属于稳定 ABI。
- 独立用户程序尝试读取 S-mode 的 `sstatus`；陷入处理器报告陷入来源、`sepc` 和非法指令原因，以证明程序来自单独构建的镜像并已在 U-mode 执行。

## 暂不实现

- 用户程序仍以原始二进制打包在内核镜像内，没有磁盘、文件系统或运行时外部文件读取。
- 没有 ELF 段解析、重定位、动态加载、多用户程序选择或通用内存分配。
- `satp=BARE` 不提供页级权限。用户程序仍能访问 PMP 允许的内核 RAM；本阶段用探针读写观察过这一限制，但后续代码不再依赖该探针。
- 没有 ELF 段解析、重定位、动态加载、多用户程序选择或通用内存分配，也没有系统调用或用户态输出接口。

## 运行与验收

需要 Rust RISC-V target、`rust-objcopy`（`cargo-binutils`）和 QEMU：

```sh
make build-user
make build
make run
```

串口应显示：

1. 用户镜像入口位于固定加载地址 `0x80400000`。
2. 陷入来源为 U-mode。
3. `sstatus` 读取产生 `Exception(IllegalInstruction)`，`scause` 为 2。

上述行为证明内核加载并运行了独立构建的用户程序；它不证明用户内存已与内核隔离。

## 后续连接

下一步可以引入 Sv39，将用户程序从固定物理执行地址迁移到受 PTE 权限控制的用户地址空间，并验证用户地址不能访问内核映射。之后再逐步扩展 ELF 段加载、多个应用和从文件系统读取程序。
