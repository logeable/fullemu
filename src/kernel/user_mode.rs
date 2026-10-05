//! 展示 U-mode 的特权指令限制，以及 satp=BARE 时缺少内存隔离的情况。

use crate::arch::riscv64::trap::{ExceptionCause, TrapCause, TrapFrame};

const STACK_SIZE: usize = 16 * 1024;
const INITIAL_KERNEL_VALUE: usize = 0x4B45_524E;
const USER_WRITTEN_KERNEL_VALUE: usize = 0x554D_4F44;

#[repr(align(16))]
struct TaskStack([u8; STACK_SIZE]);

static mut KERNEL_PRIVATE_VALUE: usize = INITIAL_KERNEL_VALUE;
static mut USER_READ_KERNEL_VALUE: usize = 0;
static mut USER_STACK: TaskStack = TaskStack([0; STACK_SIZE]);
static mut KERNEL_TRAP_STACK: TaskStack = TaskStack([0; STACK_SIZE]);

/// 进入 U-mode，观察特权指令限制和当前缺少的内存隔离。
pub fn run_privilege_boundary_demonstration() -> ! {
    println!("\nU-mode 实验：特权限制与内存访问边界");
    println!("当前 satp 使用 BARE，不启用页表地址转换");
    println!("用户任务先读写内核数据，再尝试读取 S-mode 的 sstatus CSR");

    // 安全性：仍处于 S-mode；本实验不使用定时器，清除 S-mode 外部中断源使能，避免异步陷入。
    unsafe {
        core::arch::asm!(
            "csrw sie, zero",
            "csrw satp, zero",
            "sfence.vma zero, zero",
            options(nostack)
        );
    }

    let user_stack_top = stack_top(core::ptr::addr_of_mut!(USER_STACK));
    let kernel_stack_top = stack_top(core::ptr::addr_of_mut!(KERNEL_TRAP_STACK));
    let initial_frame = TrapFrame::for_user_entry(user_task_entry, user_stack_top);

    // 安全性：内核陷入栈是静态分配且按 16 字节对齐；U-mode 陷入入口会切换到该栈保存现场。
    unsafe { crate::arch::riscv64::trap::set_user_kernel_stack(kernel_stack_top) };

    // 安全性：初始帧位于当前有效的 S-mode 栈上；帧内用户栈和入口均为静态内核映像中的有效地址。
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

extern "C" fn user_task_entry() -> ! {
    // 安全性：本实验特意在 satp=BARE 且 QEMU/OpenSBI 允许 S/U 访问该 RAM 的条件下，
    // 从 U-mode 直接读取和写入内核映像中的探针数据，以展示此时没有页级内存隔离。
    unsafe {
        let kernel_value = core::ptr::read_volatile(core::ptr::addr_of!(KERNEL_PRIVATE_VALUE));
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(USER_READ_KERNEL_VALUE),
            kernel_value,
        );
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(KERNEL_PRIVATE_VALUE),
            USER_WRITTEN_KERNEL_VALUE,
        );

        // 安全性：此处故意从 U-mode 读取 S-mode CSR；RISC-V 应产生非法指令陷入，后续不会执行。
        core::arch::asm!("csrr zero, sstatus", options(nomem, nostack));
    }

    loop {
        core::hint::spin_loop();
    }
}

/// 报告 U-mode 陷入时的原因及陷入前已完成的内核数据读写。
#[no_mangle]
pub extern "C" fn supervisor_trap_handler(frame: *mut TrapFrame) -> ! {
    // 安全性：汇编入口已切换到静态 S-mode 陷入栈，并在那里构造完整陷入帧。
    let (raw_cause, exception_pc, trap_value, status) = unsafe {
        (
            (*frame).cause,
            (*frame).exception_pc,
            (*frame).trap_value,
            (*frame).status,
        )
    };
    let cause = TrapCause::decode(raw_cause);

    // 安全性：陷入期间 SIE 已关闭，U-mode 任务暂停；以下易失性读取与 U-mode 写入顺序明确。
    let (user_read_value, kernel_value) = unsafe {
        (
            core::ptr::read_volatile(core::ptr::addr_of!(USER_READ_KERNEL_VALUE)),
            core::ptr::read_volatile(core::ptr::addr_of!(KERNEL_PRIVATE_VALUE)),
        )
    };

    println!(
        "U-mode 陷入来源：{}",
        if status & (1 << 8) == 0 {
            "U-mode"
        } else {
            "S-mode"
        }
    );
    println!("U-mode 读到的内核数据：{user_read_value:#018x}");
    println!("U-mode 写入后的内核数据：{kernel_value:#018x}");
    println!("陷入原因：{cause:?}");
    println!("原始 scause: {raw_cause:#018x}");
    println!("sepc:   {exception_pc:#018x}");
    println!("stval:  {trap_value:#018x}");

    match cause {
        TrapCause::Exception(ExceptionCause::IllegalInstruction) => {
            println!("结论：U-mode 不能读取 S-mode 的 sstatus CSR");
        }
        other => {
            println!("实验遇到非预期陷入：{other:?}");
        }
    }

    stop_forever()
}

fn stop_forever() -> ! {
    loop {
        // 安全性：实验陷入后不尝试恢复用户任务；WFI 让 hart 等待，不访问其他内存。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
