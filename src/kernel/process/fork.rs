//! 复制当前进程的用户态资源并创建子进程。

use crate::arch::riscv64::trap::TrapFrame;

use super::{creation, KernelStack, Process, ProcessResources, ProcessState};

/// fork 创建失败时供 syscall 层转换为 Linux 错误码的类别。
pub(in crate::kernel) enum ForkError {
    Resources,
    ProcessIdExhausted,
}

/// 复制当前进程并返回子进程 PID。
pub(in crate::kernel) fn fork_current_process(parent_frame: &TrapFrame) -> Result<i32, ForkError> {
    match create_child_process(parent_frame) {
        Ok(child_id) => Ok(child_id.linux_pid()),
        Err(error) => {
            crate::klog_warn!(
                "进程 PID={} fork 失败：{}",
                super::current_linux_pid(),
                error.description()
            );
            if error.is_pid_exhausted() {
                Err(ForkError::ProcessIdExhausted)
            } else {
                Err(ForkError::Resources)
            }
        }
    }
}

fn create_child_process(
    parent_frame: &TrapFrame,
) -> Result<super::ProcessId, creation::ProcessCreationError> {
    let parent_index = super::current_process_index();
    let child_id = creation::reserve_process_slot()?;
    let (parent_id, parent_program_range, program_image, user_stack) = {
        // 安全性：只在单 hart 的 S-mode trap 中创建子进程；进程表不会在该调用中被其他路径修改。
        let processes = unsafe { &*core::ptr::addr_of!(super::PROCESSES) };
        let parent = processes
            .get(parent_index)
            .unwrap_or_else(|| panic!("fork 的父进程索引无效：{parent_index}"));
        let resources = parent
            .resources
            .as_ref()
            .unwrap_or_else(|| panic!("fork 的父进程运行资源已释放：{parent_index}"));
        (
            parent.id,
            resources.program_range,
            resources
                .program_image
                .duplicate()
                .map_err(creation::ProcessCreationError::ProgramLoad)?,
            resources
                .user_stack
                .duplicate()
                .map_err(creation::ProcessCreationError::UserStack)?,
        )
    };
    let kernel_stack =
        KernelStack::allocate().map_err(creation::ProcessCreationError::KernelStack)?;
    let user_stack_range = user_stack.range();
    let kernel_stack_range = kernel_stack.range();
    let user_stack_mapping = super::super::memory::UserStackMapping {
        virtual_start: user_stack.start(),
        frames: user_stack.frames(),
    };
    let kernel_stack_mapping = super::super::memory::KernelStackMapping {
        virtual_start: kernel_stack_range.0,
        physical_start: kernel_stack.physical_start(),
        size: kernel_stack_range.1 - kernel_stack_range.0,
    };
    let user_image_mapping = super::super::memory::UserImageMapping {
        virtual_start: program_image.virtual_start,
        frames: program_image.frames(),
    };
    let address_space = super::super::memory::address_space::create_address_space(
        creation::kernel_memory_layout(),
        &user_stack_mapping,
        kernel_stack_mapping,
        user_image_mapping,
    )
    .map_err(creation::ProcessCreationError::AddressSpace)?;

    let mut child_frame = *parent_frame;
    child_frame.exception_pc = child_frame
        .exception_pc
        .checked_add(4)
        .unwrap_or_else(|| panic!("fork 子进程陷入地址溢出"));
    child_frame.registers[10] = 0;
    let initial_frame_pointer = kernel_stack.trap_frame_pointer();
    // 安全性：目标陷入帧位于新分配内核栈的高半区映射内，且该栈尚未发布给调度器。
    unsafe {
        core::ptr::write(initial_frame_pointer, child_frame);
    }

    let program_name = program_image.name;
    // 安全性：进程表容量已预留；新进程的镜像、栈和页表全部构造完成后才追加记录。
    unsafe {
        (&mut *core::ptr::addr_of_mut!(super::PROCESSES)).push(Process {
            id: child_id,
            parent_id: Some(parent_id),
            state: ProcessState::Ready,
            exit_status: None,
            wait_request: None,
            wait_collected: false,
            resources: Some(ProcessResources {
                address_space,
                program_range: parent_program_range,
                program_image,
                user_stack,
                kernel_stack,
            }),
        });
    }

    crate::klog_info!(
        "fork 创建子进程：父进程 PID={}，子进程 PID={}，程序={}，用户栈={:#018x}..{:#018x}，内核栈={:#018x}..{:#018x}",
        parent_id.linux_pid(),
        child_id.linux_pid(),
        program_name,
        user_stack_range.0,
        user_stack_range.1,
        kernel_stack_range.0,
        kernel_stack_range.1
    );
    Ok(child_id)
}
