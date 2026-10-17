//! 管理进程地址空间根页表的创建、切换、遍历校验和销毁。

use super::page::{align_down_to_page, align_up_to_page, is_page_aligned, PAGE_SIZE};
use super::page_table::{
    activate, active_root_address, map_page, PageTable, PAGE_TABLE_ENTRIES, PTE_ACCESSED,
    PTE_DIRTY, PTE_EXECUTE, PTE_READ, PTE_USER, PTE_VALID, PTE_WRITE,
};
use super::{
    KernelMemoryLayout, KernelSectionRange, KernelStackMapping, PageTableError, UserImageMapping,
    UserStackMapping,
};

/// 为一个进程建立独立页表；失败时归还本次创建的页表树。
pub(super) fn create_address_space(
    kernel_layout: KernelMemoryLayout,
    user_stack: &UserStackMapping<'_>,
    kernel_stack: KernelStackMapping,
    user_image: UserImageMapping<'_>,
) -> Result<AddressSpace, PageTableError> {
    validate_kernel_layout(kernel_layout)?;
    validate_user_mappings(kernel_layout, user_stack, user_image)?;

    let root_frame = crate::kernel::frame_allocator::allocate_zeroed_frame()
        .map_err(PageTableError::FrameAllocation)?;
    let root_address = root_frame.start_address();
    let Some(root_virtual_address) = super::address::physical_to_kernel_virtual(root_address)
    else {
        crate::kernel::frame_allocator::deallocate_frame(root_frame)
            .map_err(PageTableError::FrameDeallocation)?;
        return Err(PageTableError::InvalidRange);
    };
    let root_address = root_frame.into_address();
    let root_table = root_virtual_address as *mut PageTable;
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

    // 安全性：完整页表树的根页帧所有权由返回的地址空间对象独占。
    Ok(AddressSpace { root_address })
}

fn validate_user_mappings(
    kernel_layout: KernelMemoryLayout,
    user_stack: &UserStackMapping<'_>,
    user_image: UserImageMapping<'_>,
) -> Result<(), PageTableError> {
    let kernel_end = align_up_to_page(
        kernel_layout
            .bss
            .physical_start
            .checked_add(kernel_layout.bss.size)
            .ok_or(PageTableError::InvalidRange)?,
    )
    .ok_or(PageTableError::InvalidRange)?;

    let Some(stack_size) = user_stack.frames.len().checked_mul(PAGE_SIZE) else {
        return Err(PageTableError::InvalidRange);
    };
    let Some(stack_end) = user_stack.virtual_start.checked_add(stack_size) else {
        return Err(PageTableError::InvalidRange);
    };
    if !is_page_aligned(user_stack.virtual_start)
        || user_stack.frames.is_empty()
        || stack_end > super::address::SV39_USER_ADDRESS_END_EXCLUSIVE
    {
        return Err(PageTableError::InvalidRange);
    }

    let image_size = user_image
        .frames
        .len()
        .checked_mul(PAGE_SIZE)
        .ok_or(PageTableError::InvalidRange)?;
    let image_end = user_image
        .virtual_start
        .checked_add(image_size)
        .ok_or(PageTableError::InvalidRange)?;

    for (page_index, frame) in user_stack.frames.iter().enumerate() {
        let physical_address = frame.start_address();
        if !is_page_aligned(physical_address)
            || !crate::kernel::frame_allocator::is_allocated_frame_address(physical_address)
        {
            return Err(PageTableError::InvalidRange);
        }
        let page_virtual_start = user_stack.virtual_start + page_index * PAGE_SIZE;
        if ranges_overlap(
            page_virtual_start,
            page_virtual_start + PAGE_SIZE,
            user_image.virtual_start,
            image_end,
        ) {
            return Err(PageTableError::InvalidRange);
        }
    }

    if !is_page_aligned(user_image.virtual_start)
        || user_image.frames.is_empty()
        || image_end > super::address::SV39_USER_ADDRESS_END_EXCLUSIVE
        || image_end <= user_image.virtual_start
    {
        return Err(PageTableError::InvalidRange);
    }

    for frame in user_image.frames {
        let physical_address = frame.start_address();
        if !is_page_aligned(physical_address)
            || physical_address < kernel_end
            || !crate::kernel::frame_allocator::is_allocated_frame_address(physical_address)
        {
            return Err(PageTableError::InvalidRange);
        }
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
            image_end,
            memory_start,
            memory_end,
        ) {
            return Err(PageTableError::InvalidRange);
        }
    }

    Ok(())
}

fn populate_address_space(
    root_table: *mut PageTable,
    kernel_layout: KernelMemoryLayout,
    user_stack: &UserStackMapping<'_>,
    kernel_stack: KernelStackMapping,
    user_image: UserImageMapping<'_>,
) -> Result<(), PageTableError> {
    // FDT 描述的 RAM 映射到高半区直接映射窗口；Rust 指针使用虚拟地址，页表项仍记录物理地址。
    for memory in crate::kernel::frame_allocator::physical_memory_regions()
        .map_err(PageTableError::FrameAllocation)?
    {
        let mut page = usize::try_from(memory.start).map_err(|_| PageTableError::InvalidRange)?;
        let end = memory
            .end_exclusive()
            .and_then(|end| usize::try_from(end).ok())
            .ok_or(PageTableError::InvalidRange)?;
        page = align_up_to_page(page).ok_or(PageTableError::InvalidRange)?;
        let end = align_down_to_page(end);
        while page < end {
            let virtual_page = super::address::physical_to_kernel_virtual(page)
                .ok_or(PageTableError::InvalidRange)?;
            map_page(root_table, virtual_page, page, PTE_READ | PTE_WRITE)?;
            page = page
                .checked_add(PAGE_SIZE)
                .ok_or(PageTableError::InvalidRange)?;
        }
    }

    map_kernel_section(root_table, kernel_layout.text, PTE_READ | PTE_EXECUTE)?;
    map_kernel_section(root_table, kernel_layout.rodata, PTE_READ)?;
    map_kernel_section(root_table, kernel_layout.data, PTE_READ | PTE_WRITE)?;
    map_kernel_section(root_table, kernel_layout.bss, PTE_READ | PTE_WRITE)?;

    // 首个进程地址空间激活前，启动 Rust 调用链仍在引导栈上；映射引导栈和当前进程栈。
    map_kernel_stack(root_table, kernel_layout.boot_stack)?;
    map_kernel_stack(root_table, kernel_stack)?;

    for (page_index, frame) in user_image.frames.iter().enumerate() {
        map_page(
            root_table,
            user_image.virtual_start + page_index * PAGE_SIZE,
            frame.start_address(),
            PTE_READ | PTE_WRITE | PTE_EXECUTE | PTE_USER,
        )?;
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
    let console_start = align_down_to_page(console_region.start);
    let console_end = align_up_to_page(
        console_region
            .start
            .checked_add(console_region.size)
            .ok_or(PageTableError::InvalidRange)?,
    )
    .ok_or(PageTableError::InvalidRange)?;
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
        let mapped_size = align_up_to_page(section.size).ok_or(PageTableError::InvalidRange)?;
        if !is_page_aligned(section.virtual_start)
            || !is_page_aligned(section.physical_start)
            || section.size == 0
            || section.virtual_start.checked_add(mapped_size).is_none()
            || section.physical_start.checked_add(mapped_size).is_none()
        {
            return Err(PageTableError::InvalidRange);
        }
    }

    if align_up_to_page(layout.text.virtual_start + layout.text.size)
        .ok_or(PageTableError::InvalidRange)?
        != layout.rodata.virtual_start
        || align_up_to_page(layout.rodata.virtual_start + layout.rodata.size)
            .ok_or(PageTableError::InvalidRange)?
            != layout.data.virtual_start
        || align_up_to_page(layout.data.virtual_start + layout.data.size)
            .ok_or(PageTableError::InvalidRange)?
            != layout.bss.virtual_start
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
    let mapping_size = align_up_to_page(section.size).ok_or(PageTableError::InvalidRange)?;
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

/// 让一个进程页表能访问调度切换期间可能仍在使用的任一内核栈。
pub(super) fn map_kernel_stacks(
    address_space: &mut AddressSpace,
    stacks: &[KernelStackMapping],
) -> Result<(), PageTableError> {
    let root_table = address_space.root_table()?;
    for stack in stacks {
        map_kernel_stack(root_table, *stack)?;
    }
    Ok(())
}

fn map_kernel_stack(
    root_table: *mut PageTable,
    stack: KernelStackMapping,
) -> Result<(), PageTableError> {
    if !is_page_aligned(stack.virtual_start)
        || !is_page_aligned(stack.physical_start)
        || stack.size == 0
        || !is_page_aligned(stack.size)
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

/// 切换到指定进程的 Sv39 根页表。
pub(super) fn activate_address_space(address_space: &AddressSpace) -> Result<(), PageTableError> {
    activate(address_space.root_address)
}

/// 切换到启动阶段保留的内核页表，以便最后一个进程退出后安全回收其页表。
pub(super) fn activate_boot_address_space() -> Result<(), PageTableError> {
    super::page_table::activate_boot_address_space()
}

/// 销毁一个非当前地址空间，并归还它拥有的根页表和中间页表页。
///
/// 返回归还的页表页数。叶子映射指向的内核、用户栈和程序镜像页不会释放。
pub(super) fn destroy_address_space(address_space: AddressSpace) -> Result<usize, PageTableError> {
    let root_address = address_space.root_address;
    if root_address == active_root_address() {
        return Err(PageTableError::ActiveAddressSpace);
    }

    // 先完整验证树中所有页表页，再开始释放，避免发现坏链接后留下半棵已拆除的树。
    let page_table_count = validate_page_table_tree(root_address, 2)?;
    let released_page_count = release_page_table_tree(root_address, 2)?;
    if released_page_count != page_table_count {
        return Err(PageTableError::PageTableCountMismatch);
    }

    Ok(released_page_count)
}

/// 持有一棵已建立、且尚未销毁的 Sv39 页表树。
/// 独占一个进程的 Sv39 页表树；必须先切离，再显式销毁以归还页表页。
#[must_use = "地址空间对象代表独占的页表树所有权，必须在切离后显式销毁"]
pub struct AddressSpace {
    root_address: usize,
}

impl AddressSpace {
    pub(super) const fn root_address(&self) -> usize {
        self.root_address
    }

    fn root_table(&mut self) -> Result<*mut PageTable, PageTableError> {
        let virtual_address = super::address::physical_to_kernel_virtual(self.root_address)
            .ok_or(PageTableError::InvalidRootTable)?;
        Ok(virtual_address as *mut PageTable)
    }
}

fn validate_page_table_tree(table_address: usize, level: usize) -> Result<usize, PageTableError> {
    if !crate::kernel::frame_allocator::is_allocated_frame_address(table_address) {
        return Err(PageTableError::InvalidChildTable);
    }

    let table_virtual_address = super::address::physical_to_kernel_virtual(table_address)
        .ok_or(PageTableError::InvalidChildTable)?;
    let table = table_virtual_address as *const PageTable;
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
    let table_virtual_address = super::address::physical_to_kernel_virtual(table_address)
        .ok_or(PageTableError::InvalidChildTable)?;
    let table = table_virtual_address as *mut PageTable;
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

fn ranges_overlap(
    first_start: usize,
    first_end: usize,
    second_start: usize,
    second_end: usize,
) -> bool {
    first_start < second_end && second_start < first_end
}
