#![no_std]
#![no_main]

//! 尝试读取统一入口映射之外的旧镜像槽位地址，观察独立页表的隔离效果。

const UNMAPPED_IMAGE_SLOT_ADDRESS: usize = 0x8041_0000;

pub fn main() {
    fullemu_user::println!(
        "内存隔离示例：尝试从 U-mode 读取未映射的镜像槽位地址 {:#010x}",
        UNMAPPED_IMAGE_SLOT_ADDRESS
    );

    // 安全性：此程序故意读取统一程序映射之外的地址；独立页表应使该地址保持未映射。
    let value = unsafe { core::ptr::read_volatile(UNMAPPED_IMAGE_SLOT_ADDRESS as *const usize) };

    fullemu_user::println!("错误：跨任务访问意外成功，读到 {value:#x}");
    fullemu_user::syscall::exit(1);
}

fullemu_user::user_entry!(main);
