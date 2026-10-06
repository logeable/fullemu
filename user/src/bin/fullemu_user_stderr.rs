#![no_std]
#![no_main]

//! 通过标准错误输出诊断文本，并检查系统调用返回值。

use fullemu_user::{syscall, user_entry};

const MESSAGE: &[u8] = b"[stderr] This message uses file descriptor 2.\n";

/// 使用标准错误文件描述符输出诊断文本。
pub fn main() {
    let a = 1;
    let b = MESSAGE[0] - MESSAGE[0];
    if true {
        let _ = a / b;
    }
    let bytes_written = syscall::write(2, MESSAGE);
    if bytes_written != MESSAGE.len() as isize {
        syscall::exit(2);
    }
}

user_entry!(main);
