//! 为当前单个用户任务建立最小 Sv39 恒等映射和 U/S 权限边界。
//!
//! 页表和页表页池均静态分配。本模块只映射启动后仍会访问的内核、用户程序、用户栈
//! 和 QEMU `virt` UART；尚不提供物理页分配器或多个独立地址空间。

const PAGE_SIZE: usize = 4096;
const PAGE_MASK: usize = PAGE_SIZE - 1;
const PAGE_TABLE_ENTRIES: usize = 512;
const MAX_PAGE_TABLES: usize = 8;
const SATP_SV39_MODE: usize = 8;
const SATP_MODE_SHIFT: usize = 60;
const UART_BASE: usize = 0x1000_0000;

const PTE_VALID: usize = 1 << 0;
const PTE_READ: usize = 1 << 1;
const PTE_WRITE: usize = 1 << 2;
const PTE_EXECUTE: usize = 1 << 3;
const PTE_USER: usize = 1 << 4;
const PTE_ACCESSED: usize = 1 << 6;
const PTE_DIRTY: usize = 1 << 7;

const SSTATUS_SUM: usize = 1 << 18;

#[repr(C, align(4096))]
struct PageTable([usize; PAGE_TABLE_ENTRIES]);

static mut PAGE_TABLES: [PageTable; MAX_PAGE_TABLES] =
    [const { PageTable([0; PAGE_TABLE_ENTRIES]) }; MAX_PAGE_TABLES];

/// 建立恒等映射并切换到 Sv39。
///
/// `kernel_end` 是内核映像及 BSS 的结束地址，`user_stack` 是当前任务专用栈区间，
/// `user_image` 是已清零并装入用户程序的完整固定槽位。调用后当前指令、内核栈和
/// 后续使用的内核数据仍由恒等映射覆盖。
pub fn initialize(
    kernel_end: usize,
    user_stack: (usize, usize),
    user_image: (usize, usize),
) -> Result<(), PageTableError> {
    let kernel_start = core::ptr::addr_of!(__kernel_start) as usize;
    let kernel_end = align_up(kernel_end)?;
    let stack_start = user_stack.0;
    let stack_end = user_stack.1;
    let image_start = user_image.0;
    let image_end = user_image.1;

    if kernel_start & PAGE_MASK != 0
        || stack_start & PAGE_MASK != 0
        || stack_end & PAGE_MASK != 0
        || image_start & PAGE_MASK != 0
        || image_end & PAGE_MASK != 0
        || kernel_end <= kernel_start
        || stack_end <= stack_start
        || image_end <= image_start
        || stack_start < kernel_start
        || stack_end > kernel_end
        || image_start < kernel_end
    {
        return Err(PageTableError::InvalidRange);
    }

    // 安全性：页表池是本模块独占的静态存储；初始化只在单 hart 启动路径执行一次。
    let page_tables = core::ptr::addr_of_mut!(PAGE_TABLES).cast::<PageTable>();
    // 安全性：页表池有固定长度，清零范围由对应静态数组长度确定。
    unsafe { core::ptr::write_bytes(page_tables, 0, MAX_PAGE_TABLES) };

    let mut next_table = 1;
    let root_table = page_tables;
    let mut kernel_page = kernel_start;
    while kernel_page < kernel_end {
        map_page(
            page_tables,
            root_table,
            &mut next_table,
            kernel_page,
            PTE_READ | PTE_WRITE | PTE_EXECUTE,
        )?;
        kernel_page += PAGE_SIZE;
    }

    let mut image_page = image_start;
    while image_page < image_end {
        map_page(
            page_tables,
            root_table,
            &mut next_table,
            image_page,
            PTE_READ | PTE_WRITE | PTE_EXECUTE | PTE_USER,
        )?;
        image_page += PAGE_SIZE;
    }

    let mut stack_page = stack_start;
    while stack_page < stack_end {
        map_page(
            page_tables,
            root_table,
            &mut next_table,
            stack_page,
            PTE_READ | PTE_WRITE | PTE_USER,
        )?;
        stack_page += PAGE_SIZE;
    }

    map_page(
        page_tables,
        root_table,
        &mut next_table,
        UART_BASE,
        PTE_READ | PTE_WRITE,
    )?;

    activate(root_table as usize)?;
    Ok(())
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
    page_tables: *mut PageTable,
    root_table: *mut PageTable,
    next_table: &mut usize,
    virtual_address: usize,
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
        // 安全性：当前表指针来自静态页表池，且索引经过 9 位掩码限制。
        let entry_value = unsafe { core::ptr::read(entry) };
        if entry_value & PTE_VALID == 0 {
            if *next_table >= MAX_PAGE_TABLES {
                return Err(PageTableError::PageTablePoolExhausted);
            }
            // 安全性：新页表索引受静态池长度检查约束；每张表恰好一页且按页对齐。
            let child = unsafe { page_tables.add(*next_table) };
            *next_table += 1;
            // 安全性：子页表地址来自内核静态页表池，低 12 位为零且物理地址可由 S-mode 访问。
            unsafe { core::ptr::write_bytes(child, 0, 1) };
            let child_physical_page = child as usize >> 12;
            // 安全性：当前 PTE 指针属于有效父页表；新子表地址满足 Sv39 PPN 编码布局。
            unsafe { core::ptr::write(entry, (child_physical_page << 10) | PTE_VALID) };
            table = child;
        } else {
            if entry_value & (PTE_READ | PTE_WRITE | PTE_EXECUTE) != 0 {
                return Err(PageTableError::UnexpectedSuperpage);
            }
            let child_address = ((entry_value >> 10) << 12) as *mut PageTable;
            table = child_address;
        }
    }

    // 安全性：最低层索引也由 9 位 VPN 字段计算，且表指针来自当前页表树。
    let leaf = unsafe { core::ptr::addr_of_mut!((*table).0[indices[2]]) };
    // 安全性：叶子 PTE 位于有效 L0 页表内；映射物理地址与虚拟地址恒等。
    unsafe {
        core::ptr::write(
            leaf,
            ((virtual_address >> 12) << 10)
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
    if root_physical_address & PAGE_MASK != 0 {
        return Err(PageTableError::InvalidRootTable);
    }

    let satp_value = (SATP_SV39_MODE << SATP_MODE_SHIFT) | (root_physical_address >> 12);
    // 安全性：根页表来自本模块的静态页表池，恒等映射已覆盖当前内核代码、栈和数据。
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

/// 描述静态页表初始化失败的原因。
#[derive(Clone, Copy)]
pub enum PageTableError {
    /// 映射范围为空、反向或没有按页对齐。
    InvalidRange,
    /// 静态页表池容量不足。
    PageTablePoolExhausted,
    /// 本实验只建立 4 KiB 叶子映射，却遇到大页叶子项。
    UnexpectedSuperpage,
    /// 根页表没有按 4 KiB 对齐。
    InvalidRootTable,
    /// 当前硬件没有接受 Sv39 模式。
    Sv39Unavailable,
}

impl PageTableError {
    /// 返回适合内核启动日志的错误说明。
    pub fn description(self) -> &'static str {
        match self {
            Self::InvalidRange => "页表映射范围无效或未按 4 KiB 对齐",
            Self::PageTablePoolExhausted => "静态页表池容量不足",
            Self::UnexpectedSuperpage => "页表中出现本阶段未支持的大页映射",
            Self::InvalidRootTable => "根页表地址未按 4 KiB 对齐",
            Self::Sv39Unavailable => "硬件没有接受 Sv39 satp 模式",
        }
    }
}

extern "C" {
    static __kernel_start: u8;
}
