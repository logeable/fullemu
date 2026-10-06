#![no_std]
#![no_main]

//! 通过多次主动让出 CPU，观察任务 B 与其他用户任务交错执行。

use fullemu_user::{syscall, user_entry};

const MESSAGES: [&[u8]; 3] = [
    b"Task B: step 1\n",
    b"Task B: step 2\n",
    b"Task B: step 3\n",
];

/// 每轮输出后主动让出 CPU，全部轮次完成后结束任务。
pub fn main() {
    for message in MESSAGES {
        let bytes_written = syscall::write(1, message);
        if bytes_written != message.len() as isize {
            syscall::exit(1);
        }
        let _yield_result = syscall::sched_yield();
    }

}

user_entry!(main);
