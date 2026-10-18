#![no_std]
#![no_main]

//! 验证 fork 的父子返回值、地址空间复制、wait 阻塞和退出状态回收。

static mut FORK_MARKER: usize = 41;

pub fn main() {
    let child_pid = fullemu_user::syscall::fork();
    if child_pid < 0 {
        fullemu_user::println!("fork 失败：errno={}", -child_pid);
        fullemu_user::syscall::exit(1);
    }

    if child_pid == 0 {
        // 安全性：父子进程拥有独立的程序镜像页；子进程只修改自己的静态数据副本。
        unsafe { FORK_MARKER = 99 };
        fullemu_user::println!("fork 子进程：返回值=0，修改私有数据并以状态码 7 退出");
        fullemu_user::syscall::exit(7);
    }

    fullemu_user::println!("fork 父进程：子进程 PID={child_pid}，等待子进程退出");
    let mut status = 0;
    let waited_pid = fullemu_user::syscall::waitpid(child_pid as i32, Some(&mut status));
    if waited_pid != child_pid {
        fullemu_user::println!("waitpid 返回异常：{}", waited_pid);
        fullemu_user::syscall::exit(2);
    }

    // 安全性：读取本进程内的可写静态数据；预期父子独立地址空间使其未被子进程改动。
    let marker = unsafe { FORK_MARKER };
    if marker != 41 || status != (7 << 8) {
        fullemu_user::println!(
            "fork/wait 验证失败：静态数据={}，wait 状态={:#x}",
            marker,
            status
        );
        fullemu_user::syscall::exit(3);
    }

    fullemu_user::println!(
        "fork/wait 验证通过：子进程 PID={}，退出码={}，父进程数据仍为 {}",
        waited_pid,
        status >> 8,
        marker
    );
}

fullemu_user::user_entry!(main);
