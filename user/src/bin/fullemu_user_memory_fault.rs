#![no_std]
#![no_main]

//! 尝试读取只允许 S-mode 访问的内核页面，观察 Sv39 页故障。

pub fn main() {
    fullemu_user::println!("内存保护示例：尝试从 U-mode 读取内核地址 0x80200000");

    // 安全性：此程序故意从 U-mode 读取 S-mode 页面；预期硬件触发加载页故障并陷入内核。
    let value = unsafe { core::ptr::read_volatile(0x8020_0000 as *const usize) };

    fullemu_user::println!("错误：非法访问意外成功，读到 {value:#x}");
    fullemu_user::syscall::exit(1);
}

fullemu_user::user_entry!(main);
