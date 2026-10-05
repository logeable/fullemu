//! 展示定时器中断如何抢占不主动让出 CPU 的任务。

use crate::arch::riscv64::trap::TrapFrame;
use core::sync::atomic::{AtomicUsize, Ordering};

const TASK_COUNT: usize = 2;
const STACK_SIZE: usize = 16 * 1024;
// QEMU virt 的 time 计数器单位；100,000 个计数约为 10 ms。
const TIMER_INTERVAL: u64 = 100_000;
const REPORT_INTERVAL: usize = 25;
const SUPERVISOR_TIMER_INTERRUPT: usize = (1usize << (usize::BITS - 1)) | 5;
const SUPERVISOR_TIMER_ENABLE: usize = 1 << 5;

#[repr(align(16))]
struct TaskStack([u8; STACK_SIZE]);

static mut TASK_FRAME_POINTERS: [*mut TrapFrame; TASK_COUNT] = [core::ptr::null_mut(); TASK_COUNT];
static mut CURRENT_TASK: usize = 0;
static mut TIMER_TICKS: usize = 0;
static TASK_A_PROGRESS: AtomicUsize = AtomicUsize::new(0);
static TASK_B_PROGRESS: AtomicUsize = AtomicUsize::new(0);
static mut TASK_A_STACK: TaskStack = TaskStack([0; STACK_SIZE]);
static mut TASK_B_STACK: TaskStack = TaskStack([0; STACK_SIZE]);

/// 启动两个内核任务，并由 S-mode 定时器中断轮转调度。
pub fn run_preemptively() -> ! {
    println!("\nCPU 虚拟化实验：定时器抢占");
    println!("任务 A 不主动让出 CPU；定时器中断应让任务 B 持续取得运行机会");

    initialize_task_frames();
    arm_timer().unwrap_or_else(|error| {
        println!("无法设置 SBI 定时器：错误码 {}", error.0);
        stop_forever();
    });

    // 安全性：单 hart 启动阶段仅设置 supervisor timer interrupt 使能位；
    // 全局 SIE 仍关闭，直到首个任务的 sret 按初始化陷入帧打开中断。
    unsafe {
        core::arch::asm!(
            "csrs sie, {mask}",
            mask = in(reg) SUPERVISOR_TIMER_ENABLE,
            options(nomem, nostack)
        );
    }

    // 安全性：初始化阶段构造了对齐、有效且生命周期为静态的首个任务陷入帧。
    unsafe {
        let first_frame = core::ptr::addr_of!(TASK_FRAME_POINTERS).read()[0];
        crate::arch::riscv64::trap::start_first_task(first_frame)
    }
}

fn initialize_task_frames() {
    // 安全性：仅在启动时单次调用；两个静态栈不重叠，陷入帧位于各自栈顶下方并保持 16 字节对齐。
    unsafe {
        let task_a_stack = core::ptr::addr_of_mut!(TASK_A_STACK.0).cast::<u8>();
        let task_b_stack = core::ptr::addr_of_mut!(TASK_B_STACK.0).cast::<u8>();
        let task_a_stack_top = task_a_stack.add(STACK_SIZE) as usize;
        let task_b_stack_top = task_b_stack.add(STACK_SIZE) as usize;
        let frame_size = core::mem::size_of::<TrapFrame>();
        let task_a_frame = (task_a_stack_top - frame_size) as *mut TrapFrame;
        let task_b_frame = (task_b_stack_top - frame_size) as *mut TrapFrame;

        task_a_frame.write(TrapFrame::for_entry(task_a_entry, task_a_stack_top));
        task_b_frame.write(TrapFrame::for_entry(task_b_entry, task_b_stack_top));
        core::ptr::addr_of_mut!(TASK_FRAME_POINTERS).write([task_a_frame, task_b_frame]);
        core::ptr::addr_of_mut!(CURRENT_TASK).write(0);
        core::ptr::addr_of_mut!(TIMER_TICKS).write(0);
    }
    TASK_A_PROGRESS.store(0, Ordering::Relaxed);
    TASK_B_PROGRESS.store(0, Ordering::Relaxed);
}

extern "C" fn task_a_entry() -> ! {
    loop {
        TASK_A_PROGRESS.fetch_add(1, Ordering::Relaxed);
    }
}

extern "C" fn task_b_entry() -> ! {
    loop {
        TASK_B_PROGRESS.fetch_add(1, Ordering::Relaxed);
    }
}

/// 保存当前任务陷入帧，并返回调度器选择的下一任务陷入帧。
#[no_mangle]
pub extern "C" fn supervisor_trap_handler(frame: *mut TrapFrame) -> *mut TrapFrame {
    // 安全性：汇编入口在当前任务栈上创建完整 TrapFrame，并以其地址调用本函数。
    let cause = unsafe { (*frame).cause };
    if cause != SUPERVISOR_TIMER_INTERRUPT {
        report_unexpected_trap(frame);
    }

    if let Err(error) = arm_timer() {
        println!("\n重设 SBI 定时器失败：错误码 {}", error.0);
        stop_forever();
    }

    // 安全性：本实验只有一个 hart；陷入处理期间 SIE 自动关闭，因此调度状态不会并发修改。
    unsafe {
        let current_task = core::ptr::addr_of!(CURRENT_TASK).read();
        let task_frames = core::ptr::addr_of_mut!(TASK_FRAME_POINTERS);
        (*task_frames)[current_task] = frame;

        let next_task = (current_task + 1) % TASK_COUNT;
        core::ptr::addr_of_mut!(CURRENT_TASK).write(next_task);
        let ticks = core::ptr::addr_of_mut!(TIMER_TICKS);
        *ticks = (*ticks).wrapping_add(1);

        if *ticks % REPORT_INTERVAL == 0 {
            println!(
                "定时器 tick {}：切换到任务 {}，A 进度 {}，B 进度 {}",
                *ticks,
                if next_task == 0 { "A" } else { "B" },
                TASK_A_PROGRESS.load(Ordering::Relaxed),
                TASK_B_PROGRESS.load(Ordering::Relaxed)
            );
        }

        (*task_frames)[next_task]
    }
}

fn arm_timer() -> Result<(), crate::arch::riscv64::sbi::SbiError> {
    let deadline = crate::arch::riscv64::sbi::read_time().wrapping_add(TIMER_INTERVAL);
    crate::arch::riscv64::sbi::set_timer(deadline)
}

fn report_unexpected_trap(frame: *mut TrapFrame) -> ! {
    // 安全性：调用者传入汇编入口构造的有效陷入帧；处理期间中断关闭，帧不会被覆盖。
    let (cause, exception_pc, trap_value) =
        unsafe { ((*frame).cause, (*frame).exception_pc, (*frame).trap_value) };
    println!("\nfullemu: 非预期 S-mode 陷入");
    println!("scause: {cause:#018x}");
    println!("sepc:   {exception_pc:#018x}");
    println!("stval:  {trap_value:#018x}");
    stop_forever()
}

fn stop_forever() -> ! {
    loop {
        // 安全性：错误无法恢复；WFI 只让单 hart 等待，不改变内存或特权状态。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
