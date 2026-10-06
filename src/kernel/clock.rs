//! 为内核提供基于 RISC-V `time` 计数器的单调时钟。

use core::sync::atomic::{AtomicU64, Ordering};

static KERNEL_START_TIME: AtomicU64 = AtomicU64::new(0);

/// 由启动路径记录内核单调时钟的起点。
pub fn initialize() {
    KERNEL_START_TIME.store(crate::arch::riscv64::sbi::read_time(), Ordering::Relaxed);
}

/// Linux RISC-V 64 位 ABI 使用的时间结构布局。
#[repr(C)]
pub struct Timespec {
    /// 从内核时钟起点经过的整秒数。
    pub seconds: i64,
    /// 当前整秒内经过的纳秒数。
    pub nanoseconds: i64,
}

/// 返回从内核启动计时起点到现在经过的单调时间。
pub fn monotonic_timespec() -> Timespec {
    let start = KERNEL_START_TIME.load(Ordering::Relaxed);
    let elapsed = crate::arch::riscv64::sbi::read_time().saturating_sub(start);
    let frequency = crate::arch::riscv64::sbi::QEMU_VIRT_TIMEBASE_FREQUENCY_HZ;
    let seconds = elapsed / frequency;
    let nanoseconds = (elapsed % frequency) * 1_000_000_000 / frequency;

    Timespec {
        seconds: seconds as i64,
        nanoseconds: nanoseconds as i64,
    }
}
