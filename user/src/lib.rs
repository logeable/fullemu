#![no_std]
//! 为所有独立用户程序提供共用的最小运行时。

pub mod syscall;

core::arch::global_asm!(include_str!("start.S"));

struct StandardOutput;

impl core::fmt::Write for StandardOutput {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let bytes = text.as_bytes();
        let mut written = 0;

        while written < bytes.len() {
            let remaining = &bytes[written..];
            let result = syscall::write(1, remaining);
            if result <= 0 || result as usize > remaining.len() {
                return Err(core::fmt::Error);
            }
            written += result as usize;
        }

        Ok(())
    }
}

/// 将格式化文本写入标准输出。
///
/// 为避免引入堆分配，此接口直接通过 `write` 系统调用逐段输出。
/// 便捷宏忽略输出错误；需要检查错误时可直接调用此函数。
pub fn print(arguments: core::fmt::Arguments<'_>) -> core::fmt::Result {
    let mut output = StandardOutput;
    core::fmt::Write::write_fmt(&mut output, arguments)
}

/// 定义用户程序 `main` 返回值到进程退出状态的转换。
pub trait UserMainReturn {
    /// 转换为传给 Linux `exit` 系统调用的状态码。
    fn into_exit_code(self) -> i32;
}

impl UserMainReturn for () {
    fn into_exit_code(self) -> i32 {
        0
    }
}

impl UserMainReturn for i32 {
    fn into_exit_code(self) -> i32 {
        self
    }
}

/// 将格式化文本写入标准输出。
#[macro_export]
macro_rules! print {
    ($($argument:tt)*) => {{
        let _ = $crate::print(core::format_args!($($argument)*));
    }};
}

/// 将格式化文本和换行符写入标准输出。
#[macro_export]
macro_rules! println {
    () => {{
        let _ = $crate::print(core::format_args!("\n"));
    }};
    ($($argument:tt)*) => {{
        let _ = $crate::print(core::format_args!("{}\n", core::format_args!($($argument)*)));
    }};
}

/// 为独立用户程序生成启动汇编所需的 C ABI 入口。
///
/// 用户程序提供返回 `()` 或 `i32` 的普通 Rust `main` 函数；该入口把返回值转换为进程退出状态。
#[macro_export]
macro_rules! user_entry {
    ($main:path) => {
        #[no_mangle]
        pub extern "C" fn user_main() -> ! {
            let exit_code = $crate::UserMainReturn::into_exit_code($main());
            $crate::syscall::exit(exit_code)
        }
    };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    println!("panic: {}", info);
    syscall::exit(1)
}
