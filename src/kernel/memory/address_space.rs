//! 管理任务地址空间根页表的创建、切换、遍历校验和销毁。

use alloc::vec::Vec;

use super::page_table::{
    activate, active_root_address, align_up, map_page, PageTable, PAGE_MASK, PAGE_SIZE,
    PAGE_TABLE_ENTRIES, PTE_ACCESSED, PTE_DIRTY, PTE_EXECUTE, PTE_READ, PTE_USER, PTE_VALID,
    PTE_WRITE,
};
use super::{PageTableError, UserImageMapping, UserStackMapping};

static mut ADDRESS_SPACES: Vec<Option<usize>> = Vec::new();

extern "C" {
    static __kernel_start: u8;
}

/// 为每个任务建立独立页表，并激活第一个任务的地址空间。
///
/// `kernel_end` 是内核映像及 BSS 的结束地址；两个切片按任务索引对应，分别描述该任务
/// 的用户栈页帧和用户程序映射。所有程序和用户栈使用共同虚拟区域，但映射到各自物理页。
/// 所有页表都映射相同的 S-mode 内核区域，每张表只映射对应任务自己的 U-mode 区域。
pub(super) fn initialize(
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

        // FDT 描述的 RAM 建立为 S-mode 可读写恒等映射，页帧、堆和内核栈才可在启用 Sv39 后访问。
        for memory in crate::kernel::frame_allocator::physical_memory_regions()
            .map_err(PageTableError::FrameAllocation)?
        {
            let mut page =
                usize::try_from(memory.start).map_err(|_| PageTableError::InvalidRange)?;
            let end = memory
                .end_exclusive()
                .and_then(|end| usize::try_from(end).ok())
                .ok_or(PageTableError::InvalidRange)?;
            page = align_up(page)?;
            let end = end & !PAGE_MASK;
            while page < end {
                map_page(root_table, page, page, PTE_READ | PTE_WRITE)?;
                page = page
                    .checked_add(PAGE_SIZE)
                    .ok_or(PageTableError::InvalidRange)?;
            }
        }

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

        let console_region = crate::device::console::mmio_region();
        let console_start = console_region.start & !PAGE_MASK;
        let console_end = align_up(
            console_region
                .start
                .checked_add(console_region.size)
                .ok_or(PageTableError::InvalidRange)?,
        )?;
        let mut console_page = console_start;
        while console_page < console_end {
            map_page(root_table, console_page, console_page, PTE_READ | PTE_WRITE)?;
            console_page += PAGE_SIZE;
        }
    }

    activate_address_space(0)?;
    Ok(())
}

/// 切换到指定任务的 Sv39 根页表。
pub(super) fn activate_address_space(address_space_index: usize) -> Result<(), PageTableError> {
    let root_address =
        address_space_root(address_space_index).ok_or(PageTableError::UnknownAddressSpace)?;
    activate(root_address)
}

/// 销毁一个非当前地址空间，并归还它拥有的根页表和中间页表页。
///
/// 返回归还的页表页数。叶子映射指向的内核、用户栈和程序镜像页不会释放。
pub(super) fn destroy_address_space(address_space_index: usize) -> Result<usize, PageTableError> {
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

pub(super) fn address_space_count() -> usize {
    // 安全性：调用位于启动期或 S-mode 陷入处理路径；单 hart 定时器不会在读取期间重入修改容器。
    unsafe { (&*core::ptr::addr_of!(ADDRESS_SPACES)).len() }
}

pub(super) fn address_space_root(address_space_index: usize) -> Option<usize> {
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

fn ranges_overlap(
    first_start: usize,
    first_end: usize,
    second_start: usize,
    second_end: usize,
) -> bool {
    first_start < second_end && second_start < first_end
}
