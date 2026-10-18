//! 管理进程正常退出、异常终止和退出状态发布。

use crate::arch::riscv64::trap::{ExceptionCause, TrapCause, TrapFrame};

use super::{ProcessExitStatus, ProcessState, PROCESSES};

/// 按 `exit` 系统调用状态结束当前进程。
pub(in crate::kernel) fn exit_current_process(status: u8) -> *mut TrapFrame {
    let current_index = super::current_process_index();
    crate::klog_debug!(
        "进程 PID={} 调用 exit 结束，状态码：{status}",
        super::current_linux_pid()
    );
    mark_process_exited(current_index, ProcessExitStatus::Code(status));
    super::wait::notify_child_exit(current_index);
    super::schedule_next(current_index)
}

/// 将因用户态异常终止的进程记为退出，并切换到其他就绪进程。
pub(in crate::kernel) fn terminate_current_process(
    cause: TrapCause,
    frame: &TrapFrame,
) -> *mut TrapFrame {
    let current_index = super::current_process_index();
    let process_id = super::current_linux_pid();
    crate::klog_error!(
        "用户进程 PID={} 陷入：原因={cause:?}，scause={:#018x}，sepc={:#018x}，stval={:#018x}",
        process_id,
        frame.cause,
        frame.exception_pc,
        frame.trap_value
    );
    mark_process_exited(
        current_index,
        ProcessExitStatus::Signal(signal_for_trap(cause)),
    );
    super::wait::notify_child_exit(current_index);
    crate::klog_warn!("用户进程 PID={} 因异常终止", process_id);
    super::schedule_next(current_index)
}

fn mark_process_exited(index: usize, exit_status: ProcessExitStatus) {
    let process_id = {
        let Some(process) = (unsafe { &mut *core::ptr::addr_of_mut!(PROCESSES) }).get_mut(index)
        else {
            panic!("无法结束不存在的进程 {}", index);
        };
        process.state = ProcessState::Exited;
        process.exit_status = Some(exit_status);
        process.wait_request = None;
        process.id
    };
    super::wait::orphan_children(process_id);
}

fn signal_for_trap(cause: TrapCause) -> u8 {
    match cause {
        TrapCause::Exception(ExceptionCause::IllegalInstruction) => 4,
        TrapCause::Exception(ExceptionCause::Breakpoint) => 5,
        TrapCause::Exception(
            ExceptionCause::LoadAddressMisaligned | ExceptionCause::StoreAmoAddressMisaligned,
        ) => 7,
        TrapCause::Exception(
            ExceptionCause::InstructionAccessFault
            | ExceptionCause::LoadAccessFault
            | ExceptionCause::StoreAmoAccessFault
            | ExceptionCause::InstructionPageFault
            | ExceptionCause::LoadPageFault
            | ExceptionCause::StoreAmoPageFault,
        ) => 11,
        _ => 4,
    }
}
