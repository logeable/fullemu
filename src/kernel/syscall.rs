//! 实现当前内核支持的最小 Linux RISC-V 系统调用集合。

use crate::arch::riscv64::console;

const LINUX_WRITE_SYSCALL: usize = 64;
const LINUX_EXIT_SYSCALL: usize = 93;
const LINUX_SCHED_YIELD_SYSCALL: usize = 124;
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
    /// 当前任务主动让出 CPU，调度器运行另一个就绪任务。
    Yield,
}

/// 按 Linux RISC-V syscall ABI 分发调用，并描述返回或任务退出结果。
pub fn dispatch(program_index: usize, number: usize, arguments: [usize; 3]) -> SyscallOutcome {
    match number {
        LINUX_WRITE_SYSCALL => SyscallOutcome::Return(write(
            program_index,
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        LINUX_EXIT_SYSCALL => SyscallOutcome::Exit(arguments[0] as i32 as u8),
        LINUX_SCHED_YIELD_SYSCALL => SyscallOutcome::Yield,
        _ => SyscallOutcome::Return(LinuxErrno::NoSystemCall.return_value()),
    }
}

fn write(
    program_index: usize,
    file_descriptor: usize,
    buffer_address: usize,
    count: usize,
) -> isize {
    if file_descriptor != STDOUT_FILE_DESCRIPTOR && file_descriptor != STDERR_FILE_DESCRIPTOR {
        return LinuxErrno::BadFileDescriptor.return_value();
    }

    if count == 0 {
        return 0;
    }

    let buffer_is_in_program =
        super::user_program::contains_buffer_range(program_index, buffer_address, count);
    let buffer_is_in_stack =
        super::user_mode::contains_user_stack_range(program_index, buffer_address, count);
    if !buffer_is_in_program && !buffer_is_in_stack {
        crate::klog_trace!(
            "拒绝任务 {} 的 write 缓冲区：地址={:#018x}，长度={}",
            program_index,
            buffer_address,
            count
        );
        return LinuxErrno::Fault.return_value();
    }

    // 安全性：范围检查保证待读字节位于当前任务的镜像或独立用户栈内；系统调用期间任务暂停。
    for offset in 0..count {
        let byte = unsafe { core::ptr::read_volatile((buffer_address as *const u8).add(offset)) };
        console::write_byte(byte);
    }

    // UART 轮询写入要么完成整个缓冲区，要么设备永久不就绪；当前阶段不报告部分写入。
    count as isize
}
