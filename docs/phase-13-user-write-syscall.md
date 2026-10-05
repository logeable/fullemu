# 第 13 阶段：Linux RISC-V `write` 系统调用

## 目标关联

独立用户程序已经可以被内核加载并进入 U-mode，但它没有途径请求内核服务。shell、诊断工具和普通用户程序都需要输出能力。本阶段实现第一个系统调用，让用户程序通过受控陷入请求内核把文本写到控制台。

## ABI 约定

本阶段遵循 Linux RISC-V 的基本 syscall 调用约定：用户程序把 syscall 编号放入 `a7`，参数依次放入 `a0` 至 `a5`，执行 `ecall`；返回值放在 `a0`。`write` 使用 Linux syscall 编号 64，参数是文件描述符、缓冲区地址和字节数。Linux syscall 表通过 asm-generic 定义 `__NR_write` 为 64；RISC-V 陷入入口将 U-mode `ecall` 分发为系统调用。[Linux RISC-V syscall 编号](https://github.com/torvalds/linux/blob/master/tools/include/uapi/asm-generic/unistd.h)、[Linux RISC-V 陷入入口](https://github.com/torvalds/linux/blob/master/arch/riscv/kernel/entry.S)。

错误结果按 Linux 约定以负 errno 返回：本阶段支持 `EBADF`（9）、`EFAULT`（14）和 `ENOSYS`（38）。当前实现只复刻这一小段 ABI，不意味着已经兼容 Linux 用户态。

## 实现范围

- 用户态 `write` 封装把 fd、缓冲区、长度分别放入 `a0`、`a1`、`a2`，把编号 64 放入 `a7`，再执行 `ecall`。
- `supervisor_trap_handler` 识别 U-mode 环境调用，调用内核 syscall 分发器，将返回值写回保存帧的 `a0`，并把 `sepc` 前移 4 字节后恢复原用户上下文。
- 在恢复 U-mode 前，处理器重新把当前内核陷入栈顶写入 `sscratch`，确保下一次 U-mode 陷入仍能切换到 S-mode 栈。
- `write` 仅接受标准输出 fd 1 和标准错误 fd 2，均通过 QEMU `virt` 的 UART0 输出。其他 fd 返回 `-EBADF`；未知 syscall 返回 `-ENOSYS`。
- 由于尚无 Sv39，缓冲区检查仅接受完全落在当前已加载用户程序镜像内的地址；越界或溢出返回 `-EFAULT`。长度为零时返回 0。
- 当前串口写入会把换行符转换为终端常用的 CRLF；成功时 syscall 返回原请求字节数。轮询 UART 暂不支持部分写入或设备错误恢复。

## 暂不实现

- 没有通用文件描述符表、文件系统、`read`、参数超过 3 个的 syscall、信号或进程调度。
- 只有当前固定加载区内的用户缓冲区可用于 `write`；这不是通用用户指针验证。启用 Sv39 后应按当前地址空间检查和复制用户数据。
- U-mode 仍处于 `satp=BARE`，用户程序仍能直接读写 PMP 允许的内核 RAM。
- 该实现匹配 Linux RISC-V 的 syscall 编号、寄存器位置和基本返回约定，不支持完整 Linux RISC-V ABI。

## 运行与验收

```sh
make fmt-check
make build
make run
```

串口应先出现：

```text
Hello from an independent U-mode program via write syscall!
```

阶段 13 初始验收通过在 `write` 返回后触发非法指令，证明 `ecall` 回到原用户上下文继续执行。当前集成程序在 `write` 返回后调用阶段 14 的 `exit` 结束；探针已从当前实现移除。本阶段只记录输出系统调用的接口和行为。

## 后续连接

多个系统调用将共同构成用户态运行时接口。阶段 14 已实现 `exit`；下一步可建立 Sv39 地址空间和受限用户映射，通过真实的页权限保护用户缓冲区，再扩展输入和文件描述符，让 shell 作为用户程序运行。
