# 第 45 阶段：fork 式进程创建与 wait

## 目标关联

进程对象现在拥有地址空间、用户镜像、用户栈、内核栈和退出状态。本阶段让用户程序能从已有进程创建独立子进程，并在父进程中回收子进程退出状态。这是内核进程管理的第一条运行时创建路径，为之后的 `exec` 和 shell 动态启动程序打基础。

本阶段围绕一个问题展开：**父进程怎样复制出独立运行的子进程，并可靠地等待和回收它？**

## Linux RISC-V ABI 边界

RISC-V Linux 没有独立的 `fork` syscall 编号。用户态 `fork()` 以 fork 语义调用 `clone`（编号 220），普通子进程等待通过 `wait4`（编号 260）完成。本实现只接受 `clone(SIGCHLD, 0, 0, 0, 0)`，不支持线程共享标志或其他 clone 选项；用户库提供 `fork()` 封装，直接发起该受限 ABI 调用。

`wait4` 当前支持指定正 PID 或任意子进程（`pid = -1`）、`options = 0` 或 `WNOHANG`，以及空 `rusage`。wait 状态采用 Linux 编码：正常退出码放在高 8 位，异常终止信号放在低位。非法参数返回 `EINVAL`，无可等待子进程返回 `ECHILD`，用户指针无效返回 `EFAULT`，资源分配失败返回 `ENOMEM`，PID 耗尽返回 `EAGAIN`。

## 实现范围

- fork 复制用户程序镜像和用户栈到新页帧，复制父进程陷入现场并建立独立地址空间。
- 父进程从 syscall 返回子 PID，子进程从相同调用点返回 `0`；两者都跳过 `ecall` 指令。
- 新进程对象只有在镜像、两种栈、页表和初始陷入帧准备完成后才加入进程表；失败时局部资源所有者回收已取得资源。
- `wait4` 对未退出子进程阻塞当前父进程；子进程退出时写入状态、唤醒父进程并交付子 PID。
- 已退出子进程的资源在切换到安全的地址空间和内核栈后回收；父进程通过 wait 收集状态后，其追加式记录转为 `Reaped`。
- 用户示例验证父子返回值、用户可写数据隔离、阻塞等待、退出状态编码和资源回收。

## 对象关系与生命周期

```text
父进程（Running / Waiting）
  └─ 通过 parent_id 关联子进程
       ├─ Ready / Running：仍持有完整 ProcessResources
       ├─ Exited：退出状态已保存，运行资源等待安全回收
       └─ Reaped：资源已回收，父进程已收集状态或父进程已退出
```

进程记录使用追加式 `Vec<Process>`，PID 由记录位置派生且本阶段不复用。用户页不是共享的：镜像和用户栈均复制到子进程独占的页帧，不实现写时复制。进程状态、父子关系和等待请求由 `kernel::process` 管理；syscall 模块只解析 ABI 参数并产生已校验的 `WaitRequest`。

## 本阶段不实现

- 不实现 `execve`、shell `run` 命令、后台作业或文件系统。
- 不实现 `clone` 的线程共享、TLS、tid 指针、进程组等待、信号处理或写时复制。
- 不实现 PID 复用、多 hart 并发或通用僵尸进程策略。
- 不把受限的 `clone`/`wait4` 支持描述为完整 Linux 进程 ABI 兼容。

## 验收方式

```sh
make fmt-check
make build BOOT_PROGRAM=fork_wait BOOT_MODE=shell LOG_LEVEL=info
make run BOOT_PROGRAM=fork_wait BOOT_MODE=shell LOG_LEVEL=info
make run BOOT_MODE=batch LOG_LEVEL=info
```

shell 模式中的 `fork_wait` 程序应打印父子分支、成功等待及私有数据保持原值的信息；退出日志应显示子进程资源被回收。批处理模式应继续运行构建清单中的程序，允许程序动态追加子进程。

## 后续连接

下一步可以完善 syscall 边界和进程管理行为，例如验证子进程异常退出时的 wait 状态，以及父进程退出后的孤儿子进程生命周期；之后再设计单独的小阶段实现 `execve` 和 shell 前台程序启动。
