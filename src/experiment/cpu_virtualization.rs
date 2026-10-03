//! 展示没有 CPU 调度时，一个持续运行的执行流会独占单个 hart。

const REPORT_INTERVAL_MASK: u64 = (1 << 26) - 1;

/// 直接运行任务 A，再运行任务 B；任务 A 永不返回，因此任务 B 不会开始。
pub fn run_without_scheduling() -> ! {
    println!("\nCPU 虚拟化对照：当前没有任务调度器");
    println!("任务 A 将持续计算且不会主动让出 CPU");
    println!("如果任务 B 获得运行机会，它会定期输出心跳");

    task_a();
    task_b()
}

fn task_a() {
    println!("任务 A：开始持续计算");

    let mut count = 0_u64;
    loop {
        count = count.wrapping_add(1);
        if count & REPORT_INTERVAL_MASK == 0 {
            println!("任务 A：仍在运行，计数 {count}");
        }
        core::hint::spin_loop();
    }
}

fn task_b() -> ! {
    let mut count = 0_u64;
    loop {
        count = count.wrapping_add(1);
        if count & REPORT_INTERVAL_MASK == 0 {
            println!("任务 B：心跳 {count}");
        }
        core::hint::spin_loop();
    }
}
