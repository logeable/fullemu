//! 用新程序镜像替换当前进程的用户态映像。

use crate::arch::riscv64::trap::TrapFrame;

use super::{ProcessState, PROCESSES};

/// 替换失败时供 syscall 层转换为 Linux 错误码的类别。
pub(in crate::kernel) enum ExecError {
    Resources,
    InvalidImage,
}

/// 在保留 PID、父子关系和内核栈的同时，替换当前进程的程序、用户栈和地址空间。
pub(in crate::kernel) fn replace_current_process(
    program_index: usize,
    frame: &mut TrapFrame,
) -> Result<(), ExecError> {
    let program = match super::super::user_program::load(program_index) {
        Ok(Some(program)) => program,
        Ok(None) => panic!("exec 用户程序索引不存在：{program_index}"),
        Err(error) if error.is_invalid_image() => return Err(ExecError::InvalidImage),
        Err(_) => return Err(ExecError::Resources),
    };
    let Some(program_end) = program.virtual_start.checked_add(program.image_size) else {
        return Err(ExecError::InvalidImage);
    };
    let user_stack = super::UserStack::allocate().map_err(|_| ExecError::Resources)?;
    let current_index = super::current_process_index();

    let kernel_stack = {
        // 安全性：exec 在当前进程的 S-mode syscall 中运行；单 hart 陷入处理期间不会并发改写进程表。
        let processes = unsafe { &*core::ptr::addr_of!(PROCESSES) };
        let process = processes
            .get(current_index)
            .unwrap_or_else(|| panic!("exec 当前进程索引无效：{current_index}"));
        if process.state != ProcessState::Running {
            panic!("只有运行中的进程可以 exec：{}", process.id.linux_pid());
        }
        let resources = process
            .resources
            .as_ref()
            .unwrap_or_else(|| panic!("exec 当前进程资源已释放：{current_index}"));
        let range = resources.kernel_stack.range();
        super::super::memory::KernelStackMapping {
            virtual_start: range.0,
            physical_start: resources.kernel_stack.physical_start(),
            size: range.1 - range.0,
        }
    };

    let user_stack_mapping = super::super::memory::UserStackMapping {
        virtual_start: user_stack.start(),
        frames: user_stack.frames(),
    };
    let user_image_mapping = super::super::memory::UserImageMapping {
        virtual_start: program.virtual_start,
        frames: program.frames(),
        page_permissions: program.page_permissions(),
    };
    let address_space = super::super::memory::address_space::create_address_space(
        super::creation::kernel_memory_layout(),
        &user_stack_mapping,
        kernel_stack,
        user_image_mapping,
    )
    .map_err(|_| ExecError::Resources)?;

    // 新旧页表都映射同一内核栈；先切换到新页表，当前陷入帧和本函数栈帧仍保持有效。
    super::super::memory::address_space::activate_address_space(&address_space)
        .unwrap_or_else(|error| panic!("exec 激活新地址空间失败：{}", error.description()));

    let program_range = (program.virtual_start, program_end);
    let entry = program.entry;
    let user_stack_top = user_stack.top();
    let (old_address_space, old_program_image, old_user_stack) = {
        let processes = unsafe { &mut *core::ptr::addr_of_mut!(PROCESSES) };
        let process = processes
            .get_mut(current_index)
            .unwrap_or_else(|| panic!("exec 当前进程记录消失：{current_index}"));
        let resources = process
            .resources
            .as_mut()
            .unwrap_or_else(|| panic!("exec 当前进程资源已释放：{current_index}"));

        let old_address_space = core::mem::replace(&mut resources.address_space, address_space);
        let old_program_image = core::mem::replace(&mut resources.program_image, program);
        let old_user_stack = core::mem::replace(&mut resources.user_stack, user_stack);
        resources.program_range = program_range;
        (old_address_space, old_program_image, old_user_stack)
    };

    *frame = TrapFrame::for_user_entry(entry, user_stack_top);

    let released_page_count =
        super::super::memory::address_space::destroy_address_space(old_address_space)
            .unwrap_or_else(|error| panic!("exec 回收旧地址空间失败：{}", error.description()));
    drop(old_program_image);
    drop(old_user_stack);

    crate::klog_debug!(
        "进程 PID={} 已 exec 新程序：入口={:#018x}，镜像范围={:#018x}..{:#018x}，回收 {} 个旧页表页",
        super::current_linux_pid(),
        entry,
        program_range.0,
        program_range.1,
        released_page_count
    );
    Ok(())
}
