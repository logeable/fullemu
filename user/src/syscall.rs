//! 用户程序使用的最小 Linux RISC-V 系统调用封装。

const LINUX_WRITE_SYSCALL: usize = 64;
const LINUX_READ_SYSCALL: usize = 63;
const LINUX_CLOCK_GETTIME_SYSCALL: usize = 113;
const LINUX_EXIT_SYSCALL: usize = 93;
const CLOCK_MONOTONIC: usize = 1;

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
