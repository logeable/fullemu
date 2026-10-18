#![no_std]
#![no_main]

//! 验证 execve 将 argv 字符串和 argv[0] 放入新程序初始用户栈。

pub fn main(arguments: fullemu_user::UserArguments) -> i32 {
    let mut arguments = arguments;
    let expected = [b"argv_test".as_slice(), b"alpha", b"two words"];
    let mut received_count = 0;

    for (received, expected) in arguments.by_ref().zip(expected) {
        fullemu_user::println!("argv[{}] = {}", received_count, display_bytes(received));
        if received != expected {
            fullemu_user::println!("argv 验证失败：参数内容不匹配");
            return 24;
        }
        received_count += 1;
    }

    if received_count != expected.len() || arguments.next().is_some() {
        fullemu_user::println!("argv 验证失败：参数数量不匹配");
        return 24;
    }

    fullemu_user::println!("argv 验证通过");
    23
}

fn display_bytes(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("<非 UTF-8 参数>")
}

fullemu_user::user_entry!(main);
