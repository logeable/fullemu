# 第 49 阶段：嵌入式 RV64 ELF 装载

## 目标关联

shell 已能通过 `fork`、`exec` 和 `wait` 启动独立程序，但旧装载器只复制原始平铺镜像，并把整个程序区域设成可读、可写、可执行。真实 Linux 用户程序以 ELF 描述入口、装载段、零初始化内存和访问权限。本阶段替换平铺装载，让后续文件系统可以直接提供 ELF 文件给同一装载路径。

## 学习重点

ELF 文件描述程序如何映射到进程地址空间。本项目自行解析 ELF64 little-endian RISC-V 程序头，按 `PT_LOAD` 分配用户页帧、复制文件字节、依靠清零页提供 BSS，并根据 `PF_R`、`PF_W`、`PF_X` 建立 Sv39 页权限。

## 实现范围

- `make build-user` 保留用户 ELF 文件；内核构建期嵌入 ELF，而不是先用 `objcopy` 转成原始镜像。
- 解析 `ET_EXEC` 和 `PT_LOAD`，检查 ELF 类别、字节序、机器类型、头部范围、段范围、段权限、段对齐及入口所在的可执行段。
- 仅接受位于 `0x40000000` 及以上的低半部用户地址；装载段必须连续覆盖页范围且不能共享虚拟页，以便当前 syscall 用户缓冲区校验继续使用一段连续程序范围。
- 当前只接受一个连续的可写段范围；syscall 向用户写数据时只允许该范围或用户栈，读程序数据仍可访问任一装载段。
- 拒绝 W+X 段。链接脚本把 `.text`、`.rodata` 和 `.data/.bss` 放在不同页，生成 `UR-X`、`UR--`、`URW-` 用户映射。
- fork 为子进程复制已装载页和页权限；exec 完整准备新 ELF 映像后再替换旧映像，保持现有资源回收顺序。
- 不支持 PIE/`ET_DYN`、解释器、动态链接、重定位、文件系统加载、参数和环境变量。

## 验收方式

```sh
make fmt-check
make build
make run
```

在 QEMU shell 中运行 `compute`、`fork_wait`、`exec_test`、`elf_readonly_fault` 和 `memory_fault`。程序应从 ELF 入口运行，fork/exec/wait 行为保持正确；`fork_wait` 能修改 RW 数据段，`elf_readonly_fault` 能读取但不能写入 R 数据段，并以 `StoreAmoPageFault` 结束；用户栈仍为 `URW-`。`memory_fault` 对不可访问地址的测试仍应触发用户页故障。带 `PT_INTERP` 或不符合本阶段段布局约束的 ELF 应在 exec 时以 `ENOEXEC` 失败，原进程继续运行。

## 后续连接

ELF 装载器现在只依赖输入字节和用户地址空间，不依赖镜像来自嵌入清单还是文件系统。后续可以先完善 `execve` 的参数与环境变量栈，再逐步建设文件描述符、存储设备和文件系统；届时用文件内容替换嵌入字节来源，不必重写段装载机制。
