//! 为每个用户任务建立独立 Sv39 地址空间和 U/S 权限边界。
//!
//! 页表和页表页池均静态分配。每个任务有独立根页表，内核、用户栈和 QEMU `virt` UART
//! 使用恒等映射；共同的用户程序虚拟区域映射到任务自己的物理镜像槽位。尚不提供物理页分配器。

const PAGE_SIZE: usize = 4096;
const PAGE_MASK: usize = PAGE_SIZE - 1;
const PAGE_TABLE_ENTRIES: usize = 512;
const MAX_USER_ADDRESS_SPACES: usize = 8;
const PAGE_TABLES_PER_ADDRESS_SPACE: usize = 8;
const TOTAL_PAGE_TABLES: usize = MAX_USER_ADDRESS_SPACES * PAGE_TABLES_PER_ADDRESS_SPACE;
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

static mut PAGE_TABLES: [PageTable; TOTAL_PAGE_TABLES] =
    [const { PageTable([0; PAGE_TABLE_ENTRIES]) }; TOTAL_PAGE_TABLES];
static mut ADDRESS_SPACE_COUNT: usize = 0;

/// 描述某个任务中用户程序虚拟区域到其独立物理镜像槽位的映射。
#[derive(Clone, Copy)]
pub(crate) struct UserImageMapping {
    pub virtual_start: usize,
    pub physical_start: usize,
    pub size: usize,
}

/// 为每个任务建立独立页表，并激活第一个任务的地址空间。
///
/// `kernel_end` 是内核映像及 BSS 的结束地址；两个切片按任务索引对应，分别描述该任务
/// 的用户栈和用户程序映射。所有程序使用相同虚拟起始地址，但映射到各自物理镜像槽位。
/// 所有页表都映射相同的 S-mode 内核区域，每张表只映射对应任务自己的 U-mode 区域。
pub fn initialize(
    kernel_end: usize,
    user_stacks: &[(usize, usize)],
    user_images: &[UserImageMapping],
) -> Result<(), PageTableError> {
    let kernel_start = core::ptr::addr_of!(__kernel_start) as usize;
    let kernel_end = align_up(kernel_end)?;

    if kernel_start & PAGE_MASK != 0
        || kernel_end <= kernel_start
        || user_stacks.is_empty()
        || user_stacks.len() > MAX_USER_ADDRESS_SPACES
        || user_stacks.len() != user_images.len()
    {
        return Err(PageTableError::InvalidRange);
    }

    for (index, &(stack_start, stack_end)) in user_stacks.iter().enumerate() {
        if stack_start & PAGE_MASK != 0
            || stack_end & PAGE_MASK != 0
            || stack_start < kernel_start
            || stack_end > kernel_end
            || stack_end <= stack_start
            || user_stacks[..index]
                .iter()
                .any(|&(other_start, other_end)| {
                    ranges_overlap(stack_start, stack_end, other_start, other_end)
                })
        {
            return Err(PageTableError::InvalidRange);
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

    // 安全性：页表池由本模块独占，初始化仅在单 hart 启动路径执行一次。
    let all_page_tables = core::ptr::addr_of_mut!(PAGE_TABLES).cast::<PageTable>();
    // 安全性：清零范围由静态页表池的固定长度确定。
    unsafe { core::ptr::write_bytes(all_page_tables, 0, TOTAL_PAGE_TABLES) };

    for (address_space_index, (&(stack_start, stack_end), image)) in
        user_stacks.iter().zip(user_images).enumerate()
    {
        // 安全性：索引来自已经限制在静态地址空间数量内的输入切片。
        let page_tables =
            unsafe { all_page_tables.add(address_space_index * PAGE_TABLES_PER_ADDRESS_SPACE) };
        let root_table = page_tables;
        let mut next_table = 1;

        let mut kernel_page = kernel_start;
        while kernel_page < kernel_end {
            map_page(
                page_tables,
                root_table,
                &mut next_table,
                kernel_page,
                kernel_page,
                PTE_READ | PTE_WRITE | PTE_EXECUTE,
            )?;
            kernel_page += PAGE_SIZE;
        }

        let mut image_offset = 0;
        while image_offset < image.size {
            map_page(
                page_tables,
                root_table,
                &mut next_table,
                image.virtual_start + image_offset,
                image.physical_start + image_offset,
                PTE_READ | PTE_WRITE | PTE_EXECUTE | PTE_USER,
            )?;
            image_offset += PAGE_SIZE;
        }

        let mut stack_page = stack_start;
        while stack_page < stack_end {
            map_page(
                page_tables,
                root_table,
                &mut next_table,
                stack_page,
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
            UART_BASE,
            PTE_READ | PTE_WRITE,
        )?;
    }

    // 安全性：地址空间数量不超过根表数组容量，后续调度只激活这些已完整构造的页表。
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(ADDRESS_SPACE_COUNT),
            user_stacks.len(),
        );
    }
    activate_address_space(0)?;
    Ok(())
}

/// 切换到指定任务的 Sv39 根页表。
pub fn activate_address_space(address_space_index: usize) -> Result<(), PageTableError> {
    // 安全性：地址空间数量仅在全部根页表初始化完成后写入，调度器传入任务索引。
    let address_space_count =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!(ADDRESS_SPACE_COUNT)) };
    if address_space_index >= address_space_count {
        return Err(PageTableError::UnknownAddressSpace);
    }

    // 安全性：该根页表是静态池中按任务索引分配、已完成初始化且页对齐的一页。
    let root_table = unsafe {
        core::ptr::addr_of!(PAGE_TABLES)
            .cast::<PageTable>()
            .add(address_space_index * PAGE_TABLES_PER_ADDRESS_SPACE)
    };
    activate(root_table as usize)
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
    page_tables: *mut PageTable,
    root_table: *mut PageTable,
    next_table: &mut usize,
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
        // 安全性：当前表指针来自静态页表池，且索引经过 9 位掩码限制。
        let entry_value = unsafe { core::ptr::read(entry) };
        if entry_value & PTE_VALID == 0 {
            if *next_table >= PAGE_TABLES_PER_ADDRESS_SPACE {
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
    /// 调度器请求了尚未初始化的地址空间。
    UnknownAddressSpace,
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
            Self::UnknownAddressSpace => "任务地址空间尚未初始化",
            Self::UnexpectedSuperpage => "页表中出现本阶段未支持的大页映射",
            Self::InvalidRootTable => "根页表地址未按 4 KiB 对齐",
            Self::Sv39Unavailable => "硬件没有接受 Sv39 satp 模式",
        }
    }
}

extern "C" {
    static __kernel_start: u8;
}
