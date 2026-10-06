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
/// 用户程序提供普通 Rust `main` 函数；该入口负责调用它，并在其正常返回后以状态码 0 退出。
#[macro_export]
macro_rules! user_entry {
    ($main:path) => {
        #[no_mangle]
        pub extern "C" fn user_main() -> ! {
            $main();
            $crate::syscall::exit(0)
        }
    };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    println!("panic: {}", info);
    syscall::exit(1)
}
