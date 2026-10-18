//! 实现当前内核支持的最小 Linux RISC-V 系统调用集合。

use crate::device::console;

const LINUX_WRITE_SYSCALL: usize = 64;
const LINUX_READ_SYSCALL: usize = 63;
const LINUX_CLOCK_GETTIME_SYSCALL: usize = 113;
const LINUX_EXIT_SYSCALL: usize = 93;
const LINUX_SCHED_YIELD_SYSCALL: usize = 124;
const LINUX_CLONE_SYSCALL: usize = 220;
const LINUX_WAIT4_SYSCALL: usize = 260;
const LINUX_SIGCHLD: usize = 17;
const LINUX_WNOHANG: usize = 1;
const STDIN_FILE_DESCRIPTOR: usize = 0;
const STDOUT_FILE_DESCRIPTOR: usize = 1;
const STDERR_FILE_DESCRIPTOR: usize = 2;
const CLOCK_MONOTONIC: usize = 1;

/// Linux RISC-V 64 位 ABI 的 `timespec` 内存布局。
#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxTimespec {
    /// 从内核时钟起点经过的整秒数。
    seconds: i64,
    /// 当前整秒内经过的纳秒数。
    nanoseconds: i64,
}

#[derive(Clone, Copy)]
#[repr(isize)]
enum LinuxErrno {
    Again = 11,
    BadFileDescriptor = 9,
    Child = 10,
    NoMemory = 12,
    InvalidArgument = 22,
    Fault = 14,
    NoSystemCall = 38,
}

impl LinuxErrno {
    const fn return_value(self) -> isize {
        -(self as isize)
    }
}

/// 表示系统调用返回用户态、让出 CPU，或结束当前用户进程。
pub(super) enum SyscallOutcome {
    /// 将结果写入 `a0` 后恢复用户上下文。
    Return(isize),
    /// 当前进程调用 `exit`；Linux 对外可观察的正常退出码只有低 8 位。
    Exit(u8),
    /// 当前进程主动让出 CPU，调度器运行另一个就绪进程。
    Yield,
    /// 以独立地址空间复制当前进程；父进程和子进程分别返回 PID 和 0。
    Fork,
    /// 等待一个指定子进程或任意子进程。
    Wait(WaitRequest),
}

/// 已通过 syscall 参数检查的 `wait4` 请求。
#[derive(Clone, Copy)]
pub(super) struct WaitRequest {
    pub(super) pid: i32,
    pub(super) status_address: Option<usize>,
    pub(super) no_hang: bool,
}

/// 记录一次系统调用允许访问的用户程序和用户栈范围。
///
/// 调度器在分发前从当前进程构造该值；syscall 层据此检查用户指针，不反向查询进程表。
#[derive(Clone, Copy)]
pub(super) struct SyscallContext {
    process_id: usize,
    program_range: (usize, usize),
    stack_range: (usize, usize),
}

impl SyscallContext {
    pub(super) const fn new(
        process_id: usize,
        program_range: (usize, usize),
        stack_range: (usize, usize),
    ) -> Self {
        Self {
            process_id,
            program_range,
            stack_range,
        }
    }

    fn contains_stack_buffer(self, address: usize, length: usize) -> bool {
        contains_range(self.stack_range, address, length)
    }

    fn contains_program_or_stack_buffer(self, address: usize, length: usize) -> bool {
        contains_range(self.program_range, address, length)
            || contains_range(self.stack_range, address, length)
    }
}

/// 按 Linux RISC-V syscall ABI 分发调用，并描述返回或进程退出结果。
pub(super) fn dispatch(
    context: SyscallContext,
    number: usize,
    arguments: [usize; 5],
) -> SyscallOutcome {
    match number {
        LINUX_READ_SYSCALL => {
            SyscallOutcome::Return(read(context, arguments[0], arguments[1], arguments[2]))
        }
        LINUX_WRITE_SYSCALL => {
            SyscallOutcome::Return(write(context, arguments[0], arguments[1], arguments[2]))
        }
        LINUX_CLOCK_GETTIME_SYSCALL => {
            SyscallOutcome::Return(clock_gettime(context, arguments[0], arguments[1]))
        }
        LINUX_EXIT_SYSCALL => SyscallOutcome::Exit(arguments[0] as i32 as u8),
        LINUX_SCHED_YIELD_SYSCALL => SyscallOutcome::Yield,
        LINUX_CLONE_SYSCALL => clone(arguments),
        LINUX_WAIT4_SYSCALL => wait4(context, arguments),
        _ => SyscallOutcome::Return(LinuxErrno::NoSystemCall.return_value()),
    }
}

fn clone(arguments: [usize; 5]) -> SyscallOutcome {
    if arguments[0] == LINUX_SIGCHLD && arguments[1..].iter().all(|argument| *argument == 0) {
        SyscallOutcome::Fork
    } else {
        SyscallOutcome::Return(LinuxErrno::InvalidArgument.return_value())
    }
}

fn wait4(context: SyscallContext, arguments: [usize; 5]) -> SyscallOutcome {
    let pid = arguments[0] as isize;
    let Ok(pid) = i32::try_from(pid) else {
        return SyscallOutcome::Return(LinuxErrno::InvalidArgument.return_value());
    };
    if (pid <= 0 && pid != -1)
        || (arguments[2] != 0 && arguments[2] != LINUX_WNOHANG)
        || arguments[3] != 0
    {
        return SyscallOutcome::Return(LinuxErrno::InvalidArgument.return_value());
    }

    let status_address = if arguments[1] == 0 {
        None
    } else {
        let address = arguments[1];
        if address % core::mem::align_of::<i32>() != 0
            || !context.contains_program_or_stack_buffer(address, core::mem::size_of::<i32>())
        {
            return SyscallOutcome::Return(LinuxErrno::Fault.return_value());
        }
        Some(address)
    };

    SyscallOutcome::Wait(WaitRequest {
        pid,
        status_address,
        no_hang: arguments[2] == LINUX_WNOHANG,
    })
}

/// 将不支持的等待目标转换为 Linux `ECHILD`。
pub(super) const fn no_child_process_error() -> isize {
    LinuxErrno::Child.return_value()
}

/// 将进程创建资源不足转换为 Linux `ENOMEM`。
pub(super) const fn process_creation_error() -> isize {
    LinuxErrno::NoMemory.return_value()
}

/// 将 Linux PID 耗尽转换为 `EAGAIN`。
pub(super) const fn process_id_exhaustion_error() -> isize {
    LinuxErrno::Again.return_value()
}

fn read(
    context: SyscallContext,
    file_descriptor: usize,
    buffer_address: usize,
    count: usize,
) -> isize {
    if file_descriptor != STDIN_FILE_DESCRIPTOR {
        return LinuxErrno::BadFileDescriptor.return_value();
    }
    if count == 0 {
        return 0;
    }
    if !context.contains_stack_buffer(buffer_address, count) {
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

        // 安全性：前面的范围检查保证该字节位于当前进程已映射的可写用户栈内。
        unsafe {
            super::memory::user_access::write_user_value(buffer_address + bytes_read, byte);
        }
        bytes_read += 1;
    }

    bytes_read as isize
}

fn write(
    context: SyscallContext,
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

    if !context.contains_program_or_stack_buffer(buffer_address, count) {
        crate::klog_trace!(
            "拒绝进程 {} 的 write 缓冲区：地址={:#018x}，长度={}",
            context.process_id,
            buffer_address,
            count
        );
        return LinuxErrno::Fault.return_value();
    }

    // 安全性：范围检查保证待读字节位于当前进程已映射的镜像或栈内。
    for offset in 0..count {
        // 安全性：完整缓冲区已验证，单字节读取期间由 memory 模块短暂开启 SUM。
        let byte = unsafe {
            super::memory::user_access::read_user_byte(buffer_address + offset)
        };
        console::write_byte(byte);
    }

    // UART 轮询写入要么完成整个缓冲区，要么设备永久不就绪；当前阶段不报告部分写入。
    count as isize
}

fn clock_gettime(context: SyscallContext, clock_id: usize, timespec_address: usize) -> isize {
    if clock_id != CLOCK_MONOTONIC {
        return LinuxErrno::InvalidArgument.return_value();
    }

    let timespec_size = core::mem::size_of::<LinuxTimespec>();
    if timespec_address % core::mem::align_of::<LinuxTimespec>() != 0
        || !context.contains_stack_buffer(timespec_address, timespec_size)
    {
        return LinuxErrno::Fault.return_value();
    }

    let time = super::clock::monotonic_time();
    let timespec = LinuxTimespec {
        seconds: time.seconds as i64,
        nanoseconds: time.nanoseconds as i64,
    };
    // 安全性：对齐检查和栈范围检查保证目标是当前进程已映射的完整可写 timespec。
    unsafe {
        super::memory::user_access::write_user_value(timespec_address, timespec);
    }

    0
}

fn contains_range(range: (usize, usize), address: usize, length: usize) -> bool {
    if length == 0 {
        return true;
    }
    let Some(end) = address.checked_add(length) else {
        return false;
    };
    address >= range.0 && end <= range.1
}
