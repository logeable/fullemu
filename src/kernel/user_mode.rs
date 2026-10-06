//! 依次运行独立构建的 U-mode 程序，并处理系统调用与特权陷入。

use crate::arch::riscv64::trap::{ExceptionCause, TrapCause, TrapFrame};

const STACK_SIZE: usize = 16 * 1024;

#[repr(align(16))]
struct TaskStack([u8; STACK_SIZE]);

static mut USER_STACK: TaskStack = TaskStack([0; STACK_SIZE]);
static mut KERNEL_TRAP_STACK: TaskStack = TaskStack([0; STACK_SIZE]);
static mut CURRENT_PROGRAM_INDEX: usize = 0;

/// 按清单顺序批量运行所有嵌入内核的用户程序。
pub fn run_user_program_batch() -> ! {
    crate::klog_info!("开始用户程序批量执行；当前 satp 使用 BARE");
    crate::klog_warn!("批次中的用户程序仍未与内核内存隔离");

    // 安全性：当前批处理不使用定时器；关闭 S-mode 中断并明确保持物理地址直映。
    unsafe {
        core::arch::asm!(
            "csrw sie, zero",
            "csrw satp, zero",
            "sfence.vma zero, zero",
            options(nostack)
        );
    }

    let first_program = match super::user_program::load(0) {
        Ok(Some(program)) => program,
        Ok(None) => {
            crate::klog_error!("用户程序批次清单为空");
            stop_forever();
        }
        Err(error) => {
            report_load_error(error);
            stop_forever();
        }
    };
    // 安全性：批次索引仅在单 hart 的 S-mode 初始化和系统调用陷入中访问。
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(CURRENT_PROGRAM_INDEX), 0);
    }
    crate::klog_info!(
        "装入批次程序 1：{}，入口 {:#018x}，大小 {} 字节",
        first_program.name,
        first_program.entry,
        first_program.image_size
    );

    let user_stack_top = reset_user_stack();
    let kernel_stack_top = stack_top(core::ptr::addr_of_mut!(KERNEL_TRAP_STACK));
    let initial_frame = TrapFrame::for_user_entry(first_program.entry, user_stack_top);

    // 安全性：陷入栈是静态分配且按 16 字节对齐；U-mode 陷入入口会切换到该栈保存现场。
    unsafe { crate::arch::riscv64::trap::set_user_kernel_stack(kernel_stack_top) };

    // 安全性：初始帧位于当前有效的 S-mode 栈上，入口和用户栈均由固定链接布局保证有效。
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

fn reset_user_stack() -> usize {
    let stack = core::ptr::addr_of_mut!(USER_STACK);

    // 安全性：首次启动时仍在 S-mode；批次切换时前一程序已陷入且不再运行，用户栈此时可整体清零。
    unsafe {
        core::ptr::write_bytes(
            core::ptr::addr_of_mut!((*stack).0).cast::<u8>(),
            0,
            STACK_SIZE,
        );
    }

    stack_top(stack)
}

/// 处理用户系统调用、批次切换，或报告不可恢复的用户陷入。
#[no_mangle]
pub extern "C" fn supervisor_trap_handler(frame: *mut TrapFrame) -> *mut TrapFrame {
    // 安全性：汇编入口已切换到静态 S-mode 陷入栈，并在那里构造完整且独占的陷入帧。
    let frame = unsafe { &mut *frame };
    let cause = TrapCause::decode(frame.cause);
    crate::klog_trace!("进入 supervisor trap handler：原因={cause:?}");

    if cause == TrapCause::Exception(ExceptionCause::UserEnvironmentCall)
        && frame.status & (1 << 8) == 0
    {
        let syscall_number = frame.registers[17];
        let arguments = [
            frame.registers[10],
            frame.registers[11],
            frame.registers[12],
        ];
        crate::klog_debug!(
            "用户系统调用：编号={}，参数={:x?}",
            syscall_number,
            arguments
        );
        match super::syscall::dispatch(syscall_number, arguments) {
            super::syscall::SyscallOutcome::Return(result) => {
                frame.registers[10] = result as usize;
                // RISC-V 的 ecall 固定为 32 位指令；返回时从其后一条用户指令继续执行。
                frame.exception_pc += 4;
                return restore_user_frame(frame);
            }
            super::syscall::SyscallOutcome::Exit(status) => {
                let current_index = current_program_index();
                crate::klog_info!(
                    "批次程序 {} 调用 exit 结束，状态码：{status}",
                    current_index + 1
                );
                return start_next_program(frame, current_index + 1);
            }
        }
    }

    crate::klog_error!(
        "用户任务陷入：来源={}，原因={cause:?}，scause={:#018x}，sepc={:#018x}，stval={:#018x}",
        if frame.status & (1 << 8) == 0 {
            "U-mode"
        } else {
            "S-mode"
        },
        frame.cause,
        frame.exception_pc,
        frame.trap_value
    );

    stop_forever()
}

fn current_program_index() -> usize {
    // 安全性：只有本 hart 的 S-mode 初始化和陷入处理器读取该字段；satp=BARE 仍允许用户程序直接访问内核 RAM。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(CURRENT_PROGRAM_INDEX)) }
}

fn start_next_program(frame: &mut TrapFrame, index: usize) -> *mut TrapFrame {
    let program = match super::user_program::load(index) {
        Ok(Some(program)) => program,
        Ok(None) => {
            crate::klog_info!("用户程序批次完成，共运行 {index} 个程序");
            stop_forever();
        }
        Err(error) => {
            report_load_error(error);
            stop_forever();
        }
    };

    // 安全性：任务切换在当前 U-mode 任务已陷入后执行，只有本 hart 更新批次索引。
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(CURRENT_PROGRAM_INDEX), index);
    }
    crate::klog_info!(
        "装入批次程序 {}：{}，入口 {:#018x}，大小 {} 字节",
        index + 1,
        program.name,
        program.entry,
        program.image_size
    );

    *frame = TrapFrame::for_user_entry(program.entry, reset_user_stack());
    restore_user_frame(frame)
}

fn restore_user_frame(frame: &mut TrapFrame) -> *mut TrapFrame {
    // 安全性：陷入帧位于专用内核栈顶下方；sscratch 必须恢复为该栈顶以处理下一次 U-mode 陷入。
    let kernel_stack_top = frame as *mut TrapFrame as usize + core::mem::size_of::<TrapFrame>();
    unsafe { crate::arch::riscv64::trap::set_user_kernel_stack(kernel_stack_top) };
    frame as *mut TrapFrame
}

fn report_load_error(error: super::user_program::UserProgramLoadError) {
    crate::klog_error!(
        "用户程序 {} 加载失败：{}（镜像 {} 字节，加载区 {} 字节）",
        error.program_name,
        error.description(),
        error.image_size,
        error.capacity
    );
}

fn stop_forever() -> ! {
    loop {
        // 安全性：当前批次没有更多可运行任务；WFI 让 hart 等待，不再恢复已结束的用户程序。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
