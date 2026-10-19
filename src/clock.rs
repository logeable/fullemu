//! 为内核提供基于 RISC-V `time` 计数器的单调时钟。

use core::sync::atomic::{AtomicU64, Ordering};

static KERNEL_START_TIME: AtomicU64 = AtomicU64::new(0);

/// 由启动路径记录内核单调时钟的起点。
pub fn initialize() {
    KERNEL_START_TIME.store(crate::arch::riscv64::sbi::read_time(), Ordering::Relaxed);
}

/// 表示内核单调时钟中的一个时间点。
#[derive(Clone, Copy)]
pub(crate) struct MonotonicTime {
    /// 从内核时钟起点经过的整秒数。
    pub(crate) seconds: u64,
    /// 当前整秒内经过的纳秒数。
    pub(crate) nanoseconds: u32,
}

/// 返回从内核启动计时起点到现在经过的单调时间。
pub(crate) fn monotonic_time() -> MonotonicTime {
    let start = KERNEL_START_TIME.load(Ordering::Relaxed);
    let elapsed = crate::arch::riscv64::sbi::read_time().saturating_sub(start);
    let frequency = crate::arch::riscv64::sbi::QEMU_VIRT_TIMEBASE_FREQUENCY_HZ;
    let seconds = elapsed / frequency;
    let nanoseconds = (elapsed % frequency) * 1_000_000_000 / frequency;

    MonotonicTime {
        seconds,
        nanoseconds: nanoseconds as u32,
    }
}
