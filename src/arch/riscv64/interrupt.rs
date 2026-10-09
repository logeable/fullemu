//! 管理当前 hart 的 S-mode 本地中断使能位。

const SUPERVISOR_TIMER_INTERRUPT_ENABLE: usize = 1 << 5;
const SUPERVISOR_INTERRUPT_ENABLE: usize = 1 << 1;

/// 暂时关闭 S-mode 全局中断，并在离开作用域时恢复进入前的状态。
///
/// 当前内核只有一个 hart；该保护用于防止定时器中断在共享内核状态更新到一半时切换任务。
pub struct SupervisorInterruptGuard {
    was_enabled: bool,
}

impl Drop for SupervisorInterruptGuard {
    fn drop(&mut self) {
        if self.was_enabled {
            // 安全性：只恢复进入临界区前已开启的 S-mode 全局中断位。
            unsafe {
                core::arch::asm!(
                    "csrs sstatus, {mask}",
                    mask = in(reg) SUPERVISOR_INTERRUPT_ENABLE,
                    options(nostack)
                );
            }
        }
    }
}

/// 关闭 S-mode 全局中断并返回状态恢复守卫。
///
/// 嵌套使用时，只有最外层进入前中断已开启的守卫会重新开启中断。
pub fn disable_supervisor_interrupts() -> SupervisorInterruptGuard {
    let previous_status: usize;
    // 安全性：`sstatus` 是当前 hart 的特权 CSR；一次 CSR 原子操作读取并清除 SIE 位。
    unsafe {
        core::arch::asm!(
            "csrrc {previous}, sstatus, {mask}",
            previous = out(reg) previous_status,
            mask = in(reg) SUPERVISOR_INTERRUPT_ENABLE,
            options(nostack)
        );
    }

    SupervisorInterruptGuard {
        was_enabled: previous_status & SUPERVISOR_INTERRUPT_ENABLE != 0,
    }
}

/// 只启用 S-mode 定时器中断。
///
/// 全局中断使能由从 U-mode 陷入和 `sret` 返回时的硬件状态转换控制。
pub fn enable_supervisor_timer() {
    // 安全性：`sie` 是当前 hart 的特权 CSR；掩码只开启 supervisor timer interrupt。
    unsafe {
        core::arch::asm!(
            "csrw sie, {mask}",
            mask = in(reg) SUPERVISOR_TIMER_INTERRUPT_ENABLE,
            options(nomem, nostack)
        );
    }
}
