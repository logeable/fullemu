#![no_std]
#![no_main]

//! 编译为独立二进制镜像，由内核加载后在 U-mode 执行。

use core::arch::global_asm;

mod syscall;

global_asm!(include_str!("start.S"));

const USER_PROGRAM_MARKER: usize = 0x5553_4552;
const MESSAGE: &[u8] = b"Hello from an independent U-mode program via write syscall!\n";

#[repr(C)]
struct KernelMemoryProbe {
    private_value: usize,
    user_read_value: usize,
    user_written_value: usize,
    write_result: isize,
}

/// 演示独立用户程序对内核探针的访问并通过 write 系统调用输出文本。
#[no_mangle]
pub extern "C" fn user_main(probe_address: usize) {
    let probe = probe_address as *mut KernelMemoryProbe;

    // 安全性：当前阶段 satp=BARE，实验明确允许用户程序访问传入的内核探针地址。
    unsafe {
        let private_value = core::ptr::read_volatile(core::ptr::addr_of!((*probe).private_value));
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!((*probe).user_read_value),
            private_value,
        );
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!((*probe).private_value),
            USER_PROGRAM_MARKER,
        );
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!((*probe).user_written_value),
            USER_PROGRAM_MARKER,
        );
    }

    let write_result = syscall::write(1, MESSAGE);

    // 安全性：此阶段的内存映射为 BARE，探针指针仍可直接访问；后续 Sv39 阶段会移除这种能力。
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*probe).write_result), write_result);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
