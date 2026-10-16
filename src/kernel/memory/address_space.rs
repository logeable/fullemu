//! 管理任务地址空间根页表的创建、切换、遍历校验和销毁。

use alloc::vec::Vec;

use super::page_table::{
    activate, active_root_address, align_up, map_page, PageTable, PAGE_MASK, PAGE_SIZE,
    PAGE_TABLE_ENTRIES, PTE_ACCESSED, PTE_DIRTY, PTE_EXECUTE, PTE_READ, PTE_USER, PTE_VALID,
    PTE_WRITE,
};
use super::{
    KernelMemoryLayout, KernelSectionRange, KernelStackMapping, PageTableError, UserImageMapping,
    UserStackMapping,
};

static mut ADDRESS_SPACES: Vec<Option<usize>> = Vec::new();

/// 为一个任务建立独立页表；失败时归还本次创建的页表树。
///
/// `address_space_index` 必须等于当前已成功创建的地址空间数量。这样成功创建的任务
/// 地址空间按稳定索引追加，失败的创建不会留下空槽或消耗索引。
pub(super) fn create_address_space(
    address_space_index: usize,
    kernel_layout: KernelMemoryLayout,
    user_stack: &UserStackMapping<'_>,
    kernel_stack: KernelStackMapping,
    user_image: UserImageMapping,
) -> Result<(), PageTableError> {
    validate_kernel_layout(kernel_layout)?;
    let kernel_end = align_up(
        kernel_layout
            .bss
            .physical_start
            .checked_add(kernel_layout.bss.size)
            .ok_or(PageTableError::InvalidRange)?,
    )?;

    let Some(stack_size) = user_stack.frames.len().checked_mul(PAGE_SIZE) else {
        return Err(PageTableError::InvalidRange);
    };
    let Some(stack_end) = user_stack.virtual_start.checked_add(stack_size) else {
        return Err(PageTableError::InvalidRange);
    };
    if user_stack.virtual_start & PAGE_MASK != 0
        || user_stack.frames.is_empty()
        || stack_end > (1usize << 38)
    {
        return Err(PageTableError::InvalidRange);
    }

    for (page_index, frame) in user_stack.frames.iter().enumerate() {
        let physical_address = frame.start_address();
        if physical_address & PAGE_MASK != 0
            || !crate::kernel::frame_allocator::is_allocated_frame_address(physical_address)
        {
            return Err(PageTableError::InvalidRange);
        }
        let page_virtual_start = user_stack.virtual_start + page_index * PAGE_SIZE;
        let Some(image_end) = user_image.virtual_start.checked_add(user_image.size) else {
            return Err(PageTableError::InvalidRange);
        };
        if ranges_overlap(
            page_virtual_start,
            page_virtual_start + PAGE_SIZE,
            user_image.virtual_start,
            image_end,
        ) {
            return Err(PageTableError::InvalidRange);
        }
    }

    let Some(virtual_end) = user_image.virtual_start.checked_add(user_image.size) else {
        return Err(PageTableError::InvalidRange);
    };
    let Some(physical_end) = user_image.physical_start.checked_add(user_image.size) else {
        return Err(PageTableError::InvalidRange);
    };
    if user_image.virtual_start & PAGE_MASK != 0
        || user_image.physical_start & PAGE_MASK != 0
        || user_image.size == 0
        || user_image.size & PAGE_MASK != 0
        || user_image.physical_start < kernel_end
        || virtual_end > (1usize << 38)
        || virtual_end <= user_image.virtual_start
        || physical_end <= user_image.physical_start
    {
        return Err(PageTableError::InvalidRange);
    }

    for memory in crate::kernel::frame_allocator::physical_memory_regions()
        .map_err(PageTableError::FrameAllocation)?
    {
        let memory_start =
            usize::try_from(memory.start).map_err(|_| PageTableError::InvalidRange)?;
        let memory_end = memory
            .end_exclusive()
            .and_then(|end| usize::try_from(end).ok())
            .ok_or(PageTableError::InvalidRange)?;
        if ranges_overlap(
            user_image.virtual_start,
            virtual_end,
            memory_start,
            memory_end,
        ) {
            return Err(PageTableError::InvalidRange);
        }
    }

    {
        // 安全性：地址空间创建只在单 hart 的启动或陷入路径执行；S-mode 定时器陷入期间不会并发修改该容器。
        let address_spaces = unsafe { &mut *core::ptr::addr_of_mut!(ADDRESS_SPACES) };
        if address_space_index != address_spaces.len() {
            return Err(PageTableError::AddressSpaceIdOutOfOrder);
        }
        address_spaces
            .try_reserve(1)
            .map_err(|_| PageTableError::AddressSpaceMetadataAllocation)?;
    }

    let root_frame = crate::kernel::frame_allocator::allocate_zeroed_frame()
        .map_err(PageTableError::FrameAllocation)?;
    let root_address = root_frame.into_address();
    let root_table = root_address as *mut PageTable;
    if let Err(error) = populate_address_space(
        root_table,
        kernel_layout,
        user_stack,
        kernel_stack,
        user_image,
    ) {
        if rollback_page_table_tree(root_address).is_err() {
            return Err(PageTableError::RollbackFailed);
        }
        return Err(error);
    }

    // 安全性：容器容量已预留；只有页表树完整建立后才发布根地址，单 hart 不会并发观察中间状态。
    unsafe { (&mut *core::ptr::addr_of_mut!(ADDRESS_SPACES)).push(Some(root_address)) };
    Ok(())
}

fn populate_address_space(
    root_table: *mut PageTable,
    kernel_layout: KernelMemoryLayout,
    user_stack: &UserStackMapping<'_>,
    kernel_stack: KernelStackMapping,
    user_image: UserImageMapping,
) -> Result<(), PageTableError> {
    // FDT 描述的 RAM 建立为 S-mode 可读写恒等映射，页帧、堆、FDT 和物理设备页可继续按物理地址访问。
    for memory in crate::kernel::frame_allocator::physical_memory_regions()
        .map_err(PageTableError::FrameAllocation)?
    {
        let mut page = usize::try_from(memory.start).map_err(|_| PageTableError::InvalidRange)?;
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

    map_kernel_section(root_table, kernel_layout.text, PTE_READ | PTE_EXECUTE)?;
    map_kernel_section(root_table, kernel_layout.rodata, PTE_READ)?;
    map_kernel_section(root_table, kernel_layout.data, PTE_READ | PTE_WRITE)?;
    map_kernel_section(root_table, kernel_layout.bss, PTE_READ | PTE_WRITE)?;

    // 首个任务地址空间激活前，启动 Rust 调用链仍在引导栈上；映射引导栈和当前任务栈。
    map_kernel_stack(root_table, kernel_layout.boot_stack)?;
    map_kernel_stack(root_table, kernel_stack)?;

    let mut image_offset = 0;
    while image_offset < user_image.size {
        map_page(
            root_table,
            user_image.virtual_start + image_offset,
            user_image.physical_start + image_offset,
            PTE_READ | PTE_WRITE | PTE_EXECUTE | PTE_USER,
        )?;
        image_offset += PAGE_SIZE;
    }

    for (page_index, frame) in user_stack.frames.iter().enumerate() {
        map_page(
            root_table,
            user_stack.virtual_start + page_index * PAGE_SIZE,
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

    Ok(())
}

fn validate_kernel_layout(layout: KernelMemoryLayout) -> Result<(), PageTableError> {
    let sections = [layout.text, layout.rodata, layout.data, layout.bss];
    for section in sections {
        let mapped_size = align_up(section.size)?;
        if section.virtual_start & PAGE_MASK != 0
            || section.physical_start & PAGE_MASK != 0
            || section.size == 0
            || section.virtual_start.checked_add(mapped_size).is_none()
            || section.physical_start.checked_add(mapped_size).is_none()
        {
            return Err(PageTableError::InvalidRange);
        }
    }

    if align_up(layout.text.virtual_start + layout.text.size)? != layout.rodata.virtual_start
        || align_up(layout.rodata.virtual_start + layout.rodata.size)? != layout.data.virtual_start
        || align_up(layout.data.virtual_start + layout.data.size)? != layout.bss.virtual_start
    {
        return Err(PageTableError::InvalidRange);
    }

    Ok(())
}

fn map_kernel_section(
    root_table: *mut PageTable,
    section: KernelSectionRange,
    permissions: usize,
) -> Result<(), PageTableError> {
    let mapping_size = align_up(section.size)?;
    let mut offset = 0;
    while offset < mapping_size {
        let virtual_address = section
            .virtual_start
            .checked_add(offset)
            .ok_or(PageTableError::InvalidRange)?;
        let physical_address = section
            .physical_start
            .checked_add(offset)
            .ok_or(PageTableError::InvalidRange)?;
        map_page(root_table, virtual_address, physical_address, permissions)?;
        offset = offset
            .checked_add(PAGE_SIZE)
            .ok_or(PageTableError::InvalidRange)?;
    }
    Ok(())
}

/// 让每张任务页表都能访问调度切换期间可能仍在使用的任一内核栈。
pub(super) fn map_kernel_stacks(stacks: &[KernelStackMapping]) -> Result<(), PageTableError> {
    for address_space_index in 0..address_space_count() {
        let root_address =
            address_space_root(address_space_index).ok_or(PageTableError::UnknownAddressSpace)?;
        let root_table = root_address as *mut PageTable;
        for stack in stacks {
            map_kernel_stack(root_table, *stack)?;
        }
    }
    Ok(())
}

fn map_kernel_stack(
    root_table: *mut PageTable,
    stack: KernelStackMapping,
) -> Result<(), PageTableError> {
    if stack.virtual_start & PAGE_MASK != 0
        || stack.physical_start & PAGE_MASK != 0
        || stack.size == 0
        || stack.size & PAGE_MASK != 0
        || stack.virtual_start.checked_add(stack.size).is_none()
        || stack.physical_start.checked_add(stack.size).is_none()
    {
        return Err(PageTableError::InvalidRange);
    }

    let mut offset = 0;
    while offset < stack.size {
        map_page(
            root_table,
            stack.virtual_start + offset,
            stack.physical_start + offset,
            PTE_READ | PTE_WRITE,
        )?;
        offset += PAGE_SIZE;
    }
    Ok(())
}

fn rollback_page_table_tree(root_address: usize) -> Result<(), PageTableError> {
    let validated_page_count = validate_page_table_tree(root_address, 2)?;
    let released_page_count = release_page_table_tree(root_address, 2)?;
    if released_page_count != validated_page_count {
        return Err(PageTableError::PageTableCountMismatch);
    }
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
