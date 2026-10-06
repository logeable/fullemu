#![no_std]
#![no_main]

//! 通过标准错误输出诊断文本，并检查系统调用返回值。

use core::arch::global_asm;

#[path = "../syscall.rs"]
mod syscall;

global_asm!(include_str!("../start.S"));

const MESSAGE: &[u8] = b"[stderr] This message uses file descriptor 2.\n";

/// 使用标准错误文件描述符输出诊断文本。
#[no_mangle]
pub extern "C" fn user_main() -> ! {
    let bytes_written = syscall::write(2, MESSAGE);
    let exit_status = if bytes_written == MESSAGE.len() as isize {
        0
    } else {
        2
    };
    syscall::exit(exit_status)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
