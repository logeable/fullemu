# 第 48 阶段：shell 前台执行嵌入式程序

## 目标关联

用户态 `init` 已能启动并监督 shell，内核也已经提供 `fork`、受限 `execve` 和 `wait4`。本阶段把这些机制接到 shell 命令行，让用户可以输入程序名运行一个独立用户进程，并在进程退出后返回 shell。

## 学习重点

shell 不直接替换自身映像，而是先 `fork` 出子进程，再由子进程 `execve` 替换为目标程序；shell 父进程通过 `wait4` 阻塞等待子进程退出。由此可以从一次命令输入追踪到进程创建、程序映像替换、退出和资源回收的完整路径。

## 实现范围

- `help`、`uptime`、`exit` 仍是 shell 内置命令。
- 对其他单个命令词，shell 构造 `/bin/<程序名>`，`fork` 子进程；子进程调用 `exec`，父进程等待该子进程退出后恢复提示符。
- 程序名限 ASCII 字母、数字、下划线和连字符。程序仍从构建时嵌入的镜像清单查找，不访问文件系统。
- `exec` 成功时从目标程序入口开始运行；失败时 shell 子进程报告 errno 并以状态 127 退出，父 shell 继续运行。
- 带参数的命令明确报告暂不支持。当前 `execve` 只接受空 `argv` 和 `envp`，本阶段不增加参数栈构造。
- shell 等待具体子进程 PID，不会把 PID 1 的其他子进程退出误认为当前命令已完成。PID 1 继续负责收集这些进程的状态。

## 暂不实现

- 文件系统路径、ELF 加载、命令参数、环境变量、管道、重定向和后台运行。
- shell 内置命令的参数解析及完整错误码到用户提示的翻译。
- 非 ASCII 命令名和通配符展开。

## 验收方式

```sh
make fmt-check
make build
(sleep 1; printf 'compute\nhelp\n') | timeout 5s ./scripts/run-qemu.sh target/riscv64gc-unknown-none-elf/release/fullemu
```

QEMU 输出应显示 `compute` 在独立子进程中运行并结束，shell 报告 wait 状态后重新显示提示符；随后输入的 `help` 仍由原 shell 处理。再输入 `missing_program`，应看到 exec 失败并报告 errno，shell 仍能继续接收命令。输入 `compute arg` 应报告参数暂不支持，不应创建子进程。

## 后续连接

本阶段完成了从交互命令到进程生命周期的最小纵向路径，但程序来源仍是静态嵌入清单且不能传参。后续可先为 `execve` 建立参数和环境变量栈布局，再结合持久化阶段实现文件系统路径加载；每一步都应保持“输入命令—创建进程—装入映像—回收退出进程”这条可观察路径。
