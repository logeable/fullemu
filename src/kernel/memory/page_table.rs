//! 实现 Sv39 页表项的建立、地址空间激活和页地址计算。

use super::page::is_page_aligned;
use super::PageTableError;

pub(super) const PAGE_TABLE_ENTRIES: usize = 512;
const SATP_SV39_MODE: usize = 8;
const SATP_MODE_SHIFT: usize = 60;
const SATP_PPN_MASK: usize = (1 << 44) - 1;

pub(super) const PTE_VALID: usize = 1 << 0;
pub(super) const PTE_READ: usize = 1 << 1;
pub(super) const PTE_WRITE: usize = 1 << 2;
pub(super) const PTE_EXECUTE: usize = 1 << 3;
pub(super) const PTE_USER: usize = 1 << 4;
pub(super) const PTE_ACCESSED: usize = 1 << 6;
pub(super) const PTE_DIRTY: usize = 1 << 7;

#[repr(C, align(4096))]
pub(super) struct PageTable(pub(super) [usize; PAGE_TABLE_ENTRIES]);

pub(super) fn map_page(
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
            let child_address = child_frame.start_address();
            let Some(child_virtual_address) =
                super::address::physical_to_kernel_virtual(child_address)
            else {
                crate::kernel::frame_allocator::deallocate_frame(child_frame)
                    .map_err(PageTableError::FrameDeallocation)?;
                return Err(PageTableError::InvalidRange);
            };
            let child_address = child_frame.into_address();
            let child = child_virtual_address as *mut PageTable;
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
            let child_virtual_address = super::address::physical_to_kernel_virtual(child_address)
                .ok_or(PageTableError::InvalidChildTable)?;
            table = child_virtual_address as *mut PageTable;
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

pub(super) fn activate(root_physical_address: usize) -> Result<(), PageTableError> {
    if root_physical_address == 0
        || !is_page_aligned(root_physical_address)
        || !crate::kernel::frame_allocator::is_allocated_frame_address(root_physical_address)
    {
        return Err(PageTableError::InvalidRootTable);
    }

    activate_root(root_physical_address)
}

pub(in crate::kernel) fn activate_boot_address_space() -> Result<(), PageTableError> {
    let boot_root_virtual_address = core::ptr::addr_of!(__boot_page_table_virtual_start) as usize;
    let root_physical_address =
        super::address::kernel_virtual_to_physical(boot_root_virtual_address);
    if root_physical_address == 0 || !is_page_aligned(root_physical_address) {
        return Err(PageTableError::InvalidRootTable);
    }
    activate_root(root_physical_address)
}

fn activate_root(root_physical_address: usize) -> Result<(), PageTableError> {
    let satp_value = (SATP_SV39_MODE << SATP_MODE_SHIFT) | (root_physical_address >> 12);
    // 安全性：启动页表位于内核静态存储区，进程页表根来自已分配页帧；切换前后的页表都映射当前高半区内核代码与 RAM 直接映射区。
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

extern "C" {
    static __boot_page_table_virtual_start: u8;
}

pub(super) fn active_root_address() -> usize {
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
