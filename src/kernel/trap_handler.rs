//! 在架构陷入现场、系统调用和进程管理之间协调运行时陷入。

use crate::arch::riscv64::trap::{ExceptionCause, InterruptCause, TrapCause, TrapFrame};

const SSTATUS_SPP: usize = 1 << 8;

/// 处理汇编入口保存的陷入现场，并返回需要恢复的陷入帧。
#[no_mangle]
pub extern "C" fn supervisor_trap_handler(frame: *mut TrapFrame) -> *mut TrapFrame {
    // 安全性：汇编入口已切换到当前进程的专用内核栈，并在那里构造完整陷入帧。
    let frame = unsafe { &mut *frame };
    super::process::prepare_trap();
    let cause = TrapCause::decode(frame.cause);
    crate::klog_trace!(
        "进程 PID={} 进入 trap handler：原因={cause:?}",
        super::process::current_linux_pid()
    );

    match (cause, frame.status & SSTATUS_SPP == 0) {
        (TrapCause::Interrupt(InterruptCause::SupervisorTimer), _) => {
            super::process::handle_timer_interrupt(frame)
        }
        (TrapCause::Exception(ExceptionCause::UserEnvironmentCall), true) => {
            super::syscall::handle_user_environment_call(frame)
        }
        (_, true) => super::process::terminate_current_process(cause, frame),
        (_, false) => panic_kernel_trap(cause, frame),
    }
}

fn panic_kernel_trap(cause: TrapCause, frame: &TrapFrame) -> ! {
    panic!(
        "S-mode 陷入：进程 PID={}，原因={cause:?}，scause={:#018x}，sepc={:#018x}，stval={:#018x}",
        super::process::current_linux_pid(),
        frame.cause,
        frame.exception_pc,
        frame.trap_value
    )
}
