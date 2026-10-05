//! 用户程序使用的最小 Linux RISC-V 系统调用封装。

const LINUX_WRITE_SYSCALL: usize = 64;
const LINUX_EXIT_SYSCALL: usize = 93;

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
