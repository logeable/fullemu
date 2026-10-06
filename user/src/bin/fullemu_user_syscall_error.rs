#![no_std]
#![no_main]

//! 验证用户程序可以识别系统调用错误并继续执行。

use fullemu_user::{syscall, user_entry};

const INVALID_FILE_DESCRIPTOR: usize = 3;
const EBADF: isize = -9;
const SUCCESS_MESSAGE: &[u8] = b"[error] Invalid fd returned EBADF; program continued.\n";
const FAILURE_MESSAGE: &[u8] = b"[error] Invalid fd did not return EBADF.\n";

/// 提交无效文件描述符，识别内核返回的 `EBADF` 后继续输出结果。
pub fn main() {
    let write_result = syscall::write(INVALID_FILE_DESCRIPTOR, b"unreachable output");
    let (message, expected_result) = if write_result == EBADF {
        (SUCCESS_MESSAGE, true)
    } else {
        (FAILURE_MESSAGE, false)
    };

    let bytes_written = syscall::write(1, message);
    if !expected_result || bytes_written != message.len() as isize {
        syscall::exit(3);
    }
}

user_entry!(main);
