//! 将单个用户程序及其运行资源组成一项任务。

use crate::arch::riscv64::trap::TrapFrame;

use super::{stack::StackError, KernelStack, Task, TaskId, TaskState, UserStack, MAX_USER_TASKS};

/// 单任务创建失败的阶段和原因。
pub(super) enum TaskCreationError {
    TaskLimitReached,
    TaskMetadataAllocation,
    ProgramMissing(usize),
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
            Self::TaskLimitReached => "已达到固定任务记录上限",
            Self::TaskMetadataAllocation => "无法为任务记录预留内核堆空间",
            Self::ProgramMissing(_) => "用户程序索引不存在",
            Self::ProgramLoad(error) => error.description(),
            Self::UserStack(error) => error.description(),
            Self::KernelStack(error) => error.description(),
            Self::ProgramRangeOverflow => "用户程序地址范围溢出",
            Self::AddressSpace(error) => error.description(),
        }
    }

    /// 输出包含任务索引和子系统原因的诊断。
    pub(super) fn report(&self, program_index: usize) {
        match self {
            Self::ProgramMissing(index) => {
                crate::klog_error!("用户程序索引不存在：{index}");
            }
            Self::ProgramLoad(error) => {
                crate::klog_error!(
                    "用户程序 {} 加载失败：{}（镜像 {} 字节，加载槽位 {} 字节）",
                    error.program_name,
                    error.description(),
                    error.image_size,
                    error.capacity
                );
            }
            _ => crate::klog_error!("任务 {} 创建失败：{}", program_index, self.description()),
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
        if tasks.len() >= MAX_USER_TASKS {
            return Err(TaskCreationError::TaskLimitReached);
        }
        tasks
            .try_reserve(1)
            .map_err(|_| TaskCreationError::TaskMetadataAllocation)?;
        TaskId::from_index(tasks.len())
    };

    let program = match super::super::user_program::load(program_index)
        .map_err(TaskCreationError::ProgramLoad)?
    {
        Some(program) => program,
        None => return Err(TaskCreationError::ProgramMissing(program_index)),
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
    let user_image_mapping = super::super::memory::UserImageMapping {
        virtual_start: program.virtual_start,
        physical_start: program.slot_start,
        size: program.slot_end - program.slot_start,
    };
    // 当前 linker.ld 仍规定 VMA=LMA；物理起点改由 LMA 符号提供，不从 VMA 推测。
    let kernel_layout = super::super::memory::KernelMemoryLayout {
        text: super::super::memory::KernelSectionRange {
            virtual_start: core::ptr::addr_of!(super::__text_start) as usize,
            physical_start: core::ptr::addr_of!(super::__text_load_start) as usize,
            size: (core::ptr::addr_of!(super::__text_end) as usize)
                - (core::ptr::addr_of!(super::__text_start) as usize),
        },
        rodata: super::super::memory::KernelSectionRange {
            virtual_start: core::ptr::addr_of!(super::__rodata_start) as usize,
            physical_start: core::ptr::addr_of!(super::__rodata_load_start) as usize,
            size: (core::ptr::addr_of!(super::__rodata_end) as usize)
                - (core::ptr::addr_of!(super::__rodata_start) as usize),
        },
        data: super::super::memory::KernelSectionRange {
            virtual_start: core::ptr::addr_of!(super::__data_start) as usize,
            physical_start: core::ptr::addr_of!(super::__data_load_start) as usize,
            size: (core::ptr::addr_of!(super::__data_end) as usize)
                - (core::ptr::addr_of!(super::__data_start) as usize),
        },
        bss: super::super::memory::KernelSectionRange {
            virtual_start: core::ptr::addr_of!(super::__bss_start) as usize,
            physical_start: core::ptr::addr_of!(super::__bss_load_start) as usize,
            size: (core::ptr::addr_of!(super::__bss_end) as usize)
                - (core::ptr::addr_of!(super::__bss_start) as usize),
        },
    };

    super::super::memory::create_address_space(
        task_id.index(),
        kernel_layout,
        &user_stack_mapping,
        user_image_mapping,
    )
    .map_err(TaskCreationError::AddressSpace)?;

    // 安全性：陷入帧位于本任务独占的连续内核栈页顶部，栈页所有权仍由下方局部变量持有。
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
        "用户任务已创建：任务={}，程序={}，入口 {:#018x}，镜像 {} 字节；用户栈虚拟={:#018x}..{:#018x}，内核栈恒等地址={:#018x}..{:#018x}",
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
