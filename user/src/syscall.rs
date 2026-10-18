//! 用户程序使用的最小 Linux RISC-V 系统调用封装。

const LINUX_WRITE_SYSCALL: usize = 64;
const LINUX_READ_SYSCALL: usize = 63;
const LINUX_CLOCK_GETTIME_SYSCALL: usize = 113;
const LINUX_EXIT_SYSCALL: usize = 93;
const LINUX_CLONE_SYSCALL: usize = 220;
const LINUX_EXECVE_SYSCALL: usize = 221;
const LINUX_WAIT4_SYSCALL: usize = 260;
const LINUX_SIGCHLD: usize = 17;
const CLOCK_MONOTONIC: usize = 1;
const EXEC_PATH_BUFFER_SIZE: usize = 128;
const LINUX_ENAMETOOLONG: isize = 36;

/// Linux RISC-V 64 位 ABI 的单调时间结构。
#[repr(C)]
pub struct Timespec {
    /// 完整秒数。
    pub seconds: i64,
    /// 不足一秒的纳秒数。
    pub nanoseconds: i64,
}

/// 从指定文件描述符读取字节，并返回读取字节数或负的 Linux errno。
pub fn read(file_descriptor: usize, bytes: &mut [u8]) -> isize {
    let mut result = file_descriptor as isize;

    // 安全性：调用约定遵循 Linux RISC-V syscall ABI；内核只允许写入当前任务栈缓冲区。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") bytes.as_mut_ptr() as usize,
            in("a2") bytes.len(),
            in("a7") LINUX_READ_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 请求内核把字节写到指定文件描述符，并返回写入字节数或负的 Linux errno。
pub fn write(file_descriptor: usize, bytes: &[u8]) -> isize {
    let mut result = file_descriptor as isize;

    // 安全性：寄存器和调用指令遵循 Linux RISC-V syscall ABI；内核负责检查缓冲区并返回结果。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") bytes.as_ptr() as usize,
            in("a2") bytes.len(),
            in("a7") LINUX_WRITE_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 读取 `CLOCK_MONOTONIC`，结果表示从内核启动计时起点经过的时间。
pub fn clock_gettime(time: &mut Timespec) -> isize {
    let mut result = CLOCK_MONOTONIC as isize;

    // 安全性：时间结构位于当前用户栈；调用约定遵循 Linux RISC-V syscall ABI。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") time as *mut Timespec as usize,
            in("a7") LINUX_CLOCK_GETTIME_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 请求内核以给定状态码结束当前用户任务；符合 Linux ABI 时不会返回。
pub fn exit(status: i32) -> ! {
    // 安全性：状态码按 Linux `int` 参数传入 `a0`，系统调用号 93 放入 `a7`。
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a0") status as isize,
            in("a7") LINUX_EXIT_SYSCALL,
            options(nostack)
        );
    }

    // 若内核错误地让 exit 返回，用户代码也不能继续越过不可返回的 ABI 边界。
    loop {
        core::hint::spin_loop();
    }
}

/// 主动让出 CPU，并在再次获得调度时从系统调用之后继续执行。
// 多个独立二进制共享此封装，只有需要协作切换的程序会调用它。
#[allow(dead_code)]
pub fn sched_yield() -> isize {
    let mut result = 0isize;
    let syscall_number = 124;

    // 安全性：寄存器和调用指令遵循 Linux RISC-V syscall ABI；内核负责选择下一个就绪任务。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a7") syscall_number,
            options(nostack)
        );
    }

    result
}

/// 复制当前进程；父进程得到子进程 PID，子进程得到 0，失败时返回负的 Linux errno。
pub fn fork() -> isize {
    let mut result = LINUX_SIGCHLD as isize;

    // 安全性：只请求带 SIGCHLD 退出通知的独立进程；其余 clone 参数按 fork 约定传零。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") 0usize,
            in("a2") 0usize,
            in("a3") 0usize,
            in("a4") 0usize,
            in("a7") LINUX_CLONE_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 替换当前进程为 `/bin/<程序名>` 中的嵌入式程序；成功后不返回，失败返回负的 Linux errno。
pub fn exec(program_path: &str) -> isize {
    if program_path.len() >= EXEC_PATH_BUFFER_SIZE {
        return -LINUX_ENAMETOOLONG;
    }
    let mut path = [0u8; EXEC_PATH_BUFFER_SIZE];
    path[..program_path.len()].copy_from_slice(program_path.as_bytes());

    let mut result = path.as_mut_ptr() as isize;
    // 安全性：路径缓冲区在 ecall 期间位于当前用户栈且以 NUL 结尾；本封装只传空 argv 和 envp。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") 0usize,
            in("a2") 0usize,
            in("a7") LINUX_EXECVE_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 等待指定子进程或任意子进程，阻塞直到其退出并可选地取得 wait 状态。
pub fn waitpid(pid: i32, status: Option<&mut i32>) -> isize {
    let mut result = pid as isize;
    let status_address = status.map_or(0, |status| status as *mut i32 as usize);

    // 安全性：状态指针在调用期间指向当前用户栈中的可写 `i32`，其他参数符合 Linux wait4 ABI。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") status_address,
            in("a2") 0usize,
            in("a3") 0usize,
            in("a7") LINUX_WAIT4_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 等待任意一个子进程；这是 `waitpid(-1, status)` 的便捷封装。
pub fn wait(status: Option<&mut i32>) -> isize {
    waitpid(-1, status)
}
