#![no_std]
#![no_main]

//! 验证子进程通过 exec 替换映像，并由父进程回收退出状态。

pub fn main(_arguments: fullemu_user::UserArguments) {
    let child_pid = fullemu_user::syscall::fork();
    if child_pid < 0 {
        fullemu_user::println!("exec 测试 fork 失败：errno={}", -child_pid);
        fullemu_user::syscall::exit(1);
    }

    if child_pid == 0 {
        let missing_program = fullemu_user::syscall::exec("no_such_program", &[]);
        if missing_program != -2 {
            fullemu_user::println!("exec 错误路径返回异常：{}", missing_program);
            fullemu_user::syscall::exit(4);
        }
        fullemu_user::println!("exec 不存在的程序返回 ENOENT，当前程序继续运行");

        fullemu_user::println!("exec 测试子进程：即将替换为 argv_test");
        let result = fullemu_user::syscall::exec("argv_test", &["alpha", "two words"]);
        fullemu_user::println!("exec 失败：errno={}", -result);
        fullemu_user::syscall::exit(2);
    }

    let mut status = 0;
    let waited_pid = fullemu_user::syscall::waitpid(child_pid as i32, Some(&mut status));
    if waited_pid != child_pid || status != 23 << 8 {
        fullemu_user::println!("exec 验证失败：等待 PID={}，状态={:#x}", waited_pid, status);
        fullemu_user::syscall::exit(3);
    }

    fullemu_user::println!(
        "exec 验证通过：子进程 PID={} 替换映像后以状态码 23 退出",
        waited_pid
    );
}

fullemu_user::user_entry!(main);
