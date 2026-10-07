//! 实现当前内核支持的最小 Linux RISC-V 系统调用集合。

use crate::arch::riscv64::console;

const LINUX_WRITE_SYSCALL: usize = 64;
const LINUX_READ_SYSCALL: usize = 63;
const LINUX_CLOCK_GETTIME_SYSCALL: usize = 113;
const LINUX_EXIT_SYSCALL: usize = 93;
const LINUX_SCHED_YIELD_SYSCALL: usize = 124;
const STDIN_FILE_DESCRIPTOR: usize = 0;
const STDOUT_FILE_DESCRIPTOR: usize = 1;
const STDERR_FILE_DESCRIPTOR: usize = 2;
const CLOCK_MONOTONIC: usize = 1;

#[derive(Clone, Copy)]
#[repr(isize)]
enum LinuxErrno {
    BadFileDescriptor = 9,
    InvalidArgument = 22,
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
pub fn dispatch(
    task_index: usize,
    program_index: usize,
    number: usize,
    arguments: [usize; 3],
) -> SyscallOutcome {
    match number {
        LINUX_READ_SYSCALL => {
            SyscallOutcome::Return(read(task_index, arguments[0], arguments[1], arguments[2]))
        }
        LINUX_WRITE_SYSCALL => SyscallOutcome::Return(write(
            task_index,
            program_index,
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        LINUX_CLOCK_GETTIME_SYSCALL => {
            SyscallOutcome::Return(clock_gettime(task_index, arguments[0], arguments[1]))
        }
        LINUX_EXIT_SYSCALL => SyscallOutcome::Exit(arguments[0] as i32 as u8),
        LINUX_SCHED_YIELD_SYSCALL => SyscallOutcome::Yield,
        _ => SyscallOutcome::Return(LinuxErrno::NoSystemCall.return_value()),
    }
}

fn read(task_index: usize, file_descriptor: usize, buffer_address: usize, count: usize) -> isize {
    if file_descriptor != STDIN_FILE_DESCRIPTOR {
        return LinuxErrno::BadFileDescriptor.return_value();
    }
    if count == 0 {
        return 0;
    }
    if !super::user_mode::contains_user_stack_range(task_index, buffer_address, count) {
        return LinuxErrno::Fault.return_value();
    }

    // 第一个字节阻塞等待；之后只读取当前已经到达的字节，允许 read 返回短读。
    let mut bytes_read = 0;
    while bytes_read < count {
        let byte = if bytes_read == 0 {
            console::read_byte()
        } else {
            let Some(byte) = console::try_read_byte() else {
                break;
            };
            byte
        };

        // 安全性：前面的范围检查保证该字节位于当前任务已映射的可写用户栈内。
        unsafe {
            super::memory::write_user_value(buffer_address + bytes_read, byte);
        }
        bytes_read += 1;
    }

    bytes_read as isize
}

fn write(
    task_index: usize,
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
        super::user_mode::contains_user_stack_range(task_index, buffer_address, count);
    if !buffer_is_in_program && !buffer_is_in_stack {
        crate::klog_trace!(
            "拒绝任务 {} 的 write 缓冲区：地址={:#018x}，长度={}",
            task_index,
            buffer_address,
            count
        );
        return LinuxErrno::Fault.return_value();
    }

    // 安全性：范围检查保证待读字节位于当前任务已映射的镜像或栈内。
    for offset in 0..count {
        // 安全性：完整缓冲区已验证，单字节读取期间由 memory 模块短暂开启 SUM。
        let byte = unsafe { super::memory::read_user_byte(buffer_address + offset) };
        console::write_byte(byte);
    }

    // UART 轮询写入要么完成整个缓冲区，要么设备永久不就绪；当前阶段不报告部分写入。
    count as isize
}

fn clock_gettime(task_index: usize, clock_id: usize, timespec_address: usize) -> isize {
    if clock_id != CLOCK_MONOTONIC {
        return LinuxErrno::InvalidArgument.return_value();
    }

    let timespec_size = core::mem::size_of::<super::clock::Timespec>();
    if timespec_address % core::mem::align_of::<super::clock::Timespec>() != 0
        || !super::user_mode::contains_user_stack_range(task_index, timespec_address, timespec_size)
    {
        return LinuxErrno::Fault.return_value();
    }

    let time = super::clock::monotonic_timespec();
    // 安全性：对齐检查和栈范围检查保证目标是当前任务已映射的完整可写 timespec。
    unsafe {
        super::memory::write_user_value(timespec_address, time);
    }

    0
}
