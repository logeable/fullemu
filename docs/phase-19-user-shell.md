# 第 19 阶段：用户态简易 shell

## 目标关联

项目目标是提供用户可交互的 shell，并逐步运行用户程序。本阶段先把交互入口放在 U-mode：内核启动后运行一个 shell 用户程序，shell 从 UART 读取一行输入，在用户态解析并执行少量内置命令。当前不尝试启动外部程序；本阶段为后续按需创建任务和命令启动程序建立输入、输出和时钟接口。

## 实验问题

**在没有文件系统和外部程序启动能力时，怎样先让用户直接与 U-mode 程序交互？** 内核提供最小 Linux RISC-V `read` 和 `clock_gettime` 子集；命令行读取、回显、退格处理和命令解析都由用户态 shell 完成。

## 实现范围

- 内核按名字从嵌入式程序清单中查找 `fullemu_user_shell`，只装入并启动该程序。其他用户 bin 仍参与构建和嵌入，但当前不会在启动时批量运行。
- 用户态 shell 维护固定 128 字节命令缓冲区，逐字节调用 `read`，回显可打印输入，支持退格、回车和换行，并忽略超过缓冲区容量的字符。
- `help` 显示内置命令；`uptime` 通过 `clock_gettime(CLOCK_MONOTONIC)` 显示从内核计时起点开始经过的秒数。
- `read` 使用 Linux RISC-V syscall 编号 63，只接受 fd 0，并将数据写入当前任务的用户栈缓冲区；首字节通过 UART 轮询阻塞等待，之后读取当前已经到达的字节并允许短读。
- `clock_gettime` 使用 Linux RISC-V syscall 编号 113，只支持 `CLOCK_MONOTONIC`，输出 64 位 `timespec` 到当前任务用户栈。时钟基于 QEMU `virt` 的 10 MHz `time` 计数器，不提供日历时间。
- 任务索引和程序清单索引已分别传入系统调用分发，以便默认任务槽位 0 能运行清单中位置不固定的 shell。

## 暂不实现

- 没有文件系统，因此当前不提供目录列表命令。
- 没有外部程序启动、参数、管道、重定向、历史记录、完整行编辑或环境变量。
- `read` 在内核中轮询 UART；等待输入期间内核停留在 S-mode，当前没有其他用户任务需要运行。未来加入后台任务前，需要把输入等待改为可阻塞和唤醒的任务状态，不能让 UART 轮询占住内核。
- `read` 和 `clock_gettime` 仅接受当前任务栈中的输出缓冲区；由于仍使用 `satp=BARE`，这不是一般的用户指针安全验证。
- `clock_gettime` 只支持 `CLOCK_MONOTONIC`，不会因该单一调用而声称完整支持 Linux 时间 ABI。
- 命令缓冲区固定为 128 字节；超长输入会被截断。shell 当前只识别 ASCII 命令名称。

## Linux ABI 依据

RISC-V syscall 编号 `read=63`、`clock_gettime=113` 来自 Linux 通用 UAPI syscall 表；`CLOCK_MONOTONIC=1` 来自 Linux UAPI 时间定义：[syscall 表](https://github.com/torvalds/linux/blob/master/include/uapi/asm-generic/unistd.h)、[时间定义](https://github.com/torvalds/linux/blob/master/include/uapi/linux/time.h)。本阶段只实现文档所列的窄子集，不能据此推断支持完整 Linux ABI。

## 运行与验收

```sh
make fmt-check
make build
make run
```

启动后应直接看到 `fullemu>` 提示符。在 QEMU 串口输入 `help` 和 `uptime` 并按回车，shell 应回显输入、输出对应结果并再次显示提示符。退格键应删除前一个输入字节；未知命令应报告错误后继续接受输入。内核启动日志应显示只装入默认 shell，不再自动执行计算、协作、CPU 密集和异常示例程序。

## 后续连接

当前 shell 证明用户态程序能接收输入并解释命令，但无法启动其他程序。下一步应围绕“输入一个程序名并在运行时启动它”建立任务创建、退出回收和等待语义；初期仍可从嵌入式程序清单选择程序，无需先实现文件系统。
