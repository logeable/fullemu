#![no_std]
#![no_main]

//! 读取 ELF 只读段中的静态数据，再尝试写入并观察页权限故障。

static READ_ONLY_VALUE: u64 = 0x1234_5678_9abc_def0;

pub fn main() {
    fullemu_user::println!("ELF 权限示例：只读数据为 {:#x}", READ_ONLY_VALUE);
    fullemu_user::println!("现在尝试写入 ELF 只读数据，应触发 StoreAmoPageFault");

    // 安全性：本程序用汇编故意写入 ELF 只读段，预期由页表触发 StoreAmoPageFault。
    unsafe {
        core::arch::asm!(
            "sd zero, 0({address})",
            address = in(reg) core::ptr::addr_of!(READ_ONLY_VALUE),
            options(nostack)
        );
    }
}

fullemu_user::user_entry!(main);
