//! 展示任务主动让出 CPU 时，单 hart 如何在执行上下文之间切换。

use crate::arch::riscv64::context::TaskContext;

const TASK_COUNT: usize = 2;
const STACK_SIZE: usize = 16 * 1024;
const REPORT_INTERVAL_MASK: u64 = (1 << 22) - 1;

#[repr(align(16))]
struct TaskStack([u8; STACK_SIZE]);

static mut TASK_CONTEXTS: [TaskContext; TASK_COUNT] = [TaskContext::empty(); TASK_COUNT];
static mut BOOT_CONTEXT: TaskContext = TaskContext::empty();
static mut CURRENT_TASK: usize = 0;
static mut TASK_A_STACK: TaskStack = TaskStack([0; STACK_SIZE]);
static mut TASK_B_STACK: TaskStack = TaskStack([0; STACK_SIZE]);

/// 启动两个各自使用静态栈的内核任务，并通过显式让出操作轮流运行。
pub fn run_cooperatively() -> ! {
    println!("\nCPU 虚拟化实验：协作式任务切换");
    println!("任务 A 和任务 B 会在计数达到阈值时主动让出 CPU");

    initialize_contexts();

    // 安全性：启动过程只调用一次；启动栈和两个任务栈均保持有效，且上下文指针互不重叠。
    unsafe {
        let boot_context = core::ptr::addr_of_mut!(BOOT_CONTEXT);
        let task_contexts = core::ptr::addr_of!(TASK_CONTEXTS).cast::<TaskContext>();
        crate::arch::riscv64::context::switch_context(boot_context, task_contexts);
    }

    panic!("协作式调度意外返回启动上下文");
}

fn initialize_contexts() {
    // 安全性：此初始化只在单 hart 启动阶段执行一次；栈数组是静态存储，且大小为 16 字节的倍数。
    unsafe {
        let task_a_stack = core::ptr::addr_of_mut!(TASK_A_STACK.0).cast::<u8>();
        let task_b_stack = core::ptr::addr_of_mut!(TASK_B_STACK.0).cast::<u8>();
        let task_a_stack_top = task_a_stack.add(STACK_SIZE) as usize;
        let task_b_stack_top = task_b_stack.add(STACK_SIZE) as usize;
        let task_contexts = core::ptr::addr_of_mut!(TASK_CONTEXTS).cast::<TaskContext>();

        task_contexts.write(TaskContext::for_entry(task_a_entry, task_a_stack_top));
        task_contexts
            .add(1)
            .write(TaskContext::for_entry(task_b_entry, task_b_stack_top));
        core::ptr::addr_of_mut!(CURRENT_TASK).write(0);
    }
}

extern "C" fn task_a_entry() -> ! {
    let mut count = 0_u64;
    loop {
        count = count.wrapping_add(1);
        if count & REPORT_INTERVAL_MASK == 0 {
            println!("任务 A：运行计数 {count}");
        }
        yield_now();
    }
}

extern "C" fn task_b_entry() -> ! {
    let mut count = 0_u64;
    loop {
        count = count.wrapping_add(1);
        if count & REPORT_INTERVAL_MASK == 0 {
            println!("任务 B：运行计数 {count}");
        }
        yield_now();
    }
}

fn yield_now() {
    // 安全性：只有当前 hart 调用此函数，当前不启用中断；轮转索引保证两个上下文指针不同。
    unsafe {
        let current_task = core::ptr::addr_of!(CURRENT_TASK).read();
        let next_task = (current_task + 1) % TASK_COUNT;
        core::ptr::addr_of_mut!(CURRENT_TASK).write(next_task);

        let task_contexts = core::ptr::addr_of_mut!(TASK_CONTEXTS).cast::<TaskContext>();
        crate::arch::riscv64::context::switch_context(
            task_contexts.add(current_task),
            task_contexts.add(next_task),
        );
    }
}
