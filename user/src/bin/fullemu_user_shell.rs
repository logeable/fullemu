#![no_std]
#![no_main]

//! 提供最小交互式命令行，命令在用户态解析和执行。

const MAX_COMMAND_LENGTH: usize = 128;
const BACKSPACE: &[u8] = b"\x08 \x08";
const BUILTIN_COMMANDS: &[&str] = &["help", "uptime"];

pub fn main() {
    fullemu_user::println!("fullemu 用户态 shell");
    fullemu_user::println!("输入 help 查看内置命令。");

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
    match command {
        b"" => {}
        b"help" => print_commands(),
        b"uptime" => print_uptime(),
        _ => fullemu_user::println!("shell: 未知命令；输入 help 查看可用命令"),
    }
}

fn print_commands() {
    fullemu_user::println!("内置命令：");
    for command in BUILTIN_COMMANDS {
        fullemu_user::println!("  {}", command);
    }
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
