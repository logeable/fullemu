# 第 55 阶段：基础用户态文件系统调用

## 目标关联

第 54 阶段已让内核通过块设备格式化、挂载并操作持久文件系统。本阶段把能力沿现有分层交给用户程序：文件系统仍负责 inode 与磁盘格式，进程拥有文件描述符状态，syscall 负责 Linux RISC-V ABI、用户指针校验和错误码，用户命令只组合这些调用。

完成后可以从 shell 创建目录和空文件、列出目录、删除普通文件，并由其他用户程序通过已有 `read`/`write` 系统调用访问文件内容。

## 模块职责

- `filesystem` 持有唯一挂载卷，通过 `BlockDevice` 执行扇区 I/O；提供根目录相对路径解析、目录项枚举、普通文件创建/删除和按偏移读写，不解析 syscall 编号或用户指针。
- `Process` 拥有自己的文件描述符表。描述符 `0`、`1`、`2` 保留给标准输入、标准输出、标准错误；新文件描述符从 `3` 开始。`fork` 复制描述符及当前位置，当前实现让父子位置独立变化；`exec` 保留描述符，进程退出时清空表。
- `src/syscall/mod.rs` 只负责编号分派、创建调用上下文和返回用户态；`syscall::filesystem` 负责文件操作 ABI，`syscall::io` 负责 `read`/`write` 在控制台和文件间的分派，`syscall::abi` 维护共享编号、标志及 errno，`syscall::context` 检查用户内存范围。对文件描述符的定位由进程模块提供，不直接访问全局进程表。
- `user::syscall` 提供无堆的调用封装，路径放入用户栈上的固定缓冲区；`ls`、`mkdir`、`touch`、`rm` 是普通用户程序，由 shell 按程序名启动。

块设备 I/O 发生在用户 syscall 陷入期间，当前活跃页表是调用进程的地址空间。启动阶段因此把 FDT 中发现的 VirtIO MMIO 范围登记给 `memory::address_space`，每个进程页表都会以 supervisor-only 权限恒等映射这些设备页；用户态不能直接访问它们。

## 实现的 syscall 子集

系统调用号采用 Linux RISC-V 通用 ABI：

| 调用 | 编号 | 当前行为 |
| --- | ---: | --- |
| `mkdirat` | 34 | 只接受 `AT_FDCWD`，在根目录相对路径创建目录；`mode` 被接收但文件系统不保存权限 |
| `unlinkat` | 35 | 只接受 `AT_FDCWD` 和零标志；删除普通文件，不删除目录 |
| `openat` | 56 | 只接受 `AT_FDCWD`；支持只读、只写、读写、`O_CREAT`、`O_EXCL`、`O_TRUNC`、`O_APPEND`、`O_DIRECTORY` |
| `close` | 57 | 关闭描述符 `3` 及以上的文件/目录描述符 |
| `getdents64` | 61 | 按 Linux `dirent64` 布局返回目录项，目录描述符位置记录目录槽位 |
| `read` | 63 | 描述符 `0` 从 UART 读；普通文件描述符按当前位置读并推进位置 |
| `write` | 64 | 描述符 `1`、`2` 写 UART；可写文件描述符写入文件并推进位置 |

路径字节必须是 UTF-8，NUL 结尾的最大长度为 255 字节。相对路径统一从文件系统根目录解析；不支持当前工作目录、`dirfd` 相对路径、`.`、`..`、符号链接和挂载点。文件名仍受磁盘格式限制，最多 27 字节。单文件最多 7168 字节，最多 128 个 inode。`rm` 目前只删除普通文件，空目录也不能删除。

为避免 inode 被删除后立即复用、导致现存文件描述符指向另一文件，`unlinkat` 在任一进程仍打开目标 inode 时返回 `EBUSY`。这保证本阶段对象引用有效，但行为不同于 Linux：Linux 会先移除目录链接，等最后一个文件描述符关闭后再释放 inode。

这些调用使用 Linux 规定的系统调用编号、寄存器和目录记录布局，但只实现上表范围，不表示完整 Linux 文件系统 ABI 兼容。当前没有权限位、`lseek`、`fstat`、`dup`、close-on-exec、块缓存或异常断电一致性。`fork` 后父子文件偏移独立，和 Linux 共享 open-file-description 的语义不同。

## 用户程序

- `ls [目录...]`：不带参数时列出根目录；显式目录必须存在。
- `mkdir <目录>...`：创建一个或多个目录。
- `touch <文件>...`：创建不存在的空文件；文件已存在时不截断内容。
- `rm <文件>...`：删除普通文件。

程序没有额外依赖，不使用堆。文件命令通过既有 shell 的 `fork`、`exec`、`wait` 路径前台运行。文件系统 raw 镜像由 `scripts/run-qemu.sh` 复用；首次启动时只格式化空白镜像。

## 手动验收

```text
fullemu> help
fullemu> mkdir demo
fullemu> touch demo/note
fullemu> ls
fullemu> ls demo
fullemu> rm demo/note
fullemu> ls demo
```

预期根目录列表包含 `demo`，`ls demo` 在删除前包含 `note`、删除后为空。退出 QEMU 后重新执行 `make run`，再次运行 `ls` 应仍能看到 `demo`，用于确认正常关机后的磁盘持久化。

编译检查：

```sh
make build
```

本阶段不引入外部 crate。文件系统仍以轮询方式访问单个 VirtIO 块设备；块设备和文件系统操作发生在 syscall 陷入期间。
