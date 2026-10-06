#![no_std]
#![no_main]

//! 验证用户程序可以识别系统调用错误并继续执行。

use core::arch::global_asm;

#[path = "../syscall.rs"]
mod syscall;

global_asm!(include_str!("../start.S"));

const INVALID_FILE_DESCRIPTOR: usize = 3;
const EBADF: isize = -9;
const SUCCESS_MESSAGE: &[u8] = b"[error] Invalid fd returned EBADF; program continued.\n";
const FAILURE_MESSAGE: &[u8] = b"[error] Invalid fd did not return EBADF.\n";

/// 提交无效文件描述符，识别内核返回的 `EBADF` 后继续输出结果。
#[no_mangle]
pub extern "C" fn user_main() -> ! {
    let write_result = syscall::write(INVALID_FILE_DESCRIPTOR, b"unreachable output");
    let (message, expected_result) = if write_result == EBADF {
        (SUCCESS_MESSAGE, true)
    } else {
        (FAILURE_MESSAGE, false)
    };

    let bytes_written = syscall::write(1, message);
    let exit_status = if expected_result && bytes_written == message.len() as isize {
        0
    } else {
        3
    };
    syscall::exit(exit_status)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
