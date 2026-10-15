//! 管理 Sv39 地址空间及其对内核其他子系统提供的窄接口。
//! 地址空间创建和页表操作由子模块实现；任务仍拥有用户栈和程序镜像等映射目标。

mod address_space;
mod page_table;
mod page_table_log;
mod user_access;

/// 为单个任务建立独立地址空间；失败时不保留部分页表。
pub(super) fn create_address_space(
    address_space_index: usize,
    kernel_start: usize,
    kernel_end: usize,
    user_stack: &UserStackMapping<'_>,
    user_image: UserImageMapping,
) -> Result<(), PageTableError> {
    address_space::create_address_space(
        address_space_index,
        kernel_start,
        kernel_end,
        user_stack,
        user_image,
    )
}

/// 切换到指定任务的 Sv39 根页表。
pub(super) fn activate_address_space(address_space_index: usize) -> Result<(), PageTableError> {
    address_space::activate_address_space(address_space_index)
}

/// 销毁非活动地址空间，并返回释放的页表页数。
pub(super) fn destroy_address_space(address_space_index: usize) -> Result<usize, PageTableError> {
    address_space::destroy_address_space(address_space_index)
}

/// 在调用方已验证的用户地址范围内读取一个字节。
///
/// # Safety
///
/// 调用方必须保证地址位于当前任务已映射且可读的用户区域内。
pub(super) unsafe fn read_user_byte(address: usize) -> u8 {
    // 安全性：该包装函数沿用调用方对地址范围的保证，并将访问交给局部用户内存接口。
    unsafe { user_access::read_user_byte(address) }
}

/// 在调用方已验证的用户地址范围内写入一个值。
///
/// # Safety
///
/// 调用方必须保证地址对 `T` 对齐，并且完整的 `T` 位于当前任务可写的用户区域内。
pub(super) unsafe fn write_user_value<T: Copy>(address: usize, value: T) {
    // 安全性：该包装函数沿用调用方对地址和对齐的保证，并将访问交给局部用户内存接口。
    unsafe { user_access::write_user_value(address, value) }
}

/// 描述某个任务中用户程序虚拟区域到其独立物理镜像槽位的映射。
#[derive(Clone, Copy)]
pub(super) struct UserImageMapping {
    pub(super) virtual_start: usize,
    pub(super) physical_start: usize,
    pub(super) size: usize,
}

/// 描述一个任务的用户栈虚拟起点及其拥有的物理页帧。
pub(super) struct UserStackMapping<'a> {
    pub(super) virtual_start: usize,
    pub(super) frames: &'a [crate::kernel::frame_allocator::PhysicalFrame],
}

/// 以调试日志输出各任务页表中的映射范围。
pub(super) fn log_address_spaces() {
    page_table_log::log_address_spaces();
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
    /// 调度器请求了尚未初始化的地址空间。
    UnknownAddressSpace,
    /// 本实验只建立 4 KiB 叶子映射，却遇到大页叶子项。
    UnexpectedSuperpage,
    /// 根页表没有按 4 KiB 对齐。
    InvalidRootTable,
    /// 地址空间记录的堆内存分配失败。
    AddressSpaceMetadataAllocation,
    /// 地址空间必须按从零开始的连续索引创建。
    AddressSpaceIdOutOfOrder,
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
            Self::UnknownAddressSpace => "任务地址空间尚未初始化",
            Self::UnexpectedSuperpage => "页表中出现本阶段未支持的大页映射",
            Self::InvalidRootTable => "根页表地址为零、未按 4 KiB 对齐或不属于已分配页帧",
            Self::AddressSpaceMetadataAllocation => "无法为地址空间记录分配内核堆内存",
            Self::AddressSpaceIdOutOfOrder => "地址空间 ID 未按顺序追加",
            Self::RollbackFailed => "地址空间创建失败且页表回滚失败",
            Self::Sv39Unavailable => "硬件没有接受 Sv39 satp 模式",
        }
    }
}
