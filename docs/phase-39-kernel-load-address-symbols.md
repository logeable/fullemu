# 第 39 阶段：从链接器读取内核段的物理装载地址

## 目标关联

第 38 阶段让地址空间接口能够分别描述段的运行虚拟地址和物理地址，但调用方仍把同一个低地址符号填入两者。本阶段让链接器明确导出各段的装载起点，并由任务地址空间创建路径读取它们，消除“物理起点等于虚拟起点”的推测。

## 学习重点

链接器同时决定段的运行地址（VMA）和装载地址（LMA）。页表建立时需要将 VMA 映射到 LMA 对应的物理页；两者相等只是当前布局的选择，不是页表接口应依赖的规则。

## 实现边界

- `linker.ld` 为 `.text`、`.rodata`、`.data` 和 `.bss` 导出 `__*_load_start`，值由 `LOADADDR` 取得。
- `task::creation` 用 `__*_start` 填写虚拟起点，用 `__*_load_start` 填写物理起点，段长度仍由对应的 `__*_end - __*_start` 得到。
- 当前 `.bss` 范围从 `.bss` 开始延伸至 `__bss_end`，包含其后的启动栈；当前链接布局将该范围连续放在同一物理区域中。
- linker script 仍让 VMA=LMA；不改变 ELF 加载方式、内核入口、运行地址或页表切换时机。

## 验收方式

- `make build` 和 `make inspect-kernel-elf` 成功，ELF 继续满足低地址 VMA=LMA 基线。
- 使用 `rust-nm -n target/riscv64gc-unknown-none-elf/release/fullemu` 查看 `__text_load_start`、`__rodata_load_start`、`__data_load_start`、`__bss_load_start`，当前值应分别等于对应段起点。
- QEMU shell 能启动并执行 `help`；调试页表中的内核映射仍指向相同物理区域。

## 后续连接

地址空间创建现在会消费链接器提供的装载地址。真正分离 VMA 与 LMA 前，仍需设计独立的低地址启动入口和早期页表，并明确 BSS、启动栈、`gp`、`sp`、`stvec` 及物理内存访问在切换前后的地址语义。
