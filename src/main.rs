#![no_std]
#![no_main]

//! 提供 RISC-V 汇编调用的 Rust 入口和内核 panic 处理器。

extern crate alloc;

mod arch;
mod device;
mod kernel;

/// RISC-V 汇编完成栈和 `.bss` 初始化后，将 hart ID 与 FDT 地址传入此入口。
#[no_mangle]
pub extern "C" fn kernel_main(hart_id: usize, device_tree: usize) -> ! {
    kernel::startup::run(hart_id, device_tree)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    crate::klog_error!("内核 panic：{info}");
    loop {
        // 安全性：与 `kernel_main` 中的空闲循环相同，此处等待是有效操作。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
