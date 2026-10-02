# 第 1 阶段：S-mode 致命异常报告

## 目标

在启动路径中安装一个最小的 S-mode 陷入入口。发生同步异常时，读取并打印 `scause`、`sepc`、`stval`，随后让当前 hart 停机。本阶段不返回到异常指令，也不实现中断、用户态或上下文切换。

RISC-V 特权架构通过 `stvec` 指定 S-mode 陷入入口，并在陷入时记录 `scause`、`sepc`、`stval` 等状态。断点异常的异常码为 3，因此 `ebreak` 可用作本阶段的受控验证。参见 [RISC-V 特权架构规范：Supervisor-Level ISA](https://docs.riscv.org/reference/isa/priv/supervisor.html)。

## 实现范围

- 启动汇编在栈和 `gp` 初始化后写入 `stvec`，再继续清零 `.bss` 并进入 Rust。
- `src/arch/riscv64/trap.S` 只读取三个陷入 CSR，并按 C ABI 将值传给 Rust。
- Rust 报告函数输出原始寄存器值，然后进入 `wfi` 停机循环。
- 由于此处理器永不返回，不保存通用寄存器现场；后续若要恢复执行或切换任务，必须重新设计陷入帧。
- `trap-demo` Cargo feature 只用于可复现验证，默认构建不会触发异常。

## 验证

正常启动仍使用：

```sh
make run
```

触发一次 `ebreak` 并观察异常报告：

```sh
make trap-demo
```

验证输出应包含 `fullemu: fatal S-mode trap` 和三项 CSR。`scause` 应为 `0x0000000000000003`（断点异常）；`sepc` 应指向触发异常的指令。`stval` 的内容由异常类型与实现定义，不作为本阶段固定值。

## 当前限制

- 只覆盖当前 S-mode 内核栈上的致命陷入；尚无用户态地址空间或独立陷入栈。
- 陷入后不恢复寄存器、不推进 `sepc`、不执行 `sret`。
- 尚未区分或处理 S-mode 中断；本阶段未启用 S-mode 中断。
- 不保证能处理陷入入口自身或串口输出路径再次发生故障的情况。
