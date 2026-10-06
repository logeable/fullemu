#![no_std]
#![no_main]

//! 编译为独立二进制镜像，由内核加载后在 U-mode 执行。

use fullemu_user::{syscall, user_entry};

const MESSAGE: &[u8] = b"[stdout] Hello from the first user program.\n";

/// 通过标准输出发送问候，并检查 `write` 返回的字节数。
pub fn main() {
    let bytes_written = syscall::write(1, MESSAGE);
    if bytes_written != MESSAGE.len() as isize {
        syscall::exit(1);
    }
}

user_entry!(main);
