# 第 50 阶段：execve 参数传递

## 目标关联

ELF 装载器已经能把程序映像放入新进程，但此前 `execve` 只接受空 `argv`。这使 shell 只能启动固定行为的程序。本阶段让调用者向新映像传递命令行参数，打通“shell 命令—execve—新用户栈—程序入口”的路径。

## 实现范围

- syscall 层逐项验证 `argv` 指针和以 NUL 结尾的字符串，只从当前进程可读的程序映像或用户栈复制数据。
- 最多接受 32 个参数，总字符串数据最多 4096 字节；超限返回 `E2BIG`，无效用户地址返回 `EFAULT`，内核分配失败返回 `ENOMEM`。
- 新参数先复制到内核拥有的缓冲区，再由进程 exec 路径写入新用户栈。只有新 ELF、参数栈和地址空间准备成功后才替换旧映像；失败时原进程继续运行。
- 新用户栈按 16 字节边界对齐，包含 `argc`、`argv` 指针和字符串、空 `envp`，以及 `AT_NULL` 辅助向量结束项。入口汇编把初始栈指针交给用户运行时；所有用户程序统一定义 `main(UserArguments)`，不使用参数的程序忽略该值，参数迭代器按字节读取，不要求 UTF-8。
- 用户库的 `exec(program_name, arguments)` 将程序名用作 `argv[0]`，其余字符串依次放入 `argv`。内核直接按程序清单名称查找，不合成 `/bin/` 伪路径；非空 `envp` 仍返回 `EINVAL`。
- shell 用空白字符拆分命令并传递参数；当前不处理引号、转义、管道或重定向。
- `argv_test` 检查 argv[0]、普通参数及包含空格的单个参数；`exec_test` 通过 fork/exec/wait 验证参数传递失败或成功时的进程行为。

## 主要边界

`kernel::syscall` 负责解读并验证旧进程的 Linux `execve` 参数，将内容复制为内核数据；`process::exec` 负责构造新进程栈并执行映像替换；`process::stack::UserStack` 负责把初始栈数据写入自身拥有的页帧。用户库只提供启动栈参数迭代器和 syscall 封装，不参与内核内存管理。

## 验收方式

```sh
make fmt-check
make build
make run BOOT_PROGRAM=exec_test
```

`exec_test` 应看到 `argv_test` 收到 `argv_test`、`alpha` 和 `two words`，并以预期状态退出。以默认 `init` 启动后，也可在 shell 输入 `argv_test alpha beta`，确认参数能到达新程序。`execve` 参数无效或目标程序不存在时，原进程应继续运行。

## 后续连接

下一步可以单独支持 `envp`，并扩展初始辅助向量；在此之前不接入动态链接器或文件系统。shell 可在后续阶段增加引号和转义解析，但应保持命令解析留在用户态。
