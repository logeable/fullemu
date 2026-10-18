#![no_std]
#![no_main]

//! 验证 PID 1 接管并回收父进程退出后留下的孤儿进程。

pub fn main() {
    let child_pid = fullemu_user::syscall::fork();
    if child_pid < 0 {
        fullemu_user::println!("孤儿回收测试 fork 失败：errno={}", -child_pid);
        fullemu_user::syscall::exit(1);
    }

    if child_pid == 0 {
        let grandchild_pid = fullemu_user::syscall::fork();
        if grandchild_pid < 0 {
            fullemu_user::println!("孤儿回收测试再次 fork 失败：errno={}", -grandchild_pid);
            fullemu_user::syscall::exit(2);
        }
        if grandchild_pid == 0 {
            for _ in 0..3 {
                fullemu_user::syscall::sched_yield();
            }
            fullemu_user::println!("孙进程：中间父进程已退出，我现在退出");
            fullemu_user::syscall::exit(17);
        }

        fullemu_user::println!("中间父进程：已创建孙进程，现先退出");
        fullemu_user::syscall::exit(0);
    }

    let mut child_status = 0;
    let waited_child = fullemu_user::syscall::waitpid(-1, Some(&mut child_status));
    let mut orphan_status = 0;
    let waited_orphan = fullemu_user::syscall::waitpid(-1, Some(&mut orphan_status));
    if waited_child != child_pid
        || child_status != 0
        || waited_orphan <= 0
        || waited_orphan == child_pid
        || orphan_status != 17 << 8
    {
        fullemu_user::println!(
            "孤儿回收验证失败：子进程 PID/状态={}/{:#x}，孤儿 PID/状态={}/{:#x}",
            waited_child,
            child_status,
            waited_orphan,
            orphan_status
        );
        fullemu_user::syscall::exit(3);
    }

    fullemu_user::println!(
        "孤儿回收验证通过：PID 1 回收了子进程 {} 和孤儿进程 {}",
        waited_child,
        waited_orphan
    );
}

fullemu_user::user_entry!(main);
