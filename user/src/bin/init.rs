#![no_std]
#![no_main]

//! 作为 PID 1 启动并监督交互式 shell。

pub fn main(_arguments: fullemu_user::UserArguments) {
    fullemu_user::println!("fullemu init：启动用户态 shell");
    let mut shell_pid = start_shell();

    loop {
        let mut status = 0;
        let waited_pid = fullemu_user::syscall::waitpid(-1, Some(&mut status));
        if waited_pid < 0 {
            fullemu_user::println!("init: wait 子进程失败，errno={}", -waited_pid);
            fullemu_user::syscall::exit(1);
        }

        if waited_pid == shell_pid as isize {
            fullemu_user::println!(
                "init: shell PID={} 已退出，wait 状态={:#x}；重新启动 shell",
                waited_pid,
                status
            );
            shell_pid = start_shell();
        } else {
            fullemu_user::println!(
                "init: 已回收孤儿进程 PID={}，wait 状态={:#x}",
                waited_pid,
                status
            );
        }
    }
}

fn start_shell() -> isize {
    let child_pid = fullemu_user::syscall::fork();
    if child_pid < 0 {
        fullemu_user::println!("init: fork shell 失败，errno={}", -child_pid);
        fullemu_user::syscall::exit(1);
    }
    if child_pid == 0 {
        let error = fullemu_user::syscall::exec("shell", &[]);
        fullemu_user::println!("init: exec shell 失败，errno={}", -error);
        fullemu_user::syscall::exit(127);
    }
    child_pid
}

fullemu_user::user_entry!(main);
