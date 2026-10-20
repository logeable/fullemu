//! 适配进程控制相关的 Linux RISC-V 系统调用。

use super::abi::*;
use super::context::SyscallContext;
use crate::arch::riscv64::trap::TrapFrame;
use alloc::vec::Vec;

/// 结束当前进程。
pub(super) fn exit(arguments: [usize; 5]) -> *mut TrapFrame {
    crate::process::exit::exit_current_process(arguments[0] as i32 as u8)
}

/// 处理 Linux `sched_yield`，让当前就绪进程主动让出处理器。
pub(super) fn yield_cpu(frame: &mut TrapFrame) -> *mut TrapFrame {
    frame.registers[10] = 0;
    super::advance_system_call(frame);
    crate::process::yield_current_process()
}

/// 按 Linux `brk` 语义查询或更新当前进程堆边界。
pub(super) fn brk(address: usize, frame: &mut TrapFrame) -> *mut TrapFrame {
    let new_break = crate::process::set_current_program_break(address);
    super::return_to_user(frame, new_break as isize)
}

pub(super) fn execve(
    context: SyscallContext,
    arguments: [usize; 5],
    frame: &mut TrapFrame,
) -> *mut TrapFrame {
    if arguments[2] != 0 {
        return super::return_to_user(frame, LinuxErrno::InvalidArgument.return_value());
    }

    let mut program_name = [0u8; MAX_EXEC_PROGRAM_NAME_LENGTH];
    let program_name_length = match copy_exec_program_name(context, arguments[0], &mut program_name)
    {
        Ok(length) => length,
        Err(error) => return super::return_to_user(frame, error.return_value()),
    };
    let program_name = &program_name[..program_name_length];

    let program_arguments = match copy_exec_arguments(context, arguments[1]) {
        Ok(arguments) => arguments,
        Err(error) => return super::return_to_user(frame, error.return_value()),
    };

    let Ok(program_name) = core::str::from_utf8(program_name) else {
        return super::return_to_user(frame, LinuxErrno::NoEntry.return_value());
    };
    let Some(program_index) = crate::user_program::find_index(program_name) else {
        return super::return_to_user(frame, LinuxErrno::NoEntry.return_value());
    };

    match crate::process::exec::replace_current_process(program_index, &program_arguments, frame) {
        Ok(()) => crate::process::resume_user(frame),
        Err(crate::process::exec::ExecError::Resources) => {
            super::return_to_user(frame, LinuxErrno::NoMemory.return_value())
        }
        Err(crate::process::exec::ExecError::InvalidImage) => {
            super::return_to_user(frame, LinuxErrno::ExecFormat.return_value())
        }
        Err(crate::process::exec::ExecError::ArgumentsTooLarge) => {
            super::return_to_user(frame, LinuxErrno::TooBig.return_value())
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
        if !context.contains_readable_buffer(slot_address, core::mem::size_of::<usize>()) {
            return Err(LinuxErrno::Fault);
        }

        // 安全性：完整指针槽已验证位于当前进程可读的程序段或用户栈，并满足自然对齐。
        let argument_address: usize =
            unsafe { crate::memory::user_access::read_user_value(slot_address) };
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
            if !context.contains_readable_buffer(byte_address, 1) {
                return Err(LinuxErrno::Fault);
            }

            // 安全性：逐字节验证字符串位于当前进程可读的程序段或用户栈后再读取。
            let byte = unsafe { crate::memory::user_access::read_user_byte(byte_address) };
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
        if !context.contains_readable_buffer(user_address, 1) {
            return Err(LinuxErrno::Fault);
        }
        // 安全性：逐字节验证该地址属于当前进程的程序段或用户栈，再从当前地址空间读取。
        *byte = unsafe { crate::memory::user_access::read_user_byte(user_address) };
        if *byte == 0 {
            return Ok(offset);
        }
    }
    Err(LinuxErrno::NameTooLong)
}

pub(super) fn fork(arguments: [usize; 5], frame: &mut TrapFrame) -> *mut TrapFrame {
    if arguments[0] != LINUX_SIGCHLD || arguments[1..].iter().any(|argument| *argument != 0) {
        return super::return_to_user(frame, LinuxErrno::InvalidArgument.return_value());
    }

    let result = match crate::process::fork::fork_current_process(frame) {
        Ok(child_pid) => child_pid as isize,
        Err(crate::process::fork::ForkError::Resources) => process_creation_error(),
        Err(crate::process::fork::ForkError::ProcessIdExhausted) => process_id_exhaustion_error(),
    };
    super::return_to_user(frame, result)
}

pub(super) fn wait4(
    context: SyscallContext,
    arguments: [usize; 5],
    frame: &mut TrapFrame,
) -> *mut TrapFrame {
    let pid = arguments[0] as isize;
    let Ok(pid) = i32::try_from(pid) else {
        return super::return_to_user(frame, LinuxErrno::InvalidArgument.return_value());
    };
    if (pid <= 0 && pid != -1)
        || (arguments[2] != 0 && arguments[2] != LINUX_WNOHANG)
        || arguments[3] != 0
    {
        return super::return_to_user(frame, LinuxErrno::InvalidArgument.return_value());
    }

    let status_address = if arguments[1] == 0 {
        None
    } else {
        let address = arguments[1];
        if address % core::mem::align_of::<i32>() != 0
            || !context.contains_writable_buffer(address, core::mem::size_of::<i32>())
        {
            return super::return_to_user(frame, LinuxErrno::Fault.return_value());
        }
        Some(address)
    };

    match crate::process::wait::wait_for_current_child(
        pid,
        status_address,
        arguments[2] == LINUX_WNOHANG,
    ) {
        crate::process::wait::WaitDecision::Return(result) => super::return_to_user(frame, result),
        crate::process::wait::WaitDecision::Block => {
            super::advance_system_call(frame);
            crate::process::schedule_blocked_process()
        }
        crate::process::wait::WaitDecision::NoChild => {
            super::return_to_user(frame, no_child_process_error())
        }
    }
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
