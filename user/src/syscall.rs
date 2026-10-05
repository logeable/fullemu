//! 用户程序使用的最小 Linux RISC-V 系统调用封装。

const LINUX_WRITE_SYSCALL: usize = 64;

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
