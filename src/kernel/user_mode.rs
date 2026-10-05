//! 启动独立构建的 U-mode 程序，并观察特权限制和当前内存访问边界。

use crate::arch::riscv64::trap::{ExceptionCause, TrapCause, TrapFrame};

const STACK_SIZE: usize = 16 * 1024;
const INITIAL_KERNEL_VALUE: usize = 0x4B45_524E;
const USER_PROGRAM_MARKER: usize = 0x5553_4552;

#[repr(align(16))]
struct TaskStack([u8; STACK_SIZE]);

#[repr(C)]
/// 与用户程序入口约定的探针布局：三个 RV64 字长字段依次位于偏移 0、8、16。
struct KernelMemoryProbe {
    private_value: usize,
    user_read_value: usize,
    user_written_value: usize,
    write_result: isize,
}

static mut KERNEL_MEMORY_PROBE: KernelMemoryProbe = KernelMemoryProbe {
    private_value: INITIAL_KERNEL_VALUE,
    user_read_value: 0,
    user_written_value: 0,
    write_result: 0,
};
static mut USER_STACK: TaskStack = TaskStack([0; STACK_SIZE]);
static mut KERNEL_TRAP_STACK: TaskStack = TaskStack([0; STACK_SIZE]);

/// 加载独立用户程序，进入 U-mode 并观察它访问内核内存的结果。
pub fn run_privilege_boundary_demonstration() -> ! {
    println!("\nU-mode 实验：独立用户程序加载与特权边界");
    println!("当前 satp 使用 BARE；加载的程序仍未与内核内存隔离");

    // 安全性：仍处于 S-mode；本实验不使用定时器，清除 S-mode 外部中断源使能，避免异步陷入。
    unsafe {
        core::arch::asm!(
            "csrw sie, zero",
            "csrw satp, zero",
            "sfence.vma zero, zero",
            options(nostack)
        );
    }

    let program = match super::user_program::load() {
        Ok(program) => program,
        Err(error) => {
            println!(
                "用户程序加载失败：{}（镜像 {} 字节，加载区 {} 字节）",
                error.description(),
                error.image_size,
                error.capacity
            );
            stop_forever();
        }
    };
    println!(
        "已加载独立用户镜像：入口 {:#018x}，大小 {} 字节",
        program.entry, program.image_size
    );

    let user_stack_top = stack_top(core::ptr::addr_of_mut!(USER_STACK));
    let kernel_stack_top = stack_top(core::ptr::addr_of_mut!(KERNEL_TRAP_STACK));
    let kernel_probe = core::ptr::addr_of_mut!(KERNEL_MEMORY_PROBE) as usize;
    let initial_frame = TrapFrame::for_user_entry(program.entry, user_stack_top, kernel_probe);

    // 安全性：内核陷入栈是静态分配且按 16 字节对齐；U-mode 陷入入口会切换到该栈保存现场。
    unsafe { crate::arch::riscv64::trap::set_user_kernel_stack(kernel_stack_top) };

    // 安全性：初始帧位于当前有效的 S-mode 栈上；加载区、用户栈和入口均由链接布局保证有效。
    unsafe { crate::arch::riscv64::trap::start_first_task(&initial_frame) }
}

fn stack_top(stack: *mut TaskStack) -> usize {
    // 安全性：调用方传入静态 TaskStack 的独占裸指针；只计算其末尾地址，不解引用。
    unsafe {
        core::ptr::addr_of_mut!((*stack).0)
            .cast::<u8>()
            .add(STACK_SIZE) as usize
    }
}

/// 处理用户系统调用，或报告独立用户程序的最终陷入现场。
#[no_mangle]
pub extern "C" fn supervisor_trap_handler(frame: *mut TrapFrame) -> *mut TrapFrame {
    // 安全性：汇编入口已切换到静态 S-mode 陷入栈，并在那里构造完整且独占的陷入帧。
    let frame = unsafe { &mut *frame };
    let cause = TrapCause::decode(frame.cause);

    if cause == TrapCause::Exception(ExceptionCause::UserEnvironmentCall)
        && frame.status & (1 << 8) == 0
    {
        let syscall_number = frame.registers[17];
        let arguments = [
            frame.registers[10],
            frame.registers[11],
            frame.registers[12],
        ];
        frame.registers[10] = super::syscall::dispatch(syscall_number, arguments) as usize;
        // RISC-V 的 ecall 固定为 32 位指令；返回时从其后一条用户指令继续执行。
        frame.exception_pc += 4;

        // 安全性：U-mode 陷入帧位于专用内核栈顶下方；sscratch 必须指回该栈顶才能安全处理下一次陷入。
        let kernel_stack_top = frame as *mut TrapFrame as usize + core::mem::size_of::<TrapFrame>();
        unsafe { crate::arch::riscv64::trap::set_user_kernel_stack(kernel_stack_top) };

        return frame as *mut TrapFrame;
    }

    // 安全性：陷入期间 SIE 已关闭，U-mode 程序暂停；探针是内核静态数据且不会被其他任务并发修改。
    let probe = core::ptr::addr_of!(KERNEL_MEMORY_PROBE);
    let (kernel_value, user_read_value, user_written_value, write_result) = unsafe {
        (
            core::ptr::read_volatile(core::ptr::addr_of!((*probe).private_value)),
            core::ptr::read_volatile(core::ptr::addr_of!((*probe).user_read_value)),
            core::ptr::read_volatile(core::ptr::addr_of!((*probe).user_written_value)),
            core::ptr::read_volatile(core::ptr::addr_of!((*probe).write_result)),
        )
    };

    println!(
        "U-mode 陷入来源：{}",
        if frame.status & (1 << 8) == 0 {
            "U-mode"
        } else {
            "S-mode"
        }
    );
    println!("独立用户程序寄存器标记：{:#018x}", frame.registers[11]);
    println!("U-mode 读到的内核数据：{user_read_value:#018x}");
    println!("U-mode 写入后的内核数据：{user_written_value:#018x}");
    println!("write 系统调用返回值：{write_result}");
    println!("陷入原因：{cause:?}");
    println!("原始 scause: {:#018x}", frame.cause);
    println!("sepc:   {:#018x}", frame.exception_pc);
    println!("stval:  {:#018x}", frame.trap_value);

    match cause {
        TrapCause::Exception(ExceptionCause::IllegalInstruction)
            if frame.registers[11] == USER_PROGRAM_MARKER
                && kernel_value == USER_PROGRAM_MARKER
                && user_read_value == INITIAL_KERNEL_VALUE
                && user_written_value == USER_PROGRAM_MARKER
                && write_result > 0 =>
        {
            println!(
                "结论：独立用户程序通过 Linux RISC-V write syscall 输出；satp=BARE 尚未隔离内核内存"
            );
        }
        other => {
            println!("实验结果不符合预期：陷入原因 {other:?}，内核值 {kernel_value:#018x}");
        }
    }

    stop_forever()
}

fn stop_forever() -> ! {
    loop {
        // 安全性：实验陷入后不尝试恢复用户程序；WFI 让 hart 等待，不访问其他内存。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
