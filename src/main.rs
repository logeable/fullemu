#![no_std]
#![no_main]

//! 第一个启动阶段：从 OpenSBI 进入 Rust，并通过 QEMU 串口报告启动信息。

mod arch;

/// RISC-V 汇编入口完成栈和 `.bss` 初始化后调用此 Rust 入口。
/// OpenSBI 通过 `a0` 传入 hart ID，通过 `a1` 传入 DTB 地址；
/// 本阶段只运行一个 hart，并打印固件交接信息。
#[no_mangle]
pub extern "C" fn kernel_main(hart_id: usize, device_tree: usize) -> ! {
    arch::riscv64::console::write_str("fullemu: booted on QEMU virt (RISC-V)\n");
    arch::riscv64::console::write_str("hart: ");
    arch::riscv64::console::write_hex(hart_id);
    arch::riscv64::console::write_str("\nDTB:  ");
    arch::riscv64::console::write_hex(device_tree);
    arch::riscv64::console::write_str("\n");

    // 当前尚无调度器或关机服务。让启动 hart 保持运行，避免反复轮询设备
    // 或持续占用整个 CPU 核心。
    loop {
        // 安全性：WFI 只让当前 hart 等待中断，不访问内存，也不改变特权级。
        // 本阶段没有启用中断，因此可由宿主机直接停止 QEMU。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    arch::riscv64::console::write_str("\nfullemu: kernel panic\n");
    loop {
        // 安全性：与 `kernel_main` 中的空闲循环相同，此处等待是有效操作。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
