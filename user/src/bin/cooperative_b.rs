#![no_std]
#![no_main]

//! 每输出一步就主动让出 CPU，和其他协作任务交替运行。

use fullemu_user::syscall;

const MESSAGES: [&[u8]; 3] = [
    "协作任务 B：第 1 步\n".as_bytes(),
    "协作任务 B：第 2 步\n".as_bytes(),
    "协作任务 B：第 3 步\n".as_bytes(),
];

pub fn main() {
    for message in MESSAGES {
        if syscall::write(1, message) != message.len() as isize {
            syscall::exit(1);
        }
        if syscall::sched_yield() < 0 {
            syscall::exit(1);
        }
    }
}

fullemu_user::user_entry!(main);
