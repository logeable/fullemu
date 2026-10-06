//! 为内核诊断提供时间计数、级别、来源和统一前缀；实际字节输出仍由平台 console 完成。

use core::fmt;

use crate::arch::riscv64::console;

/// 内核日志的严重程度，顺序也表示过滤时的优先级。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// 内核无法继续或关键不变量已被破坏。
    Error,
    /// 发生异常情况，但内核仍可继续运行。
    Warn,
    /// 用于记录正常的重要生命周期事件。
    Info,
    /// 用于排查子系统行为。
    Debug,
    /// 用于观察细粒度执行过程。
    Trace,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }
}

/// 根据编译时环境变量选择日志阈值；未设置或取值无效时使用 `Info`。
fn maximum_level() -> Option<Level> {
    match option_env!("FULLEMU_LOG_LEVEL").unwrap_or("info") {
        "off" => None,
        "error" => Some(Level::Error),
        "warn" => Some(Level::Warn),
        "info" => Some(Level::Info),
        "debug" => Some(Level::Debug),
        "trace" => Some(Level::Trace),
        _ => Some(Level::Info),
    }
}

/// 输出一条带级别和模块来源的内核日志。
///
/// 日志通过平台 console 同步输出，不申请堆内存。当前实现没有锁，调用方不得依赖它
/// 在多个 hart、可抢占上下文或嵌套中断之间保持整条记录的原子性。
pub fn log(level: Level, target: &str, arguments: fmt::Arguments<'_>) {
    let Some(maximum_level) = maximum_level() else {
        return;
    };

    if level > maximum_level {
        return;
    }

    let counter = crate::arch::riscv64::sbi::read_time();
    let frequency = crate::arch::riscv64::sbi::QEMU_VIRT_TIMEBASE_FREQUENCY_HZ;
    let seconds = counter / frequency;
    let microseconds = (counter % frequency) * 1_000_000 / frequency;
    console::write_fmt(format_args!(
        "[{} time={seconds}.{microseconds:06}s {}] {}\n",
        level.label(),
        target,
        arguments
    ));
}

/// 按指定级别记录一条格式化内核日志。
#[macro_export]
macro_rules! klog {
    ($level:expr, $($argument:tt)*) => {{
        $crate::kernel::logging::log(
            $level,
            module_path!(),
            core::format_args!($($argument)*)
        );
    }};
}

/// 记录错误级别日志。
#[macro_export]
macro_rules! klog_error {
    ($($argument:tt)*) => {
        $crate::klog!($crate::kernel::logging::Level::Error, $($argument)*)
    };
}

/// 记录警告级别日志。
#[macro_export]
macro_rules! klog_warn {
    ($($argument:tt)*) => {
        $crate::klog!($crate::kernel::logging::Level::Warn, $($argument)*)
    };
}

/// 记录信息级别日志。
#[macro_export]
macro_rules! klog_info {
    ($($argument:tt)*) => {
        $crate::klog!($crate::kernel::logging::Level::Info, $($argument)*)
    };
}

/// 记录调试级别日志；当前默认阈值会过滤该级别。
#[macro_export]
macro_rules! klog_debug {
    ($($argument:tt)*) => {
        $crate::klog!($crate::kernel::logging::Level::Debug, $($argument)*)
    };
}

/// 记录跟踪级别日志；当前默认阈值会过滤该级别。
#[macro_export]
macro_rules! klog_trace {
    ($($argument:tt)*) => {
        $crate::klog!($crate::kernel::logging::Level::Trace, $($argument)*)
    };
}
