//! 将 Linux RISC-V 系统调用分派到对应的内核能力模块。

mod abi;
mod clock;
mod context;
mod filesystem;
mod io;
mod process;

use crate::arch::riscv64::trap::TrapFrame;

use abi::{LinuxErrno, *};
use context::SyscallContext;

/// 处理来自 U-mode 的环境调用，并协调 syscall 使用的内核资源。
pub(super) fn handle_user_environment_call(frame: &mut TrapFrame) -> *mut TrapFrame {
    let number = frame.registers[17];
    let arguments = [
        frame.registers[10],
        frame.registers[11],
        frame.registers[12],
        frame.registers[13],
        frame.registers[14],
    ];
    let context = SyscallContext::new(
        crate::process::current_linux_pid() as usize,
        crate::process::current_user_memory_ranges(),
    );

    match number {
        LINUX_READ_SYSCALL => return_to_user(
            frame,
            io::read(context, arguments[0], arguments[1], arguments[2]),
        ),
        LINUX_WRITE_SYSCALL => return_to_user(
            frame,
            io::write(context, arguments[0], arguments[1], arguments[2]),
        ),
        LINUX_CLOCK_GETTIME_SYSCALL => return_to_user(
            frame,
            clock::clock_gettime(context, arguments[0], arguments[1]),
        ),
        LINUX_EXIT_SYSCALL => process::exit(arguments),
        LINUX_SCHED_YIELD_SYSCALL => process::yield_cpu(frame),
        LINUX_CLONE_SYSCALL => process::fork(arguments, frame),
        LINUX_EXECVE_SYSCALL => process::execve(context, arguments, frame),
        LINUX_WAIT4_SYSCALL => process::wait4(context, arguments, frame),
        LINUX_BRK_SYSCALL => process::brk(arguments[0], frame),
        LINUX_OPENAT_SYSCALL => return_to_user(
            frame,
            filesystem::openat(
                context,
                arguments[0],
                arguments[1],
                arguments[2],
                arguments[3],
            ),
        ),
        LINUX_CLOSE_SYSCALL => return_to_user(frame, filesystem::close(arguments[0])),
        LINUX_GETDENTS64_SYSCALL => return_to_user(
            frame,
            filesystem::getdents64(context, arguments[0], arguments[1], arguments[2]),
        ),
        LINUX_MKDIRAT_SYSCALL => return_to_user(
            frame,
            filesystem::mkdirat(context, arguments[0], arguments[1], arguments[2]),
        ),
        LINUX_UNLINKAT_SYSCALL => return_to_user(
            frame,
            filesystem::unlinkat(context, arguments[0], arguments[1], arguments[2]),
        ),
        _ => return_to_user(frame, LinuxErrno::NoSystemCall.return_value()),
    }
}

/// 将结果写入 `a0`、推进 `ecall` 后的指令地址并恢复用户态。
pub(super) fn return_to_user(frame: &mut TrapFrame, result: isize) -> *mut TrapFrame {
    frame.registers[10] = result as usize;
    advance_system_call(frame);
    crate::process::resume_user(frame)
}

/// 跳过已经处理的四字节 `ecall` 指令。
pub(super) fn advance_system_call(frame: &mut TrapFrame) {
    frame.exception_pc += 4;
}
