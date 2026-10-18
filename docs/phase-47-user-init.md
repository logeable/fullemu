# 第 47 阶段：用户态 init 启动并监督 shell

## 目标关联

此前内核直接把 shell 作为第一个用户进程启动。现在已有 `fork`、`execve` 和 `wait4` 的最小实现，可以把“启动哪个服务、服务退出后怎么办”的策略移到用户态。本阶段让第一个进程成为 PID 1 的 `init`，由它创建并监督 shell。

## 学习重点

内核提供创建进程、替换进程映像、等待子进程和报告退出状态的机制；用户态 init 使用这些机制组织系统启动策略。shell 是 init 的子进程，init 不需要内核为 shell 添加特殊处理。

## 实现范围

- 默认启动程序改为 `init`。第一个进程仍由内核创建，因此获得 PID 1。
- init 启动 shell 子进程后通过 `waitpid(-1)` 等待任意子进程；收到 shell 的退出状态后重新启动 shell，收到其他子进程状态时回收它并继续等待。
- 非 PID 1 进程退出时，内核将它仍拥有的子进程重新托管给 PID 1；PID 1 退出时，子进程解除父子关系并由内核在退出后自动回收。
- shell 新增 `exit` 内置命令，用于正常结束当前 shell 并观察 init 的重启行为。
- `fork` 或 `waitpid` 失败时，init 输出错误并以非零状态退出，避免无延迟的忙循环。
- 内核入口从镜像清单中选择单个程序；其他用户程序由 init、shell 或用户程序按需创建，不再由内核批量启动。
- 内核不识别 init 的身份，也不加入 shell 重启专用 syscall 或进程策略。

## 暂不实现

- 不支持传递命令行参数或环境变量；当前 `exec` 要求空 `argv/envp`。
- 不实现 `getpid`、信号、subreaper、关机策略或服务依赖管理。

## 验收方式

```sh
make fmt-check
make build
(sleep 1; printf 'help\n') | timeout 5s ./scripts/run-qemu.sh target/riscv64gc-unknown-none-elf/release/fullemu
make build BOOT_PROGRAM=orphan_test
timeout 5s ./scripts/run-qemu.sh target/riscv64gc-unknown-none-elf/release/fullemu
```

应看到 init 启动 shell，shell 打印欢迎信息和提示符，接收 `help` 并列出内置命令。shell 持续等待输入，因此 QEMU 由 timeout 结束属于预期。

在提示符输入 `exit`，应看到 init 报告 shell 退出并再次打印 shell 欢迎信息。`orphan_test` 以 PID 1 启动一个中间子进程和孙进程；中间进程先退出后，PID 1 应分别 wait 回收该子进程及重新托管的孤儿孙进程。

用户程序清单仍包含可供 `exec` 查找的嵌入式程序；只有内核启动入口是单个程序。shell 可按需运行命令，程序退出后由其父进程等待并回收。

## 后续连接

init 以用户态策略管理 shell。shell 的前台程序启动见[第 48 阶段](phase-48-shell-program-execution.md)。
