//! 定义 Sv39 地址空间共用的数据类型，并组织地址空间、用户访问和页表诊断子模块。
//! 操作直接通过职责对应的子模块调用；进程仍拥有用户栈和程序镜像等映射目标。

pub(super) mod address;
pub(super) mod address_space;
pub(super) mod page;
mod page_table;
pub(super) mod page_table_log;
pub(super) mod user_access;

pub(super) use address_space::AddressSpace;

/// 将一张独占物理页帧的内容复制到另一张独占页帧。
pub(super) fn copy_frame(
    source: &crate::kernel::frame_allocator::PhysicalFrame,
    destination: &crate::kernel::frame_allocator::PhysicalFrame,
) {
    if source.start_address() == destination.start_address() {
        panic!("物理页帧复制的源和目标相同");
    }
    let Some(source_virtual_address) = address::physical_to_kernel_virtual(source.start_address())
    else {
        panic!("物理页帧复制源无法映射到内核高半区");
    };
    let Some(destination_virtual_address) =
        address::physical_to_kernel_virtual(destination.start_address())
    else {
        panic!("物理页帧复制目标无法映射到内核高半区");
    };

    // 安全性：调用方独占两张不同的已分配页帧；当前内核页表将两者映射到内核高半区。
    unsafe {
        core::ptr::copy_nonoverlapping(
            source_virtual_address as *const u8,
            destination_virtual_address as *mut u8,
            page::PAGE_SIZE,
        );
    }
}

/// 描述某个进程中用户程序虚拟区域到其独占物理页帧的映射。
#[derive(Clone, Copy)]
pub(super) struct UserImageMapping<'a> {
    pub(super) virtual_start: usize,
    pub(super) frames: &'a [crate::kernel::frame_allocator::PhysicalFrame],
}

/// 描述需要分别设置页表权限的内核链接段范围。
#[derive(Clone, Copy)]
pub(super) struct KernelMemoryLayout {
    pub(super) boot_stack: KernelStackMapping,
    pub(super) text: KernelSectionRange,
    pub(super) rodata: KernelSectionRange,
    pub(super) data: KernelSectionRange,
    pub(super) bss: KernelSectionRange,
}

/// 描述一个内核段的虚拟范围及其对应的物理装载起点。
#[derive(Clone, Copy)]
pub(super) struct KernelSectionRange {
    /// 段在运行时使用的虚拟地址，必须按页对齐。
    pub(super) virtual_start: usize,
    /// 段字节在 RAM 中的物理装载起点，必须按页对齐。
    pub(super) physical_start: usize,
    /// 段有效内容的字节数；页表映射会向上覆盖至整页。
    pub(super) size: usize,
}

/// 描述一个进程的用户栈虚拟起点及其拥有的物理页帧。
pub(super) struct UserStackMapping<'a> {
    pub(super) virtual_start: usize,
    pub(super) frames: &'a [crate::kernel::frame_allocator::PhysicalFrame],
}

/// 描述进程内核栈的高地址虚拟范围及其连续物理 backing。
#[derive(Clone, Copy)]
pub(super) struct KernelStackMapping {
    pub(super) virtual_start: usize,
    pub(super) physical_start: usize,
    pub(super) size: usize,
}

/// 描述 Sv39 页表初始化失败的原因。
#[derive(Clone, Copy)]
pub(super) enum PageTableError {
    /// 映射范围为空、反向或没有按页对齐。
    InvalidRange,
    /// 物理页帧分配器无法提供页表页。
    FrameAllocation(crate::kernel::frame_allocator::FrameError),
    /// 页表树中的子页表地址不属于已分配页帧。
    InvalidChildTable,
    /// 不能销毁当前 hart 正在使用的地址空间。
    ActiveAddressSpace,
    /// 归还的页表页数量与销毁前验证的数量不一致。
    PageTableCountMismatch,
    /// 页表页归还给物理页帧分配器时失败。
    FrameDeallocation(crate::kernel::frame_allocator::FrameError),
    /// 本实验只建立 4 KiB 叶子映射，却遇到大页叶子项。
    UnexpectedSuperpage,
    /// 根页表没有按 4 KiB 对齐。
    InvalidRootTable,
    /// 建立地址空间失败，且部分页表无法安全回滚。
    RollbackFailed,
    /// 当前硬件没有接受 Sv39 模式。
    Sv39Unavailable,
}

impl PageTableError {
    /// 返回适合内核启动日志的错误说明。
    pub(super) fn description(self) -> &'static str {
        match self {
            Self::InvalidRange => "页表映射范围无效或未按 4 KiB 对齐",
            Self::FrameAllocation(error) => error.description(),
            Self::InvalidChildTable => "页表子表地址不属于已分配物理页帧",
            Self::ActiveAddressSpace => "不能销毁当前活动地址空间",
            Self::PageTableCountMismatch => "地址空间销毁前后页表页数量不一致",
            Self::FrameDeallocation(error) => error.description(),
            Self::UnexpectedSuperpage => "页表中出现本阶段未支持的大页映射",
            Self::InvalidRootTable => "根页表地址为零、未按 4 KiB 对齐或不属于已分配页帧",
            Self::RollbackFailed => "地址空间创建失败且页表回滚失败",
            Self::Sv39Unavailable => "硬件没有接受 Sv39 satp 模式",
        }
    }
}
