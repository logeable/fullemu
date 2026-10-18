#![no_std]
#![no_main]

//! 提供最小交互式命令行，命令在用户态解析和执行。

const MAX_COMMAND_LENGTH: usize = 128;
const BACKSPACE: &[u8] = b"\x08 \x08";
const BUILTIN_COMMANDS: &[&str] = &["help", "uptime", "exit"];

pub fn main() {
    fullemu_user::println!("fullemu 用户态 shell");
    fullemu_user::println!("输入 help 查看内置命令，或输入程序名运行嵌入式程序。");

    let mut command = [0u8; MAX_COMMAND_LENGTH];
    loop {
        fullemu_user::print!("fullemu> ");
        let length = read_command(&mut command);
        execute_command(&command[..length]);
    }
}

fn read_command(buffer: &mut [u8]) -> usize {
    let mut length = 0;

    loop {
        let mut input = [0u8; 1];
        let result = fullemu_user::syscall::read(0, &mut input);
        if result != 1 {
            fullemu_user::println!("shell: 读取输入失败（返回值 {}）", result);
            return 0;
        }

        match input[0] {
            b'\r' | b'\n' => {
                fullemu_user::println!();
                return length;
            }
            0x08 | 0x7f if length > 0 => {
                length -= 1;
                let _ = fullemu_user::syscall::write(1, BACKSPACE);
            }
            0x08 | 0x7f => {}
            byte if length < buffer.len() => {
                buffer[length] = byte;
                length += 1;
                let _ = fullemu_user::syscall::write(1, &input);
            }
            _ => {}
        }
    }
}

fn execute_command(command: &[u8]) {
    let command = trim_ascii_whitespace(command);
    if command.is_empty() {
        return;
    }

    let name_length = command
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(command.len());
    let name = &command[..name_length];
    let arguments = trim_ascii_whitespace(&command[name_length..]);
    if !arguments.is_empty() {
        fullemu_user::println!("shell: 当前不支持命令参数");
        return;
    }

    match name {
        b"help" => print_commands(),
        b"uptime" => print_uptime(),
        b"exit" => fullemu_user::syscall::exit(0),
        _ => run_program(name),
    }
}

fn print_commands() {
    fullemu_user::println!("内置命令：");
    for command in BUILTIN_COMMANDS {
        fullemu_user::println!("  {}", command);
    }
    fullemu_user::println!("直接输入嵌入式程序名运行，例如 compute；当前不支持参数。");
}

fn run_program(program_name: &[u8]) {
    if program_name.is_empty()
        || !program_name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-'))
    {
        fullemu_user::println!("shell: 无效的程序名");
        return;
    }

    let mut path = [0u8; 128];
    let prefix = b"/bin/";
    let path_length = prefix.len() + program_name.len();
    if path_length >= path.len() {
        fullemu_user::println!("shell: 程序名过长");
        return;
    }
    path[..prefix.len()].copy_from_slice(prefix);
    path[prefix.len()..path_length].copy_from_slice(program_name);
    let Ok(path) = core::str::from_utf8(&path[..path_length]) else {
        fullemu_user::println!("shell: 程序名不是有效的 ASCII 文本");
        return;
    };

    let child_pid = fullemu_user::syscall::fork();
    if child_pid < 0 {
        fullemu_user::println!("shell: fork 失败，errno={}", -child_pid);
        return;
    }
    if child_pid == 0 {
        let error = fullemu_user::syscall::exec(path);
        fullemu_user::println!("shell: 无法启动 {}，errno={}", path, -error);
        fullemu_user::syscall::exit(127);
    }

    let mut status = 0;
    let waited_pid = fullemu_user::syscall::waitpid(child_pid as i32, Some(&mut status));
    if waited_pid < 0 {
        fullemu_user::println!("shell: wait 子进程失败，errno={}", -waited_pid);
        return;
    }
    fullemu_user::println!("{} 已退出，wait 状态={:#x}", path, status);
}

fn print_uptime() {
    let mut time = fullemu_user::syscall::Timespec {
        seconds: 0,
        nanoseconds: 0,
    };
    if fullemu_user::syscall::clock_gettime(&mut time) < 0 {
        fullemu_user::println!("uptime: 无法读取单调时钟");
        return;
    }

    fullemu_user::println!(
        "运行时间：{}.{:03} 秒",
        time.seconds,
        time.nanoseconds / 1_000_000
    );
}

fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fullemu_user::user_entry!(main);
