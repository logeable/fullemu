#![no_std]
#![no_main]

//! 执行有限时长的 CPU 密集计算且不主动让出 CPU。

const START_MESSAGE: &[u8] = "CPU 密集任务：开始长时间计算，不主动让出 CPU。\n".as_bytes();
const FINISH_MESSAGE: &[u8] = "CPU 密集任务：计算完成，即将结束。\n".as_bytes();

pub fn main() {
    const ITERATIONS: u64 = 2_500_000_000;

    if fullemu_user::syscall::write(1, START_MESSAGE) != START_MESSAGE.len() as isize {
        fullemu_user::syscall::exit(1);
    }
    let mut checksum = 1u64;
    for iteration in 0..ITERATIONS {
        checksum = checksum
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(iteration);
    }
    if checksum == 0 {
        fullemu_user::syscall::exit(2);
    }
    if fullemu_user::syscall::write(1, FINISH_MESSAGE) != FINISH_MESSAGE.len() as isize {
        fullemu_user::syscall::exit(3);
    }
}

fullemu_user::user_entry!(main);
