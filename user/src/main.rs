#![no_std]
#![no_main]

//! 编译为独立二进制镜像，由内核加载后在 U-mode 执行。

use core::arch::global_asm;

mod syscall;

global_asm!(include_str!("start.S"));

const MESSAGE: &[u8] = b"Hello from an independent U-mode program via write syscall!\n";

/// 通过 Linux RISC-V 系统调用输出文本并正常结束当前任务。
#[no_mangle]
pub extern "C" fn user_main() -> ! {
    let _bytes_written = syscall::write(1, MESSAGE);
    syscall::exit(0)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
