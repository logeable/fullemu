//! 实现当前内核支持的最小 Linux RISC-V 系统调用集合。

use crate::arch::riscv64::console;

const LINUX_WRITE_SYSCALL: usize = 64;
const LINUX_EXIT_SYSCALL: usize = 93;
const STDOUT_FILE_DESCRIPTOR: usize = 1;
const STDERR_FILE_DESCRIPTOR: usize = 2;

#[derive(Clone, Copy)]
#[repr(isize)]
enum LinuxErrno {
    BadFileDescriptor = 9,
    Fault = 14,
    NoSystemCall = 38,
}

impl LinuxErrno {
    fn return_value(self) -> isize {
        -(self as isize)
    }
}

/// 表示系统调用返回用户态，或结束当前用户任务。
pub enum SyscallOutcome {
    /// 将结果写入 `a0` 后恢复用户上下文。
    Return(isize),
    /// 当前任务调用 `exit`；Linux 对外可观察的正常退出码只有低 8 位。
    Exit(u8),
}

/// 按 Linux RISC-V syscall ABI 分发调用，并描述返回或任务退出结果。
pub fn dispatch(number: usize, arguments: [usize; 3]) -> SyscallOutcome {
    match number {
        LINUX_WRITE_SYSCALL => {
            SyscallOutcome::Return(write(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_EXIT_SYSCALL => SyscallOutcome::Exit(arguments[0] as i32 as u8),
        _ => SyscallOutcome::Return(LinuxErrno::NoSystemCall.return_value()),
    }
}

fn write(file_descriptor: usize, buffer_address: usize, count: usize) -> isize {
    if file_descriptor != STDOUT_FILE_DESCRIPTOR && file_descriptor != STDERR_FILE_DESCRIPTOR {
        return LinuxErrno::BadFileDescriptor.return_value();
    }

    if count == 0 {
        return 0;
    }

    if !super::user_program::contains_buffer_range(buffer_address, count) {
        return LinuxErrno::Fault.return_value();
    }

    // 安全性：范围检查保证每个待读字节位于已加载的用户镜像内；单 hart 实验中用户程序在陷入期间暂停。
    for offset in 0..count {
        let byte = unsafe { core::ptr::read_volatile((buffer_address as *const u8).add(offset)) };
        console::write_byte(byte);
    }

    // UART 轮询写入要么完成整个缓冲区，要么设备永久不就绪；当前阶段不报告部分写入。
    count as isize
}
