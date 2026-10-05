//! 通过 SBI 调用 OpenSBI 提供的 supervisor 定时器服务。

const TIME_EXTENSION_ID: usize = 0x5449_4d45;
const SET_TIMER_FUNCTION_ID: usize = 0;

/// SBI 返回的错误码。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SbiError(pub isize);

/// 设置下一次 S-mode 定时器中断的绝对时间。
///
/// 时间单位与 RISC-V `time` 计数器一致。调用后，OpenSBI 负责安排下一次
/// supervisor timer interrupt；本函数不负责打开 S-mode 中断使能位。
pub fn set_timer(deadline: u64) -> Result<(), SbiError> {
    let error: isize;

    // 安全性：`a7` 和 `a6` 按 SBI 调用约定选择 TIME 扩展及 set_timer；
    // 调用发生在 S-mode，且其余寄存器由内联汇编声明为输入或输出。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") deadline as usize => error,
            lateout("a1") _,
            in("a6") SET_TIMER_FUNCTION_ID,
            in("a7") TIME_EXTENSION_ID,
        );
    }

    if error == 0 {
        Ok(())
    } else {
        Err(SbiError(error))
    }
}

/// 读取当前 hart 的自由运行时间计数器。
pub fn read_time() -> u64 {
    let time: u64;

    // 安全性：本目标平台允许 S-mode 读取 `time` 计数器；指令只读取 CSR 别名。
    unsafe { core::arch::asm!("rdtime {time}", time = out(reg) time, options(nomem, nostack)) };

    time
}
