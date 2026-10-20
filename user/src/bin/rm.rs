#![no_std]
#![no_main]

//! 使用 `unlinkat` 删除普通文件。

pub fn main(mut arguments: fullemu_user::UserArguments) -> i32 {
    // 初始参数向量遵循 Linux 约定，首项是程序名。
    let _program_name = arguments.next();
    let Some(first_argument) = arguments.next() else {
        fullemu_user::println!("用法：rm <文件>...");
        return 2;
    };
    let mut exit_code = 0;
    for argument in core::iter::once(first_argument).chain(arguments) {
        let Ok(path) = core::str::from_utf8(argument) else {
            fullemu_user::println!("rm: 路径必须是 UTF-8 文本");
            exit_code = 1;
            continue;
        };
        let result = fullemu_user::syscall::unlink(path);
        if result < 0 {
            fullemu_user::println!("rm: 删除 {} 失败，errno={}", path, -result);
            exit_code = 1;
        }
    }
    exit_code
}

fullemu_user::user_entry!(main);
