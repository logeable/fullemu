//! 在内核静态区域中管理一个有界、可释放的堆。
//!
//! 堆访问通过单 hart 中断守卫串行化，并作为 Rust 全局分配器管理内核对象字节块。
//! 当前只在启动阶段演示分配器，不将动态分配扩展到运行期任务路径。
//! 堆区位于内核 BSS，页表将它作为 S-mode 内核内存映射；物理页和用户地址空间仍由现有静态区域管理。

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;

const KERNEL_HEAP_SIZE: usize = 256 * 1024;
const ALLOCATION_MAGIC: u64 = 0x4655_4c4c_4845_4150;

#[repr(C, align(4096))]
struct HeapStorage([u8; KERNEL_HEAP_SIZE]);

static mut HEAP_STORAGE: HeapStorage = HeapStorage([0; KERNEL_HEAP_SIZE]);
static mut KERNEL_HEAP: Heap = Heap::empty();

struct KernelHeapAllocator;

#[global_allocator]
static KERNEL_HEAP_ALLOCATOR: KernelHeapAllocator = KernelHeapAllocator;

// 安全性：分配与释放都由屏蔽当前 hart S-mode 中断的入口串行化；项目当前只启用一个 hart。
unsafe impl GlobalAlloc for KernelHeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocate(layout).map_or(core::ptr::null_mut(), NonNull::as_ptr)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
        // 安全性：GlobalAlloc 调用方保证指针来自此分配器且尚未释放。
        let pointer = unsafe { NonNull::new_unchecked(pointer) };
        // 安全性：GlobalAlloc 契约保证传入指针是此堆返回的原始分配地址。
        if let Err(error) = unsafe { deallocate(pointer) } {
            panic!("内核全局分配器释放失败：{}", error.description());
        }
    }
}

#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

#[repr(C)]
struct AllocationHeader {
    block_start: usize,
    block_size: usize,
    magic: u64,
}

struct Heap {
    start: usize,
    end: usize,
    first_free_block: *mut FreeBlock,
}

impl Heap {
    const fn empty() -> Self {
        Self {
            start: 0,
            end: 0,
            first_free_block: core::ptr::null_mut(),
        }
    }

    /// 初始化堆区，并在其中建立一个覆盖全区域的空闲块。
    ///
    /// # Safety
    ///
    /// `start` 必须指向大小为 `size` 的可写独占区域，且该区域在堆使用期间持续有效。
    unsafe fn initialize(&mut self, start: usize, size: usize) -> Result<(), HeapError> {
        let alignment = core::mem::align_of::<FreeBlock>();
        let Some(end) = start.checked_add(size) else {
            return Err(HeapError::InvalidRegion);
        };
        if start == 0
            || start & (alignment - 1) != 0
            || size & (alignment - 1) != 0
            || size < core::mem::size_of::<FreeBlock>() + alignment
        {
            return Err(HeapError::InvalidRegion);
        }

        let first_free_block = start as *mut FreeBlock;
        // 安全性：调用方保证该对齐区域可写，且大小足以容纳一个空闲块头部。
        unsafe {
            core::ptr::write(
                first_free_block,
                FreeBlock {
                    size,
                    next: core::ptr::null_mut(),
                },
            );
        }
        self.start = start;
        self.end = end;
        self.first_free_block = first_free_block;
        Ok(())
    }

    fn allocate(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        if self.first_free_block.is_null() || layout.size() == 0 {
            return None;
        }

        let allocation_alignment = layout
            .align()
            .max(core::mem::align_of::<AllocationHeader>());
        let mut previous = core::ptr::null_mut();
        let mut block = self.first_free_block;

        while !block.is_null() {
            // 安全性：空闲链表只包含初始化或分裂时在堆区内建立的有效块。
            let (block_size, next_block) = unsafe { ((*block).size, (*block).next) };
            let block_start = block as usize;
            let Some(minimum_payload) = block_start
                .checked_add(core::mem::size_of::<FreeBlock>())
                .and_then(|value| value.checked_add(core::mem::size_of::<AllocationHeader>()))
            else {
                previous = block;
                block = next_block;
                continue;
            };
            let Some(payload_start) = align_up(minimum_payload, allocation_alignment) else {
                previous = block;
                block = next_block;
                continue;
            };
            let Some(required_end) = payload_start.checked_add(layout.size()) else {
                previous = block;
                block = next_block;
                continue;
            };
            let Some(allocated_size) = align_up(
                required_end - block_start,
                core::mem::align_of::<FreeBlock>(),
            ) else {
                previous = block;
                block = next_block;
                continue;
            };
            if allocated_size > block_size {
                previous = block;
                block = next_block;
                continue;
            }

            let remaining_size = block_size - allocated_size;
            let can_split = remaining_size
                >= core::mem::size_of::<FreeBlock>() + core::mem::align_of::<FreeBlock>();
            let actual_block_size = if can_split {
                let remaining_block = (block_start + allocated_size) as *mut FreeBlock;
                // 安全性：分裂位置按 FreeBlock 对齐，剩余长度足以容纳块头和数据。
                unsafe {
                    core::ptr::write(
                        remaining_block,
                        FreeBlock {
                            size: remaining_size,
                            next: next_block,
                        },
                    );
                }
                replace_free_block(previous, block, remaining_block, &mut self.first_free_block);
                allocated_size
            } else {
                remove_free_block(previous, next_block, &mut self.first_free_block);
                block_size
            };

            let allocation_header =
                (payload_start - core::mem::size_of::<AllocationHeader>()) as *mut AllocationHeader;
            // 安全性：头部位于当前分配块内部，且由堆分配器独占写入。
            unsafe {
                core::ptr::write(
                    allocation_header,
                    AllocationHeader {
                        block_start,
                        block_size: actual_block_size,
                        magic: ALLOCATION_MAGIC,
                    },
                );
            }
            return NonNull::new(payload_start as *mut u8);
        }

        None
    }

    /// 释放本堆此前分配的内存，并合并相邻空闲块。
    ///
    /// # Safety
    ///
    /// `pointer` 必须是当前堆返回且尚未释放的分配指针，不能传入偏移后的指针或其他堆的指针。
    unsafe fn deallocate(&mut self, pointer: NonNull<u8>) -> Result<(), HeapError> {
        let Some(header_address) =
            (pointer.as_ptr() as usize).checked_sub(core::mem::size_of::<AllocationHeader>())
        else {
            return Err(HeapError::InvalidAllocation);
        };
        // 安全性：安全契约要求指针来自本堆且未释放，因此其前置头部位于有效堆区域内。
        let allocation = unsafe { core::ptr::read(header_address as *const AllocationHeader) };
        let Some(block_end) = allocation.block_start.checked_add(allocation.block_size) else {
            return Err(HeapError::InvalidAllocation);
        };
        if allocation.magic != ALLOCATION_MAGIC
            || allocation.block_start < self.start
            || allocation.block_start & (core::mem::align_of::<FreeBlock>() - 1) != 0
            || allocation.block_size < core::mem::size_of::<FreeBlock>()
            || block_end > self.end
        {
            return Err(HeapError::InvalidAllocation);
        }

        let block_start = allocation.block_start;
        let mut previous = core::ptr::null_mut();
        let mut next = self.first_free_block;
        while !next.is_null() && (next as usize) < block_start {
            previous = next;
            // 安全性：遍历的节点来自本堆已维护的有序空闲链表。
            next = unsafe { (*next).next };
        }

        if !previous.is_null() {
            // 安全性：前驱是本堆空闲链表中的有效块。
            let previous_end = unsafe { (previous as usize).checked_add((*previous).size) };
            if previous_end.is_none_or(|end| end > block_start) {
                return Err(HeapError::InvalidAllocation);
            }
        }
        if !next.is_null() && block_end > next as usize {
            return Err(HeapError::InvalidAllocation);
        }

        let released_block = block_start as *mut FreeBlock;
        // 安全性：经范围、对齐和重叠检查后，释放区间可重新作为空闲块头部使用。
        unsafe {
            core::ptr::write(
                released_block,
                FreeBlock {
                    size: allocation.block_size,
                    next,
                },
            );
        }

        let merged_block = released_block;
        if !next.is_null() && block_end == next as usize {
            // 安全性：相邻空闲块均位于本堆内，合并后仍落在堆区边界内。
            unsafe {
                (*merged_block).size += (*next).size;
                (*merged_block).next = (*next).next;
            }
        }

        if !previous.is_null() {
            // 安全性：前驱是本堆空闲链表中的有效块。
            let previous_end = unsafe { previous as usize + (*previous).size };
            if previous_end == merged_block as usize {
                // 安全性：两个空闲块相邻且均已验证位于堆区内，合并不跨越边界。
                unsafe {
                    (*previous).size += (*merged_block).size;
                    (*previous).next = (*merged_block).next;
                }
            } else {
                // 安全性：前驱与释放块不重叠，按地址顺序将释放块接回空闲链表。
                unsafe { (*previous).next = merged_block };
            }
        } else {
            self.first_free_block = merged_block;
        }

        Ok(())
    }

    fn free_space(&self) -> (usize, usize) {
        let mut total_size = 0;
        let mut block_count = 0;
        let mut block = self.first_free_block;
        while !block.is_null() {
            // 安全性：空闲链表只包含堆区内由本分配器建立的有效节点。
            let (size, next) = unsafe { ((*block).size, (*block).next) };
            total_size += size;
            block_count += 1;
            block = next;
        }
        (total_size, block_count)
    }
}

fn initialize_kernel_heap(start: usize, size: usize) -> Result<(), HeapError> {
    let _interrupt_guard = crate::arch::riscv64::interrupt::disable_supervisor_interrupts();
    // 安全性：单 hart 中断已关闭，静态堆状态不会被定时器切换到的任务并发访问。
    unsafe { (*core::ptr::addr_of_mut!(KERNEL_HEAP)).initialize(start, size) }
}

fn allocate(layout: Layout) -> Option<NonNull<u8>> {
    let _interrupt_guard = crate::arch::riscv64::interrupt::disable_supervisor_interrupts();
    // 安全性：单 hart 中断已关闭，对静态堆状态的可变访问不会与其他任务重叠。
    unsafe { (*core::ptr::addr_of_mut!(KERNEL_HEAP)).allocate(layout) }
}

/// 释放由内核堆返回且尚未释放的指针。
///
/// # Safety
///
/// `pointer` 必须来自本内核堆，且不能已经释放或被偏移。
unsafe fn deallocate(pointer: NonNull<u8>) -> Result<(), HeapError> {
    let _interrupt_guard = crate::arch::riscv64::interrupt::disable_supervisor_interrupts();
    // 安全性：单 hart 中断已关闭，调用方保证指针属于堆且尚未释放。
    unsafe { (*core::ptr::addr_of_mut!(KERNEL_HEAP)).deallocate(pointer) }
}

fn free_space() -> (usize, usize) {
    let _interrupt_guard = crate::arch::riscv64::interrupt::disable_supervisor_interrupts();
    // 安全性：单 hart 中断已关闭，遍历空闲链表期间堆状态不会变化。
    unsafe { (*core::ptr::addr_of_mut!(KERNEL_HEAP)).free_space() }
}

fn replace_free_block(
    previous: *mut FreeBlock,
    old_block: *mut FreeBlock,
    new_block: *mut FreeBlock,
    first_free_block: &mut *mut FreeBlock,
) {
    if previous.is_null() {
        *first_free_block = new_block;
    } else {
        // 安全性：前驱来自当前空闲链表，且替换节点仍位于同一堆区。
        unsafe { (*previous).next = new_block };
    }
    // 安全性：新节点已初始化，旧节点来自当前空闲链表。
    unsafe { (*new_block).next = (*old_block).next };
}

fn remove_free_block(
    previous: *mut FreeBlock,
    next: *mut FreeBlock,
    first_free_block: &mut *mut FreeBlock,
) {
    if previous.is_null() {
        *first_free_block = next;
    } else {
        // 安全性：前驱来自当前空闲链表，next 是待移除节点的后继。
        unsafe { (*previous).next = next };
    }
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    value
        .checked_add(alignment - 1)
        .map(|aligned| aligned & !(alignment - 1))
}

/// 描述堆初始化或释放操作失败的原因。
#[derive(Clone, Copy)]
pub enum HeapError {
    /// 堆区域未对齐、容量不足或地址范围溢出。
    InvalidRegion,
    /// 分配头部无效、指针非法或释放区间与空闲块重叠。
    InvalidAllocation,
    /// 启动演示中的固定布局无法构造。
    InvalidDemoLayout,
    /// 启动演示中的分配请求无法满足。
    DemoOutOfMemory,
    /// 启动演示没有恢复为一个完整空闲块。
    DemoDidNotCoalesce,
    /// Vec 没有发生预期的容量增长。
    ContainerDidNotGrow,
    /// Vec 分配期间没有占用内核堆空间。
    ContainerDidNotUseHeap,
    /// Vec 释放后没有恢复完整空闲块。
    ContainerDidNotRelease,
}

impl HeapError {
    /// 返回用于启动诊断的错误说明。
    pub fn description(self) -> &'static str {
        match self {
            Self::InvalidRegion => "内核堆区域无效",
            Self::InvalidAllocation => "内核堆释放指针无效或重复释放",
            Self::InvalidDemoLayout => "内核堆演示布局无效",
            Self::DemoOutOfMemory => "内核堆演示分配失败",
            Self::DemoDidNotCoalesce => "内核堆释放后没有合并为空闲区域",
            Self::ContainerDidNotGrow => "Vec 容量没有按预期增长",
            Self::ContainerDidNotUseHeap => "Vec 没有使用内核堆空间",
            Self::ContainerDidNotRelease => "Vec 释放后内核堆没有恢复",
        }
    }
}

/// 初始化固定内核堆。
///
/// 必须在任何堆分配之前调用，并且只调用一次。
pub fn initialize() -> Result<(), HeapError> {
    // 安全性：HEAP_STORAGE 是独占的静态可写 BSS 区域，生命周期覆盖整个内核运行期。
    let heap_start = unsafe { core::ptr::addr_of_mut!(HEAP_STORAGE.0).cast::<u8>() as usize };
    // 安全性：起始地址指向完整静态数组，固定容量与该数组长度一致。
    initialize_kernel_heap(heap_start, KERNEL_HEAP_SIZE)
}

/// 演示对齐、释放、地址复用和相邻空闲块合并。
pub fn run_allocator_experiment() -> Result<(), HeapError> {
    let heap_start = unsafe { core::ptr::addr_of!(HEAP_STORAGE.0).cast::<u8>() as usize };

    let first_layout = Layout::from_size_align(37, 16).map_err(|_| HeapError::InvalidDemoLayout)?;
    let second_layout =
        Layout::from_size_align(113, 64).map_err(|_| HeapError::InvalidDemoLayout)?;
    let first = allocate(first_layout).ok_or(HeapError::DemoOutOfMemory)?;
    let second = allocate(second_layout).ok_or(HeapError::DemoOutOfMemory)?;
    if first.as_ptr() == second.as_ptr()
        || first.as_ptr() as usize & (first_layout.align() - 1) != 0
        || second.as_ptr() as usize & (second_layout.align() - 1) != 0
    {
        return Err(HeapError::InvalidAllocation);
    }

    // 安全性：first 由当前堆分配，且此前尚未释放。
    unsafe { deallocate(first)? };
    let reused = allocate(first_layout).ok_or(HeapError::DemoOutOfMemory)?;
    if reused != first {
        return Err(HeapError::InvalidAllocation);
    }

    // 安全性：second 和 reused 均由当前堆分配，且此前尚未释放。
    unsafe {
        deallocate(second)?;
        deallocate(reused)?;
    }
    if free_space() != (KERNEL_HEAP_SIZE, 1) {
        return Err(HeapError::DemoDidNotCoalesce);
    }

    crate::klog_info!(
        "内核堆实验完成：区域起始={heap_start:#018x}，容量={} 字节；堆操作经过单 hart 中断守卫，对齐、释放、复用和空闲块合并均通过",
        KERNEL_HEAP_SIZE
    );
    Ok(())
}

/// 使用 `Vec` 演示全局分配器的扩容与释放。
pub fn run_vec_experiment() -> Result<(), HeapError> {
    let initial_free = free_space();
    if initial_free != (KERNEL_HEAP_SIZE, 1) {
        return Err(HeapError::DemoDidNotCoalesce);
    }

    let mut values = alloc::vec::Vec::new();
    let mut previous_capacity = values.capacity();
    let mut growth_count = 0;
    for value in 0..128_u64 {
        values.push(value);
        if values.capacity() != previous_capacity {
            growth_count += 1;
            previous_capacity = values.capacity();
        }
    }

    let final_length = values.len();
    let final_capacity = values.capacity();
    let during_allocation = free_space();
    drop(values);
    let after_release = free_space();

    if growth_count == 0 || final_capacity < final_length {
        return Err(HeapError::ContainerDidNotGrow);
    }
    if during_allocation.0 >= initial_free.0 {
        return Err(HeapError::ContainerDidNotUseHeap);
    }
    if after_release != initial_free {
        return Err(HeapError::ContainerDidNotRelease);
    }

    crate::klog_info!(
        "Vec 全局分配实验完成：元素={}，容量={}，扩容次数={}，空闲字节={} -> {} -> {}；扩容和释放均通过",
        final_length,
        final_capacity,
        growth_count,
        initial_free.0,
        during_allocation.0,
        after_release.0
    );
    Ok(())
}
