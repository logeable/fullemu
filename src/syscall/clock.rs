//! 适配 Linux `clock_gettime` 调用。

use super::abi::*;
use super::context::SyscallContext;

/// Linux RISC-V 64 位 ABI 的 `timespec` 内存布局。
#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxTimespec {
    /// 从内核时钟起点经过的整秒数。
    seconds: i64,
    /// 当前整秒内经过的纳秒数。
    nanoseconds: i64,
}

pub(super) fn clock_gettime(
    context: SyscallContext,
    clock_id: usize,
    timespec_address: usize,
) -> isize {
    if clock_id != CLOCK_MONOTONIC {
        return LinuxErrno::InvalidArgument.return_value();
    }

    let timespec_size = core::mem::size_of::<LinuxTimespec>();
    if timespec_address % core::mem::align_of::<LinuxTimespec>() != 0
        || !context.contains_writable_buffer(timespec_address, timespec_size)
    {
        return LinuxErrno::Fault.return_value();
    }

    let time = crate::clock::monotonic_time();
    let timespec = LinuxTimespec {
        seconds: time.seconds as i64,
        nanoseconds: time.nanoseconds as i64,
    };
    // 安全性：对齐检查和栈范围检查保证目标是当前进程已映射的完整可写 timespec。
    unsafe {
        crate::memory::user_access::write_user_value(timespec_address, timespec);
    }

    0
}
