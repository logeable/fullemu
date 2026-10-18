#![no_std]
#![no_main]

//! 作为 exec 测试目标，验证内核装入了另一份程序映像。

pub fn main() -> i32 {
    fullemu_user::println!("exec_target：新映像已开始运行");
    23
}

fullemu_user::user_entry!(main);
