#![no_std]
#![no_main]

//! 提供最小交互式命令行，命令在用户态解析和执行。

const MAX_COMMAND_LENGTH: usize = 128;
const MAX_COMMAND_ARGUMENT_COUNT: usize = 16;
const BACKSPACE: &[u8] = b"\x08 \x08";
const BUILTIN_COMMANDS: &[&str] = &["help", "uptime", "exit"];

pub fn main(_arguments: fullemu_user::UserArguments) {
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

    let Ok(command) = core::str::from_utf8(command) else {
        fullemu_user::println!("shell: 命令必须是 UTF-8 文本");
        return;
    };
    let mut tokens = [""; MAX_COMMAND_ARGUMENT_COUNT + 1];
    let mut token_count = 0;
    for token in command.split_ascii_whitespace() {
        if token_count == tokens.len() {
            fullemu_user::println!("shell: 参数数量过多");
            return;
        }
        tokens[token_count] = token;
        token_count += 1;
    }
    let name = tokens[0];
    let arguments = &tokens[1..token_count];

    match name {
        "help" if arguments.is_empty() => print_commands(),
        "uptime" if arguments.is_empty() => print_uptime(),
        "exit" if arguments.is_empty() => fullemu_user::syscall::exit(0),
        "help" | "uptime" | "exit" => {
            fullemu_user::println!("shell: 内置命令不接受参数");
        }
        _ => run_program(name, arguments),
    }
}

fn print_commands() {
    fullemu_user::println!("内置命令：");
    for command in BUILTIN_COMMANDS {
        fullemu_user::println!("  {}", command);
    }
    fullemu_user::println!("直接输入程序名运行；参数以空格分隔，不支持引号和转义。");
}

fn run_program(program_name: &str, arguments: &[&str]) {
    if program_name.is_empty()
        || !program_name
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-'))
    {
        fullemu_user::println!("shell: 无效的程序名");
        return;
    }

    let child_pid = fullemu_user::syscall::fork();
    if child_pid < 0 {
        fullemu_user::println!("shell: fork 失败，errno={}", -child_pid);
        return;
    }
    if child_pid == 0 {
        let error = fullemu_user::syscall::exec(program_name, arguments);
        fullemu_user::println!("shell: 无法启动 {}，errno={}", program_name, -error);
        fullemu_user::syscall::exit(127);
    }

    let mut status = 0;
    let waited_pid = fullemu_user::syscall::waitpid(child_pid as i32, Some(&mut status));
    if waited_pid < 0 {
        fullemu_user::println!("shell: wait 子进程失败，errno={}", -waited_pid);
        return;
    }
    fullemu_user::println!("{} 已退出，wait 状态={:#x}", program_name, status);
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
