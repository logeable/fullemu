//! 根据 FDT 描述的 RAM 范围分配和回收 4 KiB 页帧。
//!
//! 分配器元数据固定保存在内核 BSS，不依赖堆；堆可在初始化后从这里取得连续页帧作为后备区。
//! 当前单 hart 的调用发生在启动期或 S-mode 陷入处理路径，不会被可调度的 S-mode 定时器重入。

use fullemu::device_tree::memory_info::{FdtMemoryInfo, PhysicalMemoryRegion};

use super::memory::page::{align_down_to_page, align_up_to_page, is_page_aligned, PAGE_SIZE};
const MAX_FREE_RANGES: usize = 256;
const MAX_MEMORY_REGIONS: usize = 32;

static mut FRAME_ALLOCATOR: FrameAllocator = FrameAllocator::empty();

extern "C" {
    static __kernel_load_end_virtual: u8;
    static __user_program_load_virtual_start: u8;
    static __user_program_load_virtual_end: u8;
}

/// 表示页池中的一个物理页帧。
#[must_use = "页帧令牌代表独占的物理内存所有权"]
#[derive(Debug, PartialEq, Eq)]
pub(super) struct PhysicalFrame {
    start_address: usize,
}

impl PhysicalFrame {
    /// 返回页帧的物理起始地址。
    pub(super) const fn start_address(&self) -> usize {
        self.start_address
    }

    /// 将页帧所有权交给记录其物理地址的调用方。
    pub(super) fn into_address(self) -> usize {
        self.start_address
    }

    /// 从页帧地址恢复所有权令牌。
    ///
    /// # Safety
    ///
    /// 调用方必须保证自己独占该页帧，且没有其他资源继续持有或引用它。
    pub(super) unsafe fn from_address(address: usize) -> Result<Self, FrameError> {
        // 安全性：调用方保证独占所有权；单 hart 分配器不会在当前启动或陷入路径并发修改位图。
        unsafe { (*core::ptr::addr_of!(FRAME_ALLOCATOR)).frame_from_address(address) }
    }
}

/// 表示一段连续、由页帧分配器独占的物理页帧。
#[must_use = "连续页帧令牌代表独占的物理内存所有权"]
pub(super) struct PhysicalFrameRun {
    start_address: usize,
    frame_count: usize,
}

impl PhysicalFrameRun {
    /// 返回连续页帧的物理起始地址。
    pub(super) const fn start_address(&self) -> usize {
        self.start_address
    }

    /// 返回连续页帧的总字节数。
    pub(super) const fn size(&self) -> usize {
        self.frame_count * PAGE_SIZE
    }
}

#[derive(Clone, Copy)]
struct FrameRange {
    start_address: usize,
    frame_count: usize,
}

impl FrameRange {
    const EMPTY: Self = Self {
        start_address: 0,
        frame_count: 0,
    };

    fn end_address(self) -> usize {
        self.start_address + self.frame_count * PAGE_SIZE
    }
}

struct FrameAllocator {
    memory_regions: [PhysicalMemoryRegion; MAX_MEMORY_REGIONS],
    memory_region_count: usize,
    managed_ranges: [FrameRange; MAX_FREE_RANGES],
    managed_range_count: usize,
    free_ranges: [FrameRange; MAX_FREE_RANGES],
    free_range_count: usize,
    initialized: bool,
}

impl FrameAllocator {
    const fn empty() -> Self {
        Self {
            memory_regions: [PhysicalMemoryRegion { start: 0, size: 0 }; MAX_MEMORY_REGIONS],
            memory_region_count: 0,
            managed_ranges: [FrameRange::EMPTY; MAX_FREE_RANGES],
            managed_range_count: 0,
            free_ranges: [FrameRange::EMPTY; MAX_FREE_RANGES],
            free_range_count: 0,
            initialized: false,
        }
    }

    fn initialize(&mut self, memory_info: &FdtMemoryInfo) -> Result<(), FrameError> {
        self.validate_initialization_request(memory_info)?;

        let kernel_end = super::memory::address::kernel_virtual_to_physical(core::ptr::addr_of!(
            __kernel_load_end_virtual
        ) as usize);
        let program_start = super::memory::address::kernel_virtual_to_physical(
            core::ptr::addr_of!(__user_program_load_virtual_start) as usize,
        );
        let program_end = super::memory::address::kernel_virtual_to_physical(core::ptr::addr_of!(
            __user_program_load_virtual_end
        ) as usize);

        self.copy_and_sort_memory_regions(memory_info.memory_regions());
        for memory_index in 0..self.memory_region_count {
            let memory = self.memory_regions[memory_index];
            self.append_usable_frames_from_region(
                memory,
                memory_info,
                kernel_end,
                program_start,
                program_end,
            )?;
        }

        if self.managed_range_count == 0 {
            return Err(FrameError::NoUsableMemory);
        }
        self.free_ranges = self.managed_ranges;
        self.free_range_count = self.managed_range_count;
        self.initialized = true;
        Ok(())
    }

    fn validate_initialization_request(
        &self,
        memory_info: &FdtMemoryInfo,
    ) -> Result<(), FrameError> {
        if self.initialized {
            return Err(FrameError::AlreadyInitialized);
        }
        if memory_info.memory_regions().len() > MAX_MEMORY_REGIONS {
            return Err(FrameError::TooManyMemoryRegions);
        }
        Ok(())
    }

    fn copy_and_sort_memory_regions(&mut self, memory_regions: &[PhysicalMemoryRegion]) {
        self.memory_region_count = memory_regions.len();
        for (index, memory) in memory_regions.iter().copied().enumerate() {
            self.memory_regions[index] = memory;
        }
        for index in 1..self.memory_region_count {
            let memory = self.memory_regions[index];
            let mut insertion = index;
            while insertion > 0 && self.memory_regions[insertion - 1].start > memory.start {
                self.memory_regions[insertion] = self.memory_regions[insertion - 1];
                insertion -= 1;
            }
            self.memory_regions[insertion] = memory;
        }
    }

    fn append_usable_frames_from_region(
        &mut self,
        memory: PhysicalMemoryRegion,
        memory_info: &FdtMemoryInfo,
        kernel_end: usize,
        program_start: usize,
        program_end: usize,
    ) -> Result<(), FrameError> {
        let memory_start =
            usize::try_from(memory.start).map_err(|_| FrameError::InvalidMemoryRange)?;
        let start = memory_start.max(kernel_end);
        let end = memory
            .end_exclusive()
            .and_then(|end| usize::try_from(end).ok())
            .ok_or(FrameError::InvalidMemoryRange)?;
        let start = align_up_to_page(start).ok_or(FrameError::InvalidMemoryRange)?;
        let end = align_down_to_page(end);
        if start >= end {
            return Ok(());
        }

        let mut frame_start = start;
        while frame_start < end {
            let frame_end = frame_start
                .checked_add(PAGE_SIZE)
                .ok_or(FrameError::InvalidMemoryRange)?;
            let frame_region = PhysicalMemoryRegion {
                start: frame_start as u64,
                size: PAGE_SIZE as u64,
            };
            let overlaps_program = frame_start < program_end && program_start < frame_end;
            if !overlaps_program && memory_info.contains_unreserved_ram_region(frame_region) {
                self.append_managed_frame(frame_start)?;
            }
            frame_start = frame_end;
        }
        Ok(())
    }

    fn allocate(&mut self) -> Result<PhysicalFrame, FrameError> {
        let run = self.allocate_run(1)?;
        Ok(PhysicalFrame {
            start_address: run.start_address,
        })
    }

    fn allocate_run(&mut self, frame_count: usize) -> Result<PhysicalFrameRun, FrameError> {
        self.ensure_initialized()?;
        if frame_count == 0 {
            return Err(FrameError::ZeroFrameCount);
        }
        for index in 0..self.free_range_count {
            let range = self.free_ranges[index];
            if range.frame_count >= frame_count {
                let start_address = range.start_address;
                self.free_ranges[index].start_address += frame_count * PAGE_SIZE;
                self.free_ranges[index].frame_count -= frame_count;
                if self.free_ranges[index].frame_count == 0 {
                    self.remove_free_range(index);
                }
                return Ok(PhysicalFrameRun {
                    start_address,
                    frame_count,
                });
            }
        }
        Err(FrameError::OutOfFrames)
    }

    fn deallocate(&mut self, frame: PhysicalFrame) -> Result<(), FrameError> {
        self.ensure_initialized()?;
        if !self.is_managed_address(frame.start_address) {
            return Err(FrameError::FrameOutsidePool);
        }
        if self.is_free_address(frame.start_address) {
            return Err(FrameError::FrameAlreadyFree);
        }
        self.insert_free_range(frame.start_address, 1)
    }

    fn deallocate_run(&mut self, run: PhysicalFrameRun) -> Result<(), FrameError> {
        self.ensure_initialized()?;
        let end = run
            .start_address
            .checked_add(
                run.frame_count
                    .checked_mul(PAGE_SIZE)
                    .ok_or(FrameError::InvalidMemoryRange)?,
            )
            .ok_or(FrameError::InvalidMemoryRange)?;
        let managed = self.managed_ranges[..self.managed_range_count]
            .iter()
            .any(|range| run.start_address >= range.start_address && end <= range.end_address());
        if !managed || run.frame_count == 0 || !is_page_aligned(run.start_address) {
            return Err(FrameError::FrameOutsidePool);
        }
        self.insert_free_range(run.start_address, run.frame_count)
    }

    fn is_managed_address(&self, address: usize) -> bool {
        is_page_aligned(address)
            && self.managed_ranges[..self.managed_range_count]
                .iter()
                .any(|range| address >= range.start_address && address < range.end_address())
    }

    fn is_free_address(&self, address: usize) -> bool {
        self.free_ranges[..self.free_range_count]
            .iter()
            .any(|range| address >= range.start_address && address < range.end_address())
    }

    fn insert_free_range(
        &mut self,
        start_address: usize,
        frame_count: usize,
    ) -> Result<(), FrameError> {
        let end_address = start_address
            .checked_add(
                frame_count
                    .checked_mul(PAGE_SIZE)
                    .ok_or(FrameError::InvalidMemoryRange)?,
            )
            .ok_or(FrameError::InvalidMemoryRange)?;
        let mut index = 0;
        while index < self.free_range_count && self.free_ranges[index].start_address < start_address
        {
            index += 1;
        }
        if index > 0 && self.free_ranges[index - 1].end_address() > start_address {
            return Err(FrameError::FrameAlreadyFree);
        }
        if index < self.free_range_count && self.free_ranges[index].start_address < end_address {
            return Err(FrameError::FrameAlreadyFree);
        }

        let joins_previous =
            index > 0 && self.free_ranges[index - 1].end_address() == start_address;
        let joins_next =
            index < self.free_range_count && self.free_ranges[index].start_address == end_address;
        match (joins_previous, joins_next) {
            (true, true) => {
                self.free_ranges[index - 1].frame_count +=
                    frame_count + self.free_ranges[index].frame_count;
                self.remove_free_range(index);
            }
            (true, false) => self.free_ranges[index - 1].frame_count += frame_count,
            (false, true) => {
                self.free_ranges[index].start_address = start_address;
                self.free_ranges[index].frame_count += frame_count;
            }
            (false, false) => {
                if self.free_range_count == MAX_FREE_RANGES {
                    return Err(FrameError::TooManyFreeRanges);
                }
                self.free_ranges
                    .copy_within(index..self.free_range_count, index + 1);
                self.free_ranges[index] = FrameRange {
                    start_address,
                    frame_count,
                };
                self.free_range_count += 1;
            }
        }
        Ok(())
    }

    fn append_managed_frame(&mut self, address: usize) -> Result<(), FrameError> {
        if self.managed_range_count > 0 {
            let last = &mut self.managed_ranges[self.managed_range_count - 1];
            if last.end_address() == address {
                last.frame_count += 1;
                return Ok(());
            }
        }
        if self.managed_range_count == MAX_FREE_RANGES {
            return Err(FrameError::TooManyFreeRanges);
        }
        self.managed_ranges[self.managed_range_count] = FrameRange {
            start_address: address,
            frame_count: 1,
        };
        self.managed_range_count += 1;
        Ok(())
    }

    fn remove_free_range(&mut self, index: usize) {
        self.free_ranges
            .copy_within(index + 1..self.free_range_count, index);
        self.free_range_count -= 1;
        self.free_ranges[self.free_range_count] = FrameRange::EMPTY;
    }

    fn frame_from_address(&self, address: usize) -> Result<PhysicalFrame, FrameError> {
        self.ensure_initialized()?;
        if !self.is_managed_address(address) {
            return Err(FrameError::FrameOutsidePool);
        }
        if self.is_free_address(address) {
            return Err(FrameError::FrameAlreadyFree);
        }

        Ok(PhysicalFrame {
            start_address: address,
        })
    }

    fn free_count(&self) -> Result<usize, FrameError> {
        self.ensure_initialized()?;
        Ok(self.free_ranges[..self.free_range_count]
            .iter()
            .map(|range| range.frame_count)
            .sum())
    }

    fn is_allocated_frame_address(&self, address: usize) -> bool {
        if !self.initialized {
            return false;
        }

        self.is_managed_address(address) && !self.is_free_address(address)
    }

    fn ensure_initialized(&self) -> Result<(), FrameError> {
        if self.initialized {
            Ok(())
        } else {
            Err(FrameError::NotInitialized)
        }
    }
}

/// 根据 FDT RAM 范围建立物理页帧池，并排除启动期占用的内存。
pub(super) fn initialize(memory_info: &FdtMemoryInfo) -> Result<(), FrameError> {
    // 安全性：初始化只在启动期单 hart 路径执行，定时器和全局 S-mode 中断尚未开启。
    unsafe { (*core::ptr::addr_of_mut!(FRAME_ALLOCATOR)).initialize(memory_info) }
}

/// 分配一段连续页帧，不建立堆元数据；供早期堆初始化使用。
pub(super) fn allocate_zeroed_run(frame_count: usize) -> Result<PhysicalFrameRun, FrameError> {
    // 安全性：当前分配位于启动期或 S-mode 陷入路径，不会被定时器重入；项目仅启用单 hart。
    let run = unsafe { (*core::ptr::addr_of_mut!(FRAME_ALLOCATOR)).allocate_run(frame_count)? };
    let Some(virtual_address) =
        super::memory::address::physical_to_kernel_virtual(run.start_address)
    else {
        deallocate_run(run)?;
        return Err(FrameError::InvalidMemoryRange);
    };
    // 安全性：run 独占连续已分配页帧；当前启动页表或任务页表将该 RAM 映射到高半区地址。
    unsafe { core::ptr::write_bytes(virtual_address as *mut u8, 0, run.size()) };
    Ok(run)
}

/// 分配一个物理页帧。
///
/// 返回的页内容未初始化；读取前必须先写入有效内容。
pub(super) fn allocate_frame() -> Result<PhysicalFrame, FrameError> {
    // 安全性：当前分配只发生在启动期或 S-mode 陷入处理路径，不会被定时器重入；项目仅启用单 hart。
    unsafe { (*core::ptr::addr_of_mut!(FRAME_ALLOCATOR)).allocate() }
}

/// 分配一个物理页帧，并将整页内容清零。
pub(super) fn allocate_zeroed_frame() -> Result<PhysicalFrame, FrameError> {
    let frame = allocate_frame()?;
    let Some(virtual_address) =
        super::memory::address::physical_to_kernel_virtual(frame.start_address)
    else {
        deallocate_frame(frame)?;
        return Err(FrameError::InvalidMemoryRange);
    };
    // 安全性：frame 是当前调用独占持有的已分配页；当前页表将该 RAM 映射到高半区地址。
    unsafe {
        core::ptr::write_bytes(virtual_address as *mut u8, 0, PAGE_SIZE);
    }
    Ok(frame)
}

/// 释放此前由该分配器分配的页帧。
pub(super) fn deallocate_frame(frame: PhysicalFrame) -> Result<(), FrameError> {
    // 安全性：当前释放只发生在启动期或 S-mode 陷入处理路径，不会被定时器重入；项目仅启用单 hart。
    unsafe { (*core::ptr::addr_of_mut!(FRAME_ALLOCATOR)).deallocate(frame) }
}

/// 归还一段连续页帧。
pub(super) fn deallocate_run(run: PhysicalFrameRun) -> Result<(), FrameError> {
    // 安全性：调用方移交独占的连续页帧令牌；单 hart 分配器不会并发修改空闲区间。
    unsafe { (*core::ptr::addr_of_mut!(FRAME_ALLOCATOR)).deallocate_run(run) }
}

/// 返回页池中当前空闲页帧的数量。
pub(super) fn free_frame_count() -> Result<usize, FrameError> {
    // 安全性：当前读取只发生在启动期或 S-mode 陷入处理路径，不会被定时器重入；项目仅启用单 hart。
    unsafe { (*core::ptr::addr_of!(FRAME_ALLOCATOR)).free_count() }
}

/// 返回 FDT 描述的物理 RAM 范围，供地址空间建立内核高半区直接映射。
pub(super) fn physical_memory_regions() -> Result<&'static [PhysicalMemoryRegion], FrameError> {
    // 安全性：初始化完成后范围数组保持不变；返回切片不暴露分配器的可变状态。
    let allocator = unsafe { &*core::ptr::addr_of!(FRAME_ALLOCATOR) };
    allocator.ensure_initialized()?;
    Ok(&allocator.memory_regions[..allocator.memory_region_count])
}

/// 检查物理地址是否对应页池中当前已分配的页帧。
pub(crate) fn is_allocated_frame_address(address: usize) -> bool {
    // 安全性：当前检查只发生在启动期或 S-mode 陷入处理路径，不会被定时器重入；项目仅启用单 hart。
    unsafe { (*core::ptr::addr_of!(FRAME_ALLOCATOR)).is_allocated_frame_address(address) }
}

/// 检查页帧分配、对齐、释放复用和完整归还。
pub(super) fn check_allocation() -> Result<(), FrameError> {
    let initial_free_count = free_frame_count()?;

    let first = allocate_frame()?;
    let second = allocate_frame()?;
    let third = allocate_frame()?;
    if !is_page_aligned(first.start_address())
        || !is_page_aligned(second.start_address())
        || !is_page_aligned(third.start_address())
        || first == second
        || first == third
        || second == third
    {
        return Err(FrameError::InvalidAllocation);
    }

    let second_address = second.start_address();
    let Some(second_virtual_address) =
        super::memory::address::physical_to_kernel_virtual(second.start_address)
    else {
        return Err(FrameError::InvalidMemoryRange);
    };
    // 安全性：自检独占持有 second；当前页表将该 RAM 映射到高半区地址。
    unsafe { core::ptr::write_bytes(second_virtual_address as *mut u8, 0xa5, PAGE_SIZE) };
    deallocate_frame(second)?;
    let reused = allocate_zeroed_frame()?;
    if reused.start_address() != second_address {
        return Err(FrameError::FrameWasNotReused);
    }
    if !frame_is_zeroed(&reused) {
        return Err(FrameError::FrameNotZeroed);
    }

    deallocate_frame(first)?;
    deallocate_frame(reused)?;
    deallocate_frame(third)?;
    if free_frame_count()? != initial_free_count {
        return Err(FrameError::UnexpectedFreeCount);
    }

    crate::klog_info!(
        "物理页帧分配器自检通过：可用页数={}，页大小={} 字节；对齐、独立分配、脏页清零、释放复用和完整归还均符合预期",
        initial_free_count,
        PAGE_SIZE
    );
    Ok(())
}

fn frame_is_zeroed(frame: &PhysicalFrame) -> bool {
    let Some(virtual_address) =
        super::memory::address::physical_to_kernel_virtual(frame.start_address)
    else {
        return false;
    };
    // 安全性：调用方持有该页帧的唯一令牌；当前页表将该 RAM 映射到高半区地址。
    let bytes = unsafe { core::slice::from_raw_parts(virtual_address as *const u8, PAGE_SIZE) };
    bytes.iter().all(|byte| *byte == 0)
}

/// 描述 FDT 物理内存管理和页帧操作错误。
#[derive(Clone, Copy)]
pub(super) enum FrameError {
    /// FDT 描述的物理内存范围无效或地址换算溢出。
    InvalidMemoryRange,
    /// FDT 描述的 RAM 区域数量超过固定启动信息容量。
    TooManyMemoryRegions,
    /// 扣除内核、DTB 和固件保留区域后没有可分配页帧。
    NoUsableMemory,
    /// 可用物理内存被切分为过多区间。
    TooManyFreeRanges,
    /// 分配器尚未初始化。
    NotInitialized,
    /// 分配器已经初始化，不能重复清空分配状态。
    AlreadyInitialized,
    /// 连续页帧分配请求的数量为零。
    ZeroFrameCount,
    /// 所有可用页帧均已分配，或没有足够大的连续范围。
    OutOfFrames,
    /// 要释放的页帧不属于 FDT 确认的可用范围或地址未按页对齐。
    FrameOutsidePool,
    /// 页帧已经处于空闲状态。
    FrameAlreadyFree,
    /// 启动自检中分配结果违反唯一性或对齐要求。
    InvalidAllocation,
    /// 释放后的空闲页数与预期不符。
    UnexpectedFreeCount,
    /// 已释放的页帧没有被下一次分配复用。
    FrameWasNotReused,
    /// 已分配的清零页帧仍包含非零内容。
    FrameNotZeroed,
}

impl FrameError {
    /// 返回用于启动诊断的错误说明。
    pub(super) fn description(self) -> &'static str {
        match self {
            Self::InvalidMemoryRange => "FDT 物理内存范围无效或地址计算溢出",
            Self::TooManyMemoryRegions => "FDT RAM 区域数量超过物理页帧分配器容量",
            Self::NoUsableMemory => "FDT RAM 中没有可分配的物理页帧",
            Self::TooManyFreeRanges => "可用物理内存区间数量超过物理页帧分配器容量",
            Self::NotInitialized => "物理页帧分配器尚未初始化",
            Self::AlreadyInitialized => "物理页帧分配器重复初始化",
            Self::ZeroFrameCount => "连续页帧分配数量不能为零",
            Self::OutOfFrames => "可用物理页帧已耗尽或连续范围不足",
            Self::FrameOutsidePool => "待释放页帧不属于 FDT 确认的可用物理内存",
            Self::FrameAlreadyFree => "物理页帧已被释放",
            Self::InvalidAllocation => "页帧分配未满足对齐或唯一性要求",
            Self::UnexpectedFreeCount => "物理页帧自检后的空闲页数不符",
            Self::FrameWasNotReused => "已释放的物理页帧未被复用",
            Self::FrameNotZeroed => "重新分配的页帧未被完整清零",
        }
    }
}
