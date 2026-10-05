#![no_std]
#![no_main]

//! 编译为独立二进制镜像，由内核加载后在 U-mode 执行。

use core::arch::global_asm;

global_asm!(include_str!("start.S"));

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
