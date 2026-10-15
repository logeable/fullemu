#![no_std]
#![no_main]

//! 尝试从 U-mode 读取 S-mode 专用的镜像物理槽位，观察独立页表的隔离效果。

const SUPERVISOR_IMAGE_SLOT_ADDRESS: usize = 0x8041_0000;

pub fn main() {
    fullemu_user::println!(
        "内存隔离示例：尝试从 U-mode 读取 S-mode 专用的镜像物理槽位 {:#010x}",
        SUPERVISOR_IMAGE_SLOT_ADDRESS
    );

    // 安全性：此程序故意读取仅以 S-mode 权限映射的物理镜像槽；U-mode 访问应触发页故障。
    let value = unsafe { core::ptr::read_volatile(SUPERVISOR_IMAGE_SLOT_ADDRESS as *const usize) };

    fullemu_user::println!("错误：跨任务访问意外成功，读到 {value:#x}");
    fullemu_user::syscall::exit(1);
}

fullemu_user::user_entry!(main);
