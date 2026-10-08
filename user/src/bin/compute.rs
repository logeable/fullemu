#![no_std]
#![no_main]

//! 计算一组整数的平方和，并通过格式化输出报告结果。

pub fn main() {
    let mut square_sum = 0u64;
    for value in 1..=10_000u64 {
        square_sum += value * value;
    }

    fullemu_user::println!("计算示例：1 到 10000 的平方和为 {}", square_sum);
}

fullemu_user::user_entry!(main);
