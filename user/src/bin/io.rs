#![no_std]
#![no_main]

//! 展示标准输出、标准错误和多次控制台写入。

use fullemu_user::syscall;

const STDERR_MESSAGE: &[u8] = "I/O 示例：这行写入标准错误。\n".as_bytes();

pub fn main() {
    fullemu_user::println!("I/O 示例：这行写入标准输出。");
    fullemu_user::println!("I/O 示例：格式化数字 {}。", 42);

    let bytes_written = syscall::write(2, STDERR_MESSAGE);
    if bytes_written != STDERR_MESSAGE.len() as isize {
        syscall::exit(1);
    }
}

fullemu_user::user_entry!(main);
