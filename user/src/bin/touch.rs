#![no_std]
#![no_main]

//! 使用 `openat` 创建文件；已存在的文件保持原有内容。

pub fn main(mut arguments: fullemu_user::UserArguments) -> i32 {
    // 初始参数向量遵循 Linux 约定，首项是程序名。
    let _program_name = arguments.next();
    let Some(first_argument) = arguments.next() else {
        fullemu_user::println!("用法：touch <文件>...");
        return 2;
    };
    let mut exit_code = 0;
    for argument in core::iter::once(first_argument).chain(arguments) {
        let Ok(path) = core::str::from_utf8(argument) else {
            fullemu_user::println!("touch: 路径必须是 UTF-8 文本");
            exit_code = 1;
            continue;
        };
        let descriptor = fullemu_user::syscall::open(
            path,
            fullemu_user::syscall::O_CREAT | fullemu_user::syscall::O_WRONLY,
            0o644,
        );
        if descriptor < 0 {
            fullemu_user::println!("touch: 创建 {} 失败，errno={}", path, -descriptor);
            exit_code = 1;
        } else if fullemu_user::syscall::close(descriptor as usize) < 0 {
            fullemu_user::println!("touch: 关闭 {} 的文件描述符失败", path);
            exit_code = 1;
        }
    }
    exit_code
}

fullemu_user::user_entry!(main);
