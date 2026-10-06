#![no_std]
#![no_main]

//! 执行非法指令，观察内核记录并终止当前 U-mode 任务。

pub fn main() {
    fullemu_user::println!("异常示例：即将执行非法指令");

    // 安全性：此演示故意执行零编码的非法指令；预期内核将当前任务标记为异常终止。
    unsafe {
        core::arch::asm!(".word 0", options(nomem, nostack));
    }

    fullemu_user::println!("错误：非法指令没有触发异常");
    fullemu_user::syscall::exit(1);
}

fullemu_user::user_entry!(main);
