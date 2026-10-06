#![no_std]
#![no_main]

//! 编译为独立二进制镜像，由内核加载后在 U-mode 执行。

use core::arch::global_asm;

#[path = "../syscall.rs"]
mod syscall;

global_asm!(include_str!("../start.S"));

const MESSAGE: &[u8] = b"[stdout] Hello from the first user program.\n";

/// 通过标准输出发送问候，并检查 `write` 返回的字节数。
#[no_mangle]
pub extern "C" fn user_main() -> ! {
    let bytes_written = syscall::write(1, MESSAGE);
    let exit_status = if bytes_written == MESSAGE.len() as isize {
        0
    } else {
        1
    };
    syscall::exit(exit_status)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
