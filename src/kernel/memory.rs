//! 为每个用户任务建立独立 Sv39 地址空间和 U/S 权限边界。
//!
//! 页表页和任务栈从固定物理页帧池取得；根页表地址按任务索引记录。内核和 QEMU `virt`
//! UART 使用恒等映射；用户栈及共同的用户程序虚拟区域映射到各自任务的物理页。
//! 当前单 hart 的容器访问发生在启动期或 S-mode 陷入处理路径，不会被可调度的 S-mode 定时器中断重入。

use alloc::vec::Vec;

const PAGE_SIZE: usize = 4096;
const PAGE_MASK: usize = PAGE_SIZE - 1;
const PAGE_TABLE_ENTRIES: usize = 512;
const SATP_SV39_MODE: usize = 8;
const SATP_MODE_SHIFT: usize = 60;
const SATP_PPN_MASK: usize = (1 << 44) - 1;
const UART_BASE: usize = 0x1000_0000;

const PTE_VALID: usize = 1 << 0;
const PTE_READ: usize = 1 << 1;
const PTE_WRITE: usize = 1 << 2;
const PTE_EXECUTE: usize = 1 << 3;
const PTE_USER: usize = 1 << 4;
const PTE_ACCESSED: usize = 1 << 6;
const PTE_DIRTY: usize = 1 << 7;

const SSTATUS_SUM: usize = 1 << 18;
const SV39_VIRTUAL_ADDRESS_MASK: usize = (1 << 39) - 1;

#[repr(C, align(4096))]
struct PageTable([usize; PAGE_TABLE_ENTRIES]);

static mut ADDRESS_SPACES: Vec<Option<usize>> = Vec::new();

/// 描述某个任务中用户程序虚拟区域到其独立物理镜像槽位的映射。
#[derive(Clone, Copy)]
pub(crate) struct UserImageMapping {
    pub virtual_start: usize,
    pub physical_start: usize,
    pub size: usize,
}

/// 描述一个任务的用户栈虚拟起点及其拥有的物理页帧。
pub(crate) struct UserStackMapping<'a> {
    pub virtual_start: usize,
    pub frames: &'a [crate::kernel::frame_allocator::PhysicalFrame],
}

/// 为每个任务建立独立页表，并激活第一个任务的地址空间。
///
/// `kernel_end` 是内核映像及 BSS 的结束地址；两个切片按任务索引对应，分别描述该任务
/// 的用户栈页帧和用户程序映射。所有程序和用户栈使用共同虚拟区域，但映射到各自物理页。
/// 所有页表都映射相同的 S-mode 内核区域，每张表只映射对应任务自己的 U-mode 区域。
pub fn initialize(
    kernel_end: usize,
    user_stacks: &[UserStackMapping<'_>],
    user_images: &[UserImageMapping],
) -> Result<(), PageTableError> {
    let kernel_start = core::ptr::addr_of!(__kernel_start) as usize;
    let kernel_end = align_up(kernel_end)?;

    if kernel_start & PAGE_MASK != 0
        || kernel_end <= kernel_start
        || user_stacks.is_empty()
        || user_stacks.len() != user_images.len()
    {
        return Err(PageTableError::InvalidRange);
    }

    for stack in user_stacks {
        let Some(stack_size) = stack.frames.len().checked_mul(PAGE_SIZE) else {
            return Err(PageTableError::InvalidRange);
        };
        let Some(stack_end) = stack.virtual_start.checked_add(stack_size) else {
            return Err(PageTableError::InvalidRange);
        };
        if stack.virtual_start & PAGE_MASK != 0
            || stack.frames.is_empty()
            || stack_end > (1usize << 38)
        {
            return Err(PageTableError::InvalidRange);
        }
        for (page_index, frame) in stack.frames.iter().enumerate() {
            let physical_address = frame.start_address();
            if physical_address & PAGE_MASK != 0
                || physical_address < kernel_start
                || physical_address
                    .checked_add(PAGE_SIZE)
                    .is_none_or(|end| end > kernel_end)
                || !crate::kernel::frame_allocator::is_allocated_frame_address(physical_address)
            {
                return Err(PageTableError::InvalidRange);
            }
            let page_virtual_start = stack.virtual_start + page_index * PAGE_SIZE;
            if user_images.iter().any(|image| {
                image
                    .virtual_start
                    .checked_add(image.size)
                    .is_none_or(|image_end| {
                        ranges_overlap(
                            page_virtual_start,
                            page_virtual_start + PAGE_SIZE,
                            image.virtual_start,
                            image_end,
                        )
                    })
            }) {
                return Err(PageTableError::InvalidRange);
            }
        }
    }

    for (index, image) in user_images.iter().enumerate() {
        let Some(virtual_end) = image.virtual_start.checked_add(image.size) else {
            return Err(PageTableError::InvalidRange);
        };
        let Some(physical_end) = image.physical_start.checked_add(image.size) else {
            return Err(PageTableError::InvalidRange);
        };
        if image.virtual_start & PAGE_MASK != 0
            || image.physical_start & PAGE_MASK != 0
            || image.size == 0
            || image.size & PAGE_MASK != 0
            || image.virtual_start < kernel_end
            || image.physical_start < kernel_end
            || virtual_end <= image.virtual_start
            || physical_end <= image.physical_start
            || user_images[..index].iter().any(|other| {
                ranges_overlap(
                    image.physical_start,
                    physical_end,
                    other.physical_start,
                    other.physical_start.saturating_add(other.size),
                )
            })
        {
            return Err(PageTableError::InvalidRange);
        }
    }

    // 安全性：初始化只在单 hart 启动路径执行，定时器和全局 S-mode 中断尚未开启。
    let address_spaces = unsafe { &mut *core::ptr::addr_of_mut!(ADDRESS_SPACES) };
    if !address_spaces.is_empty() {
        return Err(PageTableError::AlreadyInitialized);
    }
    address_spaces
        .try_reserve_exact(user_stacks.len())
        .map_err(|_| PageTableError::AddressSpaceMetadataAllocation)?;
    address_spaces.resize(user_stacks.len(), None);

    for (address_space_index, (stack, image)) in user_stacks.iter().zip(user_images).enumerate() {
        let root_frame = crate::kernel::frame_allocator::allocate_zeroed_frame()
            .map_err(PageTableError::FrameAllocation)?;
        let root_address = root_frame.into_address();
        set_address_space_root(address_space_index, Some(root_address))?;
        let root_table = root_address as *mut PageTable;

        let mut kernel_page = kernel_start;
        while kernel_page < kernel_end {
            map_page(
                root_table,
                kernel_page,
                kernel_page,
                PTE_READ | PTE_WRITE | PTE_EXECUTE,
            )?;
            kernel_page += PAGE_SIZE;
        }

        let mut image_offset = 0;
        while image_offset < image.size {
            map_page(
                root_table,
                image.virtual_start + image_offset,
                image.physical_start + image_offset,
                PTE_READ | PTE_WRITE | PTE_EXECUTE | PTE_USER,
            )?;
            image_offset += PAGE_SIZE;
        }

        for (page_index, frame) in stack.frames.iter().enumerate() {
            map_page(
                root_table,
                stack.virtual_start + page_index * PAGE_SIZE,
                frame.start_address(),
                PTE_READ | PTE_WRITE | PTE_USER,
            )?;
        }

        map_page(root_table, UART_BASE, UART_BASE, PTE_READ | PTE_WRITE)?;
    }

    activate_address_space(0)?;
    Ok(())
}

/// 切换到指定任务的 Sv39 根页表。
pub fn activate_address_space(address_space_index: usize) -> Result<(), PageTableError> {
    let root_address =
        address_space_root(address_space_index).ok_or(PageTableError::UnknownAddressSpace)?;
    activate(root_address)
}

/// 销毁一个非当前地址空间，并归还它拥有的根页表和中间页表页。
///
/// 返回归还的页表页数。叶子映射指向的内核、用户栈和程序镜像页不会释放。
pub fn destroy_address_space(address_space_index: usize) -> Result<usize, PageTableError> {
    let root_address =
        address_space_root(address_space_index).ok_or(PageTableError::UnknownAddressSpace)?;
    if root_address == active_root_address() {
        return Err(PageTableError::ActiveAddressSpace);
    }

    // 先完整验证树中所有页表页，再开始释放，避免发现坏链接后留下半棵已拆除的树。
    let page_table_count = validate_page_table_tree(root_address, 2)?;
    let released_page_count = release_page_table_tree(root_address, 2)?;
    if released_page_count != page_table_count {
        return Err(PageTableError::PageTableCountMismatch);
    }

    // 安全性：根页表树已完整释放；清空容器槽位避免后续误激活已归还的根页帧。
    set_address_space_root(address_space_index, None)?;
    Ok(released_page_count)
}

fn validate_page_table_tree(table_address: usize, level: usize) -> Result<usize, PageTableError> {
    if !crate::kernel::frame_allocator::is_allocated_frame_address(table_address) {
        return Err(PageTableError::InvalidChildTable);
    }

    let table = table_address as *const PageTable;
    let mut page_table_count = 1usize;
    for index in 0..PAGE_TABLE_ENTRIES {
        // 安全性：当前表地址已验证为页池中的已分配页帧，索引限制在表的 512 项内。
        let entry = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*table).0[index])) };
        if entry & PTE_VALID == 0 || entry & (PTE_READ | PTE_WRITE | PTE_EXECUTE) != 0 {
            continue;
        }
        if level == 0 || entry & (PTE_USER | PTE_ACCESSED | PTE_DIRTY) != 0 {
            return Err(PageTableError::InvalidChildTable);
        }

        let child_address = ((entry >> 10) << 12) as usize;
        let child_count = validate_page_table_tree(child_address, level - 1)?;
        page_table_count = page_table_count
            .checked_add(child_count)
            .ok_or(PageTableError::PageTableCountMismatch)?;
    }

    Ok(page_table_count)
}

fn release_page_table_tree(table_address: usize, level: usize) -> Result<usize, PageTableError> {
    let table = table_address as *mut PageTable;
    let mut released_page_count = 1usize;
    for index in 0..PAGE_TABLE_ENTRIES {
        // 安全性：调用前已验证整棵页表树，当前表仍由未释放的父表或根索引持有。
        let entry_pointer = unsafe { core::ptr::addr_of_mut!((*table).0[index]) };
        // 安全性：索引在 512 项页表范围内，当前表页尚未归还给分配器。
        let entry = unsafe { core::ptr::read_volatile(entry_pointer) };
        if entry & PTE_VALID == 0 || entry & (PTE_READ | PTE_WRITE | PTE_EXECUTE) != 0 {
            continue;
        }

        let child_address = ((entry >> 10) << 12) as usize;
        // 安全性：整棵树已预先验证，且当前地址空间未活动，子页表没有硬件或其他所有者引用。
        unsafe { core::ptr::write_volatile(entry_pointer, 0) };
        released_page_count += release_page_table_tree(child_address, level - 1)?;
    }

    // 安全性：该页只由已验证的非活动页表树拥有，所有子链接已清除，不再有活动页表引用。
    let frame =
        unsafe { crate::kernel::frame_allocator::PhysicalFrame::from_address(table_address) }
            .map_err(PageTableError::FrameDeallocation)?;
    crate::kernel::frame_allocator::deallocate_frame(frame)
        .map_err(PageTableError::FrameDeallocation)?;
    Ok(released_page_count)
}

fn active_root_address() -> usize {
    let satp_value: usize;
    // 安全性：读取 satp 仅用于拒绝销毁当前 hart 正在使用的页表，不访问外部内存。
    unsafe {
        core::arch::asm!(
            "csrr {value}, satp",
            value = out(reg) satp_value,
            options(nomem, nostack, preserves_flags)
        );
    }
    if satp_value >> SATP_MODE_SHIFT != SATP_SV39_MODE {
        return 0;
    }
    (satp_value & SATP_PPN_MASK) << 12
}

fn address_space_count() -> usize {
    // 安全性：调用位于启动期或 S-mode 陷入处理路径；单 hart 定时器不会在读取期间重入修改容器。
    unsafe { (&*core::ptr::addr_of!(ADDRESS_SPACES)).len() }
}

fn address_space_root(address_space_index: usize) -> Option<usize> {
    // 安全性：调用位于启动期或 S-mode 陷入处理路径；单 hart 定时器不会在读取期间重入修改容器。
    unsafe {
        (&*core::ptr::addr_of!(ADDRESS_SPACES))
            .get(address_space_index)
            .copied()
            .flatten()
    }
}

fn set_address_space_root(
    address_space_index: usize,
    root_address: Option<usize>,
) -> Result<(), PageTableError> {
    // 安全性：调用位于启动期或 S-mode 陷入处理路径；单 hart 定时器不会在写入期间重入修改容器。
    let address_space = unsafe { &mut *core::ptr::addr_of_mut!(ADDRESS_SPACES) }
        .get_mut(address_space_index)
        .ok_or(PageTableError::UnknownAddressSpace)?;
    *address_space = root_address;
    Ok(())
}

/// 以调试日志输出每个任务页表中的有效映射。
///
/// 连续虚拟页映射到连续物理页且权限相同的条目会合并为一个范围。
pub fn log_address_spaces() {
    if !crate::kernel::logging::is_enabled(crate::kernel::logging::Level::Debug) {
        return;
    }

    for address_space_index in 0..address_space_count() {
        let Some(root_address) = address_space_root(address_space_index) else {
            continue;
        };
        if !crate::kernel::frame_allocator::is_allocated_frame_address(root_address) {
            crate::klog_error!(
                "Sv39 页表无效：任务={}，根页表地址 {root_address:#018x} 不是已分配页帧",
                address_space_index
            );
            continue;
        }
        crate::klog_debug!(
            "Sv39 页表：任务={}，根页表物理地址={:#018x}",
            address_space_index,
            root_address
        );

        let mut mapping_run = None;
        walk_page_table(
            root_address as *const PageTable,
            2,
            0,
            address_space_index,
            &mut mapping_run,
        );
        flush_mapping_run(address_space_index, mapping_run);
    }
}

#[derive(Clone, Copy)]
struct MappingRun {
    virtual_start: usize,
    physical_start: usize,
    size: usize,
    permissions: usize,
}

fn walk_page_table(
    table: *const PageTable,
    level: usize,
    virtual_page_number: usize,
    address_space_index: usize,
    mapping_run: &mut Option<MappingRun>,
) {
    for index in 0..PAGE_TABLE_ENTRIES {
        // 安全性：当前表是根页帧或已验证的已分配子页帧，索引由 512 项表长度限制。
        let entry = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*table).0[index])) };
        if entry & PTE_VALID == 0 {
            continue;
        }

        let next_virtual_page_number = (virtual_page_number << 9) | index;
        if entry & (PTE_READ | PTE_WRITE | PTE_EXECUTE) != 0 {
            let page_shift = 12 + level * 9;
            let mapping_size = 1usize << page_shift;
            let virtual_start = canonicalize_sv39(next_virtual_page_number << page_shift);
            let physical_start = ((entry >> 10) << 12) & !(mapping_size - 1);
            record_mapping(
                address_space_index,
                MappingRun {
                    virtual_start,
                    physical_start,
                    size: mapping_size,
                    permissions: entry & (PTE_USER | PTE_READ | PTE_WRITE | PTE_EXECUTE),
                },
                mapping_run,
            );
            continue;
        }

        if level == 0 {
            crate::klog_error!(
                "Sv39 页表无效：任务={}，最低级页表项不是叶子映射，虚拟页号={:#x}",
                address_space_index,
                next_virtual_page_number
            );
            continue;
        }

        let child_table_address = ((entry >> 10) << 12) as usize;
        if child_table_address & PAGE_MASK != 0
            || !crate::kernel::frame_allocator::is_allocated_frame_address(child_table_address)
        {
            crate::klog_error!(
                "Sv39 页表无效：任务={}，子页表地址 {child_table_address:#018x} 不是已分配页帧",
                address_space_index
            );
            continue;
        }

        walk_page_table(
            child_table_address as *const PageTable,
            level - 1,
            next_virtual_page_number,
            address_space_index,
            mapping_run,
        );
    }
}

fn record_mapping(
    address_space_index: usize,
    next_mapping: MappingRun,
    mapping_run: &mut Option<MappingRun>,
) {
    if let Some(current) = mapping_run.as_mut() {
        let virtual_end = current.virtual_start + current.size;
        let physical_end = current.physical_start + current.size;
        if virtual_end == next_mapping.virtual_start
            && physical_end == next_mapping.physical_start
            && current.permissions == next_mapping.permissions
        {
            current.size += next_mapping.size;
            return;
        }
    }

    flush_mapping_run(address_space_index, *mapping_run);
    *mapping_run = Some(next_mapping);
}

fn flush_mapping_run(address_space_index: usize, mapping_run: Option<MappingRun>) {
    let Some(mapping) = mapping_run else {
        return;
    };

    crate::klog_debug!(
        "页表映射：任务={}，虚拟={:#018x}..{:#018x}，物理={:#018x}..{:#018x}，权限={}{}{}{}",
        address_space_index,
        mapping.virtual_start,
        mapping.virtual_start + mapping.size,
        mapping.physical_start,
        mapping.physical_start + mapping.size,
        if mapping.permissions & PTE_USER != 0 {
            "U"
        } else {
            "S"
        },
        if mapping.permissions & PTE_READ != 0 {
            "R"
        } else {
            "-"
        },
        if mapping.permissions & PTE_WRITE != 0 {
            "W"
        } else {
            "-"
        },
        if mapping.permissions & PTE_EXECUTE != 0 {
            "X"
        } else {
            "-"
        }
    );
}

fn canonicalize_sv39(address: usize) -> usize {
    if address & (1 << 38) == 0 {
        address & SV39_VIRTUAL_ADDRESS_MASK
    } else {
        address | !SV39_VIRTUAL_ADDRESS_MASK
    }
}

fn ranges_overlap(
    first_start: usize,
    first_end: usize,
    second_start: usize,
    second_end: usize,
) -> bool {
    first_start < second_end && second_start < first_end
}

/// 在短暂允许 S-mode 访问 U 页面期间读取一个已验证的用户字节。
///
/// # Safety
///
/// 调用方必须先验证 `address` 位于当前任务已映射且可读的用户区域内。
pub(super) unsafe fn read_user_byte(address: usize) -> u8 {
    // 安全性：SUM 仅在下面这一次用户地址读取期间开启，避免扩大内核访问 U 页的窗口。
    set_sum(true);
    // 安全性：调用方保证地址位于当前用户程序或用户栈的有效映射范围内。
    let value = unsafe { core::ptr::read_volatile(address as *const u8) };
    // 安全性：结束单次用户内存访问后立即清除 SUM，恢复内核默认访问限制。
    set_sum(false);
    value
}

/// 在短暂允许 S-mode 访问 U 页面期间写入一个已验证的用户值。
///
/// # Safety
///
/// 调用方必须保证 `address` 对 `T` 对齐，并且完整的 `T` 位于当前任务可写的用户区域内。
pub(super) unsafe fn write_user_value<T: Copy>(address: usize, value: T) {
    // 安全性：SUM 仅在下面这一次用户地址写入期间开启，避免扩大内核访问 U 页的窗口。
    set_sum(true);
    // 安全性：调用方保证地址对齐且整个值位于当前用户栈的可写映射范围内。
    unsafe { core::ptr::write_volatile(address as *mut T, value) };
    // 安全性：结束单次用户内存访问后立即清除 SUM，恢复内核默认访问限制。
    set_sum(false);
}

fn map_page(
    root_table: *mut PageTable,
    virtual_address: usize,
    physical_address: usize,
    permissions: usize,
) -> Result<(), PageTableError> {
    let indices = [
        (virtual_address >> 30) & 0x1ff,
        (virtual_address >> 21) & 0x1ff,
        (virtual_address >> 12) & 0x1ff,
    ];
    let mut table = root_table;

    for index in indices[..2].iter().copied() {
        // 安全性：每一级索引都从 Sv39 虚拟地址的 9 位 VPN 字段计算，范围固定为 0..512。
        let entry = unsafe { core::ptr::addr_of_mut!((*table).0[index]) };
        // 安全性：当前表指针来自已分配根页帧或已验证的子页帧，索引经过 9 位掩码限制。
        let entry_value = unsafe { core::ptr::read(entry) };
        if entry_value & PTE_VALID == 0 {
            let child_frame = crate::kernel::frame_allocator::allocate_zeroed_frame()
                .map_err(PageTableError::FrameAllocation)?;
            let child_address = child_frame.into_address();
            let child = child_address as *mut PageTable;
            let child_physical_page = child_address >> 12;
            // 安全性：当前 PTE 指针属于有效父页表；新子表地址满足 Sv39 PPN 编码布局。
            unsafe { core::ptr::write(entry, (child_physical_page << 10) | PTE_VALID) };
            table = child;
        } else {
            if entry_value & (PTE_READ | PTE_WRITE | PTE_EXECUTE) != 0 {
                return Err(PageTableError::UnexpectedSuperpage);
            }
            let child_address = ((entry_value >> 10) << 12) as usize;
            if !crate::kernel::frame_allocator::is_allocated_frame_address(child_address) {
                return Err(PageTableError::InvalidChildTable);
            }
            table = child_address as *mut PageTable;
        }
    }

    // 安全性：最低层索引也由 9 位 VPN 字段计算，且表指针来自当前页表树。
    let leaf = unsafe { core::ptr::addr_of_mut!((*table).0[indices[2]]) };
    // 安全性：叶子 PTE 位于有效 L0 页表内；物理页由调用方提供并按页对齐。
    unsafe {
        core::ptr::write(
            leaf,
            ((physical_address >> 12) << 10)
                | permissions
                | PTE_VALID
                | PTE_ACCESSED
                | if permissions & PTE_WRITE != 0 {
                    PTE_DIRTY
                } else {
                    0
                },
        );
    }
    Ok(())
}

fn activate(root_physical_address: usize) -> Result<(), PageTableError> {
    if root_physical_address == 0
        || root_physical_address & PAGE_MASK != 0
        || !crate::kernel::frame_allocator::is_allocated_frame_address(root_physical_address)
    {
        return Err(PageTableError::InvalidRootTable);
    }

    let satp_value = (SATP_SV39_MODE << SATP_MODE_SHIFT) | (root_physical_address >> 12);
    // 安全性：根页表来自已分配页帧，恒等映射已覆盖当前内核代码、栈、数据和帧池。
    unsafe {
        core::arch::asm!(
            "csrw satp, {value}",
            "sfence.vma zero, zero",
            value = in(reg) satp_value,
            options(nostack)
        );
    }

    let active_satp: usize;
    // 安全性：读取 satp 仅用于确认硬件接受 Sv39 模式，不访问外部内存。
    unsafe {
        core::arch::asm!(
            "csrr {value}, satp",
            value = out(reg) active_satp,
            options(nomem, nostack, preserves_flags)
        );
    }
    if active_satp >> SATP_MODE_SHIFT != SATP_SV39_MODE {
        return Err(PageTableError::Sv39Unavailable);
    }

    Ok(())
}

fn set_sum(enabled: bool) {
    if enabled {
        // 安全性：调用方仅在单次已验证的 U 页面读写期间设置 SUM。
        unsafe {
            core::arch::asm!(
                "csrs sstatus, {mask}",
                mask = in(reg) SSTATUS_SUM,
                options(nostack)
            );
        }
    } else {
        // 安全性：调用方在访问 U 页面后立即清除 SUM；当前实现不会嵌套该访问。
        unsafe {
            core::arch::asm!(
                "csrc sstatus, {mask}",
                mask = in(reg) SSTATUS_SUM,
                options(nostack)
            );
        }
    }
}

fn align_up(address: usize) -> Result<usize, PageTableError> {
    address
        .checked_add(PAGE_MASK)
        .map(|value| value & !PAGE_MASK)
        .ok_or(PageTableError::InvalidRange)
}

/// 描述 Sv39 页表初始化失败的原因。
#[derive(Clone, Copy)]
pub enum PageTableError {
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
    /// 地址空间集合已经初始化。
    AlreadyInitialized,
    /// 当前硬件没有接受 Sv39 模式。
    Sv39Unavailable,
}

impl PageTableError {
    /// 返回适合内核启动日志的错误说明。
    pub fn description(self) -> &'static str {
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
            Self::AlreadyInitialized => "地址空间集合已经初始化",
            Self::Sv39Unavailable => "硬件没有接受 Sv39 satp 模式",
        }
    }
}

extern "C" {
    static __kernel_start: u8;
}
