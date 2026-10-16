//! 将单个用户程序及其运行资源组成一项任务。

use crate::arch::riscv64::trap::TrapFrame;

use super::super::memory::{KernelMemoryLayout, KernelSectionRange, KernelStackMapping};
use super::{stack::StackError, KernelStack, Task, TaskId, TaskState, UserStack};

/// 单任务创建失败的阶段和原因。
pub(super) enum TaskCreationError {
    TaskMetadataAllocation,
    ProgramMissing,
    ProgramLoad(super::super::user_program::UserProgramLoadError),
    UserStack(StackError),
    KernelStack(StackError),
    ProgramRangeOverflow,
    AddressSpace(super::super::memory::PageTableError),
}

impl TaskCreationError {
    /// 返回适合启动错误日志的简要说明。
    pub(super) fn description(&self) -> &'static str {
        match self {
            Self::TaskMetadataAllocation => "无法为任务记录预留内核堆空间",
            Self::ProgramMissing => "用户程序索引不存在",
            Self::ProgramLoad(error) => error.description(),
            Self::UserStack(error) => error.description(),
            Self::KernelStack(error) => error.description(),
            Self::ProgramRangeOverflow => "用户程序地址范围溢出",
            Self::AddressSpace(error) => error.description(),
        }
    }
}

/// 创建并追加一个任务；失败时局部所有者自动回收栈，页表由地址空间模块回滚。
pub(super) fn create_task(program_index: usize) -> Result<TaskId, TaskCreationError> {
    create_task_inner(program_index)
}

fn create_task_inner(program_index: usize) -> Result<TaskId, TaskCreationError> {
    let task_id = {
        // 安全性：任务只能在单 hart 的启动或陷入路径创建；当前 S-mode 陷入不会嵌套修改任务表。
        let tasks = unsafe { &mut *core::ptr::addr_of_mut!(super::TASKS) };
        tasks
            .try_reserve(1)
            .map_err(|_| TaskCreationError::TaskMetadataAllocation)?;
        TaskId::from_index(tasks.len())
    };

    let program = match super::super::user_program::load(program_index)
        .map_err(TaskCreationError::ProgramLoad)?
    {
        Some(program) => program,
        None => return Err(TaskCreationError::ProgramMissing),
    };
    let user_stack = UserStack::allocate().map_err(TaskCreationError::UserStack)?;
    let kernel_stack = KernelStack::allocate().map_err(TaskCreationError::KernelStack)?;
    let program_end = program
        .virtual_start
        .checked_add(program.image_size)
        .ok_or(TaskCreationError::ProgramRangeOverflow)?;

    let user_stack_range = user_stack.range();
    let kernel_stack_range = kernel_stack.range();
    let initial_frame_address = kernel_stack.top() - core::mem::size_of::<TrapFrame>();
    let initial_frame = TrapFrame::for_user_entry(program.entry, user_stack.top());
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
        virtual_start: program.virtual_start,
        frames: program.frames(),
    };
    let kernel_layout = kernel_memory_layout();

    super::super::memory::create_address_space(
        task_id.index(),
        kernel_layout,
        &user_stack_mapping,
        kernel_stack_mapping,
        user_image_mapping,
    )
    .map_err(TaskCreationError::AddressSpace)?;

    // 安全性：陷入帧写入本任务独占内核栈的高半区直接映射；该物理页已分配且保持映射。
    unsafe {
        core::ptr::write(initial_frame_address as *mut TrapFrame, initial_frame);
    }

    let program_name = program.name;
    let program_entry = program.entry;
    let image_size = program.image_size;
    // 安全性：任务表容量已在资源分配前预留；追加不再申请内存，且只有全部资源准备成功后才发布任务记录。
    unsafe {
        (&mut *core::ptr::addr_of_mut!(super::TASKS)).push(Task {
            id: task_id,
            state: TaskState::Ready,
            saved_frame_address: initial_frame_address,
            program_range: (program.virtual_start, program_end),
            program_image: Some(program),
            user_stack: Some(user_stack),
            kernel_stack: Some(kernel_stack),
        });
    }

    crate::klog_info!(
        "用户任务已创建：任务={}，程序={}，入口 {:#018x}，镜像 {} 字节；用户栈虚拟={:#018x}..{:#018x}，内核栈高地址={:#018x}..{:#018x}",
        task_id.index(),
        program_name,
        program_entry,
        image_size,
        user_stack_range.0,
        user_stack_range.1,
        kernel_stack_range.0,
        kernel_stack_range.1
    );
    Ok(task_id)
}

/// 从链接器符号组装高半区内核段及其物理装载范围。
fn kernel_memory_layout() -> KernelMemoryLayout {
    let boot_stack_start = core::ptr::addr_of!(super::__boot_stack_virtual_start) as usize;
    let boot_stack_end = core::ptr::addr_of!(super::__boot_stack_virtual_end) as usize;

    KernelMemoryLayout {
        boot_stack: KernelStackMapping {
            virtual_start: boot_stack_start,
            physical_start: super::super::memory::address::kernel_virtual_to_physical(
                boot_stack_start,
            ),
            size: boot_stack_end - boot_stack_start,
        },
        text: kernel_section(
            core::ptr::addr_of!(super::__text_start) as usize,
            core::ptr::addr_of!(super::__text_end) as usize,
        ),
        rodata: kernel_section(
            core::ptr::addr_of!(super::__rodata_start) as usize,
            core::ptr::addr_of!(super::__rodata_end) as usize,
        ),
        data: kernel_section(
            core::ptr::addr_of!(super::__data_start) as usize,
            core::ptr::addr_of!(super::__data_end) as usize,
        ),
        bss: kernel_section(
            core::ptr::addr_of!(super::__bss_start) as usize,
            core::ptr::addr_of!(super::__bss_end) as usize,
        ),
    }
}

fn kernel_section(virtual_start: usize, virtual_end: usize) -> KernelSectionRange {
    KernelSectionRange {
        virtual_start,
        physical_start: super::super::memory::address::kernel_virtual_to_physical(virtual_start),
        size: virtual_end - virtual_start,
    }
}
