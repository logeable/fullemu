#![no_std]
#![no_main]

//! 使用 `openat` 和 `getdents64` 列出根目录或指定目录的内容。

const DIRECTORY_BUFFER_SIZE: usize = 512;

pub fn main(mut arguments: fullemu_user::UserArguments) -> i32 {
    // 初始参数向量遵循 Linux 约定，首项是程序名。
    let _program_name = arguments.next();
    let Some(path) = arguments.next() else {
        return list_directory("/");
    };
    let mut exit_code = list_path(path);
    for path in arguments {
        if list_path(path) != 0 {
            exit_code = 1;
        }
    }
    exit_code
}

fn list_path(path: &[u8]) -> i32 {
    let Ok(path) = core::str::from_utf8(path) else {
        fullemu_user::println!("ls: 路径必须是 UTF-8 文本");
        return 1;
    };
    list_directory(path)
}

fn list_directory(path: &str) -> i32 {
    let descriptor = fullemu_user::syscall::open(
        path,
        fullemu_user::syscall::O_RDONLY | fullemu_user::syscall::O_DIRECTORY,
        0,
    );
    if descriptor < 0 {
        fullemu_user::println!("ls: 打开 {} 失败，errno={}", path, -descriptor);
        return 1;
    }

    let mut buffer = [0; DIRECTORY_BUFFER_SIZE];
    loop {
        let count = fullemu_user::syscall::getdents64(descriptor as usize, &mut buffer);
        if count < 0 {
            fullemu_user::println!("ls: 读取 {} 失败，errno={}", path, -count);
            let _ = fullemu_user::syscall::close(descriptor as usize);
            return 1;
        }
        if count == 0 {
            break;
        }

        let mut offset = 0;
        while offset < count as usize {
            let remaining = count as usize - offset;
            if remaining < 20 {
                fullemu_user::println!("ls: 内核返回了不完整的目录记录");
                let _ = fullemu_user::syscall::close(descriptor as usize);
                return 1;
            }
            let record_length =
                u16::from_le_bytes([buffer[offset + 16], buffer[offset + 17]]) as usize;
            if record_length < 20 || record_length > remaining {
                fullemu_user::println!("ls: 内核返回了无效的目录记录长度");
                let _ = fullemu_user::syscall::close(descriptor as usize);
                return 1;
            }

            let name_bytes = &buffer[offset + 19..offset + record_length];
            let name_length = name_bytes
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(name_bytes.len());
            match core::str::from_utf8(&name_bytes[..name_length]) {
                Ok(name) => fullemu_user::println!("{}", name),
                Err(_) => fullemu_user::println!("<非 UTF-8 文件名>"),
            }
            offset += record_length;
        }
    }

    if fullemu_user::syscall::close(descriptor as usize) < 0 {
        fullemu_user::println!("ls: 关闭 {} 的目录描述符失败", path);
        return 1;
    }
    0
}

fullemu_user::user_entry!(main);
