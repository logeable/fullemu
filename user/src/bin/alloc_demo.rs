#![no_std]
#![no_main]

//! 展示用户态 `alloc` 堆如何支持 Rust 常用拥有型容器。

use fullemu_user::alloc::{boxed::Box, string::String, vec::Vec};

pub fn main(_arguments: fullemu_user::UserArguments) -> i32 {
    let mut values = Vec::new();
    for value in 0..256_u64 {
        values.push(value * value);
    }

    let mut message = String::from("用户态 Rust 堆");
    message.push_str(" 已支持 String、Vec 和 Box");
    let boxed_value = Box::new(values[255]);
    let valid = *boxed_value == 255 * 255 && values.len() == 256;

    fullemu_user::println!(
        "{}；Vec 长度={}，容量={}，Box 值={}",
        message,
        values.len(),
        values.capacity(),
        boxed_value
    );

    let child_pid = fullemu_user::syscall::fork();
    if child_pid < 0 {
        fullemu_user::println!("alloc_demo: fork 失败，errno={}", -child_pid);
        return 1;
    }
    if child_pid == 0 {
        for value in 256..1024_u64 {
            values.push(value * value);
        }
        let exit_code = if values.len() == 1024 && values[1023] == 1023 * 1023 {
            0
        } else {
            1
        };
        fullemu_user::syscall::exit(exit_code);
    }

    let child_pid = child_pid as i32;
    let mut status = 0;
    if fullemu_user::syscall::waitpid(child_pid, Some(&mut status)) != child_pid as isize
        || status != 0
        || values.len() != 256
        || values[255] != 255 * 255
    {
        fullemu_user::println!("alloc_demo: fork 后的堆分配影响了父进程");
        return 1;
    }

    drop(boxed_value);
    drop(values);
    drop(message);

    let mut reused_values = Vec::with_capacity(64);
    reused_values.extend(0..64_u32);
    if reused_values.len() != 64 || reused_values[63] != 63 || !valid {
        return 1;
    }

    fullemu_user::println!("容器释放后可复用堆空间");
    0
}

fullemu_user::user_entry!(main);
