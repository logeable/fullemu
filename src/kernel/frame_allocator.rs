//! 从固定的物理页池中分配和回收 4 KiB 页帧。
//!
//! 当前页池静态保留在内核 BSS 中，位图记录每一页的占用状态。它与按字节分配内核对象的
//! `heap` 分离；本阶段只演示页帧所有权，不替换 Sv39 页表和用户栈的静态存储。

const PAGE_SIZE: usize = 4096;
const FRAME_COUNT: usize = 128;
const BITMAP_WORD_BITS: usize = usize::BITS as usize;
const BITMAP_WORD_COUNT: usize = FRAME_COUNT.div_ceil(BITMAP_WORD_BITS);
const FRAME_POOL_SIZE: usize = FRAME_COUNT * PAGE_SIZE;

#[repr(C, align(4096))]
struct FrameStorage([[u8; PAGE_SIZE]; FRAME_COUNT]);

static mut FRAME_STORAGE: FrameStorage = FrameStorage([[0; PAGE_SIZE]; FRAME_COUNT]);
static mut FRAME_ALLOCATOR: FrameAllocator = FrameAllocator::empty();

/// 表示页池中的一个物理页帧。
#[derive(Debug, PartialEq, Eq)]
pub struct PhysicalFrame {
    start_address: usize,
}

impl PhysicalFrame {
    /// 返回页帧的物理起始地址。
    pub const fn start_address(&self) -> usize {
        self.start_address
    }
}

struct FrameAllocator {
    start_address: usize,
    allocated: [usize; BITMAP_WORD_COUNT],
    initialized: bool,
}

impl FrameAllocator {
    const fn empty() -> Self {
        Self {
            start_address: 0,
            allocated: [0; BITMAP_WORD_COUNT],
            initialized: false,
        }
    }

    fn initialize(&mut self, start_address: usize) -> Result<(), FrameError> {
        if self.initialized {
            return Err(FrameError::AlreadyInitialized);
        }
        if start_address == 0 || start_address & (PAGE_SIZE - 1) != 0 {
            return Err(FrameError::InvalidPool);
        }
        if start_address.checked_add(FRAME_POOL_SIZE).is_none() {
            return Err(FrameError::InvalidPool);
        }

        self.start_address = start_address;
        self.allocated = [0; BITMAP_WORD_COUNT];
        self.initialized = true;
        Ok(())
    }

    fn allocate(&mut self) -> Result<PhysicalFrame, FrameError> {
        self.ensure_initialized()?;

        for frame_index in 0..FRAME_COUNT {
            let word_index = frame_index / BITMAP_WORD_BITS;
            let bit_index = frame_index % BITMAP_WORD_BITS;
            let bit_mask = 1usize << bit_index;
            if self.allocated[word_index] & bit_mask == 0 {
                self.allocated[word_index] |= bit_mask;
                return Ok(PhysicalFrame {
                    start_address: self.start_address + frame_index * PAGE_SIZE,
                });
            }
        }

        Err(FrameError::OutOfFrames)
    }

    fn deallocate(&mut self, frame: PhysicalFrame) -> Result<(), FrameError> {
        self.ensure_initialized()?;

        let Some(offset) = frame.start_address.checked_sub(self.start_address) else {
            return Err(FrameError::FrameOutsidePool);
        };
        if offset >= FRAME_POOL_SIZE || offset & (PAGE_SIZE - 1) != 0 {
            return Err(FrameError::FrameOutsidePool);
        }

        let frame_index = offset / PAGE_SIZE;
        let word_index = frame_index / BITMAP_WORD_BITS;
        let bit_index = frame_index % BITMAP_WORD_BITS;
        let bit_mask = 1usize << bit_index;
        if self.allocated[word_index] & bit_mask == 0 {
            return Err(FrameError::FrameAlreadyFree);
        }

        self.allocated[word_index] &= !bit_mask;
        Ok(())
    }

    fn free_count(&self) -> Result<usize, FrameError> {
        self.ensure_initialized()?;
        Ok(FRAME_COUNT
            - self
                .allocated
                .iter()
                .map(|word| word.count_ones() as usize)
                .sum::<usize>())
    }

    fn ensure_initialized(&self) -> Result<(), FrameError> {
        if self.initialized {
            Ok(())
        } else {
            Err(FrameError::NotInitialized)
        }
    }
}

/// 初始化固定页帧池；必须在分配或释放页帧前调用，并且只调用一次。
pub fn initialize() -> Result<(), FrameError> {
    // 安全性：FrameStorage 是页对齐的静态可写区域，大小恰为 FRAME_POOL_SIZE。
    let pool_start = unsafe { core::ptr::addr_of_mut!(FRAME_STORAGE.0).cast::<u8>() as usize };
    let _interrupt_guard = crate::arch::riscv64::interrupt::disable_supervisor_interrupts();
    // 安全性：单 hart 中断已关闭，初始化期间不会有其他内核路径访问分配器状态。
    unsafe { (*core::ptr::addr_of_mut!(FRAME_ALLOCATOR)).initialize(pool_start) }
}

/// 分配一个物理页帧。
pub fn allocate_frame() -> Result<PhysicalFrame, FrameError> {
    let _interrupt_guard = crate::arch::riscv64::interrupt::disable_supervisor_interrupts();
    // 安全性：单 hart 中断已关闭，位图更新不会与其他任务并发执行。
    unsafe { (*core::ptr::addr_of_mut!(FRAME_ALLOCATOR)).allocate() }
}

/// 释放此前由该分配器分配的页帧。
pub fn deallocate_frame(frame: PhysicalFrame) -> Result<(), FrameError> {
    let _interrupt_guard = crate::arch::riscv64::interrupt::disable_supervisor_interrupts();
    // 安全性：单 hart 中断已关闭，位图更新不会与其他任务并发执行。
    unsafe { (*core::ptr::addr_of_mut!(FRAME_ALLOCATOR)).deallocate(frame) }
}

/// 返回页池中当前空闲页帧的数量。
pub fn free_frame_count() -> Result<usize, FrameError> {
    let _interrupt_guard = crate::arch::riscv64::interrupt::disable_supervisor_interrupts();
    // 安全性：单 hart 中断已关闭，读取位图期间分配状态不会变化。
    unsafe { (*core::ptr::addr_of!(FRAME_ALLOCATOR)).free_count() }
}

/// 演示页帧分配、对齐、释放复用和完整归还。
pub fn run_experiment() -> Result<(), FrameError> {
    if free_frame_count()? != FRAME_COUNT {
        return Err(FrameError::UnexpectedFreeCount);
    }

    let first = allocate_frame()?;
    let second = allocate_frame()?;
    let third = allocate_frame()?;
    if first.start_address() & (PAGE_SIZE - 1) != 0
        || second.start_address() & (PAGE_SIZE - 1) != 0
        || third.start_address() & (PAGE_SIZE - 1) != 0
        || first == second
        || first == third
        || second == third
    {
        return Err(FrameError::InvalidAllocation);
    }

    let second_address = second.start_address();
    deallocate_frame(second)?;
    let reused = allocate_frame()?;
    if reused.start_address() != second_address {
        return Err(FrameError::FrameWasNotReused);
    }

    deallocate_frame(first)?;
    deallocate_frame(reused)?;
    deallocate_frame(third)?;
    if free_frame_count()? != FRAME_COUNT {
        return Err(FrameError::UnexpectedFreeCount);
    }

    crate::klog_info!(
        "物理页帧实验完成：页池起始={:#018x}，页数={}，页大小={} 字节；对齐、独立分配、释放复用和完整归还均通过",
        core::ptr::addr_of!(FRAME_STORAGE) as usize,
        FRAME_COUNT,
        PAGE_SIZE
    );
    Ok(())
}

/// 描述固定物理页池的初始化和页帧操作错误。
#[derive(Clone, Copy)]
pub enum FrameError {
    /// 页池地址未对齐或其地址范围溢出。
    InvalidPool,
    /// 分配器尚未初始化。
    NotInitialized,
    /// 分配器已经初始化，不能重复清空分配状态。
    AlreadyInitialized,
    /// 页池中的所有页帧均已分配。
    OutOfFrames,
    /// 要释放的页帧不属于当前页池或地址未按页对齐。
    FrameOutsidePool,
    /// 页帧已经处于空闲状态。
    FrameAlreadyFree,
    /// 启动演示中分配结果违反唯一性或对齐要求。
    InvalidAllocation,
    /// 释放后的空闲页数与预期不符。
    UnexpectedFreeCount,
    /// 已释放的页帧没有被下一次分配复用。
    FrameWasNotReused,
}

impl FrameError {
    /// 返回用于启动诊断的错误说明。
    pub fn description(self) -> &'static str {
        match self {
            Self::InvalidPool => "物理页帧池地址无效",
            Self::NotInitialized => "物理页帧分配器尚未初始化",
            Self::AlreadyInitialized => "物理页帧分配器重复初始化",
            Self::OutOfFrames => "物理页帧池已耗尽",
            Self::FrameOutsidePool => "待释放页帧不属于物理页池",
            Self::FrameAlreadyFree => "物理页帧已被释放",
            Self::InvalidAllocation => "页帧分配未满足对齐或唯一性要求",
            Self::UnexpectedFreeCount => "物理页帧实验后的空闲页数不符",
            Self::FrameWasNotReused => "已释放的物理页帧未被复用",
        }
    }
}
