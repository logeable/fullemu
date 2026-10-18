#![no_std]
//! 为所有独立用户程序提供共用的最小运行时。

pub mod syscall;

use core::sync::atomic::{AtomicUsize, Ordering};

static INITIAL_STACK_POINTER: AtomicUsize = AtomicUsize::new(0);

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

/// 表示从新进程初始用户栈读取的命令行参数。
pub struct UserArguments {
    next_argument_pointer: usize,
    remaining: usize,
}

impl Iterator for UserArguments {
    type Item = &'static [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        // 安全性：迭代器只由入口宏从内核构造的初始栈创建；内核保证 argv 指针表和字符串位于本进程用户栈。
        let argument_address =
            unsafe { core::ptr::read(self.next_argument_pointer as *const usize) };
        if argument_address == 0 {
            self.remaining = 0;
            return None;
        }

        let mut argument_length = 0;
        while argument_length < 4096 {
            // 安全性：execve 将每个参数字符串及其 NUL 结尾复制到当前进程的已映射用户栈。
            let byte =
                unsafe { core::ptr::read((argument_address + argument_length) as *const u8) };
            if byte == 0 {
                break;
            }
            argument_length += 1;
        }
        if argument_length == 4096 {
            self.remaining = 0;
            return None;
        }

        self.next_argument_pointer += core::mem::size_of::<usize>();
        self.remaining -= 1;
        // 安全性：上述扫描已找到 NUL，且字节范围位于该进程保持映射的用户栈中。
        Some(unsafe { core::slice::from_raw_parts(argument_address as *const u8, argument_length) })
    }
}

/// 返回当前进程由 `execve` 提供的参数；最初启动的程序参数列表为空。
pub fn arguments() -> UserArguments {
    let initial_stack_pointer = INITIAL_STACK_POINTER.load(Ordering::Relaxed);
    if initial_stack_pointer == 0 {
        return UserArguments {
            next_argument_pointer: 0,
            remaining: 0,
        };
    }

    // 安全性：入口汇编只把内核准备的初始用户栈地址交给本运行时；首字为内核写入的 argc。
    let argument_count = unsafe { core::ptr::read(initial_stack_pointer as *const usize) };
    UserArguments {
        next_argument_pointer: initial_stack_pointer + core::mem::size_of::<usize>(),
        remaining: argument_count,
    }
}

#[doc(hidden)]
///
/// # Safety
///
/// `stack_pointer` 必须指向内核为当前进程构造的有效初始用户栈，并在进程运行期间保持映射。
pub unsafe fn set_initial_stack_pointer(stack_pointer: usize) {
    INITIAL_STACK_POINTER.store(stack_pointer, Ordering::Relaxed);
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
        pub extern "C" fn user_main(initial_stack_pointer: usize) -> ! {
            // 安全性：入口汇编把内核准备的当前进程初始用户栈作为参数传入。
            unsafe { $crate::set_initial_stack_pointer(initial_stack_pointer) };
            let exit_code = $crate::UserMainReturn::into_exit_code($main($crate::arguments()));
            $crate::syscall::exit(exit_code)
        }
    };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    println!("panic: {}", info);
    syscall::exit(1)
}
