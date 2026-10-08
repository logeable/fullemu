#![no_std]
#![no_main]

//! 尝试读取另一任务的程序页面，观察独立页表的隔离效果。

const OTHER_PROGRAM_BASE: usize = 0x8040_0000;

pub fn main() {
    fullemu_user::println!(
        "内存隔离示例：尝试从 U-mode 读取另一程序的页面 {:#010x}",
        OTHER_PROGRAM_BASE
    );

    // 安全性：此程序故意读取另一任务的程序槽位；独立页表应使该地址在本任务中未映射。
    let value = unsafe { core::ptr::read_volatile(OTHER_PROGRAM_BASE as *const usize) };

    fullemu_user::println!("错误：跨任务访问意外成功，读到 {value:#x}");
    fullemu_user::syscall::exit(1);
}

fullemu_user::user_entry!(main);
