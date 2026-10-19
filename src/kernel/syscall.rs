//! 实现当前内核支持的最小 Linux RISC-V 系统调用集合。

use crate::arch::riscv64::trap::TrapFrame;
use crate::console;
use alloc::vec::Vec;

const LINUX_WRITE_SYSCALL: usize = 64;
const LINUX_READ_SYSCALL: usize = 63;
const LINUX_CLOCK_GETTIME_SYSCALL: usize = 113;
const LINUX_EXIT_SYSCALL: usize = 93;
const LINUX_SCHED_YIELD_SYSCALL: usize = 124;
const LINUX_CLONE_SYSCALL: usize = 220;
const LINUX_EXECVE_SYSCALL: usize = 221;
const LINUX_WAIT4_SYSCALL: usize = 260;
const LINUX_SIGCHLD: usize = 17;
const LINUX_WNOHANG: usize = 1;
const STDIN_FILE_DESCRIPTOR: usize = 0;
const STDOUT_FILE_DESCRIPTOR: usize = 1;
const STDERR_FILE_DESCRIPTOR: usize = 2;
const CLOCK_MONOTONIC: usize = 1;
const MAX_EXEC_PROGRAM_NAME_LENGTH: usize = 128;
const MAX_EXEC_ARGUMENT_COUNT: usize = 32;
const MAX_EXEC_ARGUMENT_BYTES: usize = 4096;

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
    TooBig = 7,
    Again = 11,
    BadFileDescriptor = 9,
    Child = 10,
    NoEntry = 2,
    NoMemory = 12,
    InvalidArgument = 22,
    Fault = 14,
    NameTooLong = 36,
    ExecFormat = 8,
    NoSystemCall = 38,
}

impl LinuxErrno {
    const fn return_value(self) -> isize {
        -(self as isize)
    }
}

/// 记录当前进程的身份及 syscall 可访问的用户内存范围。
#[derive(Clone, Copy)]
struct SyscallContext {
    process_id: usize,
    user_memory: super::memory::UserMemoryRanges,
}

impl SyscallContext {
    const fn new(process_id: usize, user_memory: super::memory::UserMemoryRanges) -> Self {
        Self {
            process_id,
            user_memory,
        }
    }

    fn contains_stack_buffer(self, address: usize, length: usize) -> bool {
        contains_range(self.user_memory.stack, address, length)
    }

    fn contains_program_or_stack_buffer(self, address: usize, length: usize) -> bool {
        contains_range(self.user_memory.program, address, length)
            || contains_range(self.user_memory.stack, address, length)
    }

    fn contains_writable_program_or_stack_buffer(self, address: usize, length: usize) -> bool {
        self.user_memory
            .writable_program
            .is_some_and(|range| contains_range(range, address, length))
            || contains_range(self.user_memory.stack, address, length)
    }
}

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
        super::process::current_linux_pid() as usize,
        super::process::current_user_memory_ranges(),
    );

    match number {
        LINUX_READ_SYSCALL => return_to_user(
            frame,
            read(context, arguments[0], arguments[1], arguments[2]),
        ),
        LINUX_WRITE_SYSCALL => return_to_user(
            frame,
            write(context, arguments[0], arguments[1], arguments[2]),
        ),
        LINUX_CLOCK_GETTIME_SYSCALL => {
            return_to_user(frame, clock_gettime(context, arguments[0], arguments[1]))
        }
        LINUX_EXIT_SYSCALL => super::process::exit::exit_current_process(arguments[0] as i32 as u8),
        LINUX_SCHED_YIELD_SYSCALL => {
            frame.registers[10] = 0;
            advance_system_call(frame);
            super::process::yield_current_process()
        }
        LINUX_CLONE_SYSCALL => fork(arguments, frame),
        LINUX_EXECVE_SYSCALL => execve(context, arguments, frame),
        LINUX_WAIT4_SYSCALL => wait4(context, arguments, frame),
        _ => return_to_user(frame, LinuxErrno::NoSystemCall.return_value()),
    }
}

fn execve(context: SyscallContext, arguments: [usize; 5], frame: &mut TrapFrame) -> *mut TrapFrame {
    if arguments[2] != 0 {
        return return_to_user(frame, LinuxErrno::InvalidArgument.return_value());
    }

    let mut program_name = [0u8; MAX_EXEC_PROGRAM_NAME_LENGTH];
    let program_name_length = match copy_exec_program_name(context, arguments[0], &mut program_name)
    {
        Ok(length) => length,
        Err(error) => return return_to_user(frame, error.return_value()),
    };
    let program_name = &program_name[..program_name_length];

    let program_arguments = match copy_exec_arguments(context, arguments[1]) {
        Ok(arguments) => arguments,
        Err(error) => return return_to_user(frame, error.return_value()),
    };

    let Ok(program_name) = core::str::from_utf8(program_name) else {
        return return_to_user(frame, LinuxErrno::NoEntry.return_value());
    };
    let Some(program_index) = super::user_program::find_index(program_name) else {
        return return_to_user(frame, LinuxErrno::NoEntry.return_value());
    };

    match super::process::exec::replace_current_process(program_index, &program_arguments, frame) {
        Ok(()) => super::process::resume_user(frame),
        Err(super::process::exec::ExecError::Resources) => {
            return_to_user(frame, LinuxErrno::NoMemory.return_value())
        }
        Err(super::process::exec::ExecError::InvalidImage) => {
            return_to_user(frame, LinuxErrno::ExecFormat.return_value())
        }
        Err(super::process::exec::ExecError::ArgumentsTooLarge) => {
            return_to_user(frame, LinuxErrno::TooBig.return_value())
        }
    }
}

fn copy_exec_arguments(
    context: SyscallContext,
    vector_address: usize,
) -> Result<Vec<Vec<u8>>, LinuxErrno> {
    let mut arguments = Vec::new();
    if vector_address == 0 {
        return Ok(arguments);
    }
    if vector_address % core::mem::align_of::<usize>() != 0 {
        return Err(LinuxErrno::Fault);
    }

    let mut total_bytes = 0;
    for argument_index in 0..=MAX_EXEC_ARGUMENT_COUNT {
        let vector_offset = argument_index
            .checked_mul(core::mem::size_of::<usize>())
            .ok_or(LinuxErrno::Fault)?;
        let slot_address = vector_address
            .checked_add(vector_offset)
            .ok_or(LinuxErrno::Fault)?;
        if !context.contains_program_or_stack_buffer(slot_address, core::mem::size_of::<usize>()) {
            return Err(LinuxErrno::Fault);
        }

        // 安全性：完整指针槽已验证位于当前进程可读的程序段或用户栈，并满足自然对齐。
        let argument_address: usize =
            unsafe { super::memory::user_access::read_user_value(slot_address) };
        if argument_address == 0 {
            return Ok(arguments);
        }
        if argument_index == MAX_EXEC_ARGUMENT_COUNT {
            return Err(LinuxErrno::TooBig);
        }

        let mut argument = Vec::new();
        loop {
            if total_bytes == MAX_EXEC_ARGUMENT_BYTES {
                return Err(LinuxErrno::TooBig);
            }
            let Some(byte_address) = argument_address.checked_add(argument.len()) else {
                return Err(LinuxErrno::Fault);
            };
            if !context.contains_program_or_stack_buffer(byte_address, 1) {
                return Err(LinuxErrno::Fault);
            }

            // 安全性：逐字节验证字符串位于当前进程可读的程序段或用户栈后再读取。
            let byte = unsafe { super::memory::user_access::read_user_byte(byte_address) };
            total_bytes += 1;
            if byte == 0 {
                break;
            }
            argument.try_reserve(1).map_err(|_| LinuxErrno::NoMemory)?;
            argument.push(byte);
        }

        arguments.try_reserve(1).map_err(|_| LinuxErrno::NoMemory)?;
        arguments.push(argument);
    }

    unreachable!("exec argv 遍历必定遇到空指针或达到参数上限")
}

fn copy_exec_program_name(
    context: SyscallContext,
    address: usize,
    destination: &mut [u8; MAX_EXEC_PROGRAM_NAME_LENGTH],
) -> Result<usize, LinuxErrno> {
    for (offset, byte) in destination.iter_mut().enumerate() {
        let Some(user_address) = address.checked_add(offset) else {
            return Err(LinuxErrno::Fault);
        };
        if !context.contains_program_or_stack_buffer(user_address, 1) {
            return Err(LinuxErrno::Fault);
        }
        // 安全性：逐字节验证该地址属于当前进程的程序段或用户栈，再从当前地址空间读取。
        *byte = unsafe { super::memory::user_access::read_user_byte(user_address) };
        if *byte == 0 {
            return Ok(offset);
        }
    }
    Err(LinuxErrno::NameTooLong)
}

fn fork(arguments: [usize; 5], frame: &mut TrapFrame) -> *mut TrapFrame {
    if arguments[0] != LINUX_SIGCHLD || arguments[1..].iter().any(|argument| *argument != 0) {
        return return_to_user(frame, LinuxErrno::InvalidArgument.return_value());
    }

    let result = match super::process::fork::fork_current_process(frame) {
        Ok(child_pid) => child_pid as isize,
        Err(super::process::fork::ForkError::Resources) => process_creation_error(),
        Err(super::process::fork::ForkError::ProcessIdExhausted) => process_id_exhaustion_error(),
    };
    return_to_user(frame, result)
}

fn wait4(context: SyscallContext, arguments: [usize; 5], frame: &mut TrapFrame) -> *mut TrapFrame {
    let pid = arguments[0] as isize;
    let Ok(pid) = i32::try_from(pid) else {
        return return_to_user(frame, LinuxErrno::InvalidArgument.return_value());
    };
    if (pid <= 0 && pid != -1)
        || (arguments[2] != 0 && arguments[2] != LINUX_WNOHANG)
        || arguments[3] != 0
    {
        return return_to_user(frame, LinuxErrno::InvalidArgument.return_value());
    }

    let status_address = if arguments[1] == 0 {
        None
    } else {
        let address = arguments[1];
        if address % core::mem::align_of::<i32>() != 0
            || !context
                .contains_writable_program_or_stack_buffer(address, core::mem::size_of::<i32>())
        {
            return return_to_user(frame, LinuxErrno::Fault.return_value());
        }
        Some(address)
    };

    match super::process::wait::wait_for_current_child(
        pid,
        status_address,
        arguments[2] == LINUX_WNOHANG,
    ) {
        super::process::wait::WaitDecision::Return(result) => return_to_user(frame, result),
        super::process::wait::WaitDecision::Block => {
            advance_system_call(frame);
            super::process::schedule_blocked_process()
        }
        super::process::wait::WaitDecision::NoChild => {
            return_to_user(frame, no_child_process_error())
        }
    }
}

fn return_to_user(frame: &mut TrapFrame, result: isize) -> *mut TrapFrame {
    frame.registers[10] = result as usize;
    advance_system_call(frame);
    super::process::resume_user(frame)
}

fn advance_system_call(frame: &mut TrapFrame) {
    frame.exception_pc += 4;
}

/// 将不支持的等待目标转换为 Linux `ECHILD`。
const fn no_child_process_error() -> isize {
    LinuxErrno::Child.return_value()
}

/// 将进程创建资源不足转换为 Linux `ENOMEM`。
const fn process_creation_error() -> isize {
    LinuxErrno::NoMemory.return_value()
}

/// 将 Linux PID 耗尽转换为 `EAGAIN`。
const fn process_id_exhaustion_error() -> isize {
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
        let byte = unsafe { super::memory::user_access::read_user_byte(buffer_address + offset) };
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
