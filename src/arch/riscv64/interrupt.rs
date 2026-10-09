//! 管理当前 hart 的 S-mode 本地中断使能位。

const SUPERVISOR_TIMER_INTERRUPT_ENABLE: usize = 1 << 5;

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
