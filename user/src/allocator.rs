//! 通过 `brk` 扩展进程堆，并在用户态管理 `alloc` 容器需要的对象内存。
//!
//! 堆空间按页向内核申请；空闲链表将已经取得的空间切分给对象，并在释放时合并相邻块。
//! 当前每个进程只有一个执行流，因此分配器状态不需要锁；`fork` 会随进程内存一起复制该状态。

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;

const HEAP_GROWTH_SIZE: usize = 4096;
const PAGE_SIZE: usize = 4096;
const ALLOCATION_MAGIC: u64 = 0x4655_4c4c_5553_4552;

static mut USER_HEAP: UserHeap = UserHeap::empty();

struct UserHeapAllocator;

#[global_allocator]
static USER_HEAP_ALLOCATOR: UserHeapAllocator = UserHeapAllocator;

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

struct UserHeap {
    start: usize,
    end: usize,
    first_free_block: *mut FreeBlock,
    initialized: bool,
}

impl UserHeap {
    const fn empty() -> Self {
        Self {
            start: 0,
            end: 0,
            first_free_block: core::ptr::null_mut(),
            initialized: false,
        }
    }

    fn allocate(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        if layout.size() == 0 || !self.initialize() {
            return None;
        }

        if let Some(pointer) = self.allocate_from_free_blocks(layout) {
            return Some(pointer);
        }
        if !self.grow(layout) {
            return None;
        }
        self.allocate_from_free_blocks(layout)
    }

    fn initialize(&mut self) -> bool {
        if self.initialized {
            return true;
        }

        let initial_break = super::syscall::brk(0);
        if initial_break == 0 {
            return false;
        }
        self.start = initial_break;
        self.end = initial_break;
        self.initialized = true;
        true
    }

    fn grow(&mut self, layout: Layout) -> bool {
        let required_size = core::mem::size_of::<FreeBlock>()
            .checked_add(core::mem::size_of::<AllocationHeader>())
            .and_then(|size| size.checked_add(layout.align()))
            .and_then(|size| size.checked_add(layout.size()));
        let Some(required_size) = required_size else {
            return false;
        };
        let growth_size = required_size.max(HEAP_GROWTH_SIZE);
        let Some(unrounded_end) = self.end.checked_add(growth_size) else {
            return false;
        };
        let Some(new_end) = align_up(unrounded_end, PAGE_SIZE) else {
            return false;
        };

        if super::syscall::brk(new_end) != new_end {
            return false;
        }

        let Some(block_start) = align_up(self.end, core::mem::align_of::<FreeBlock>()) else {
            panic!("用户堆空闲块起始地址溢出");
        };
        let block_size = new_end - block_start;
        self.end = new_end;
        if !self.insert_free_block(block_start, block_size) {
            panic!("用户堆扩展区域无法加入空闲链表");
        }
        true
    }

    fn allocate_from_free_blocks(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        let allocation_alignment = layout
            .align()
            .max(core::mem::align_of::<AllocationHeader>());
        let mut previous = core::ptr::null_mut();
        let mut block = self.first_free_block;

        while !block.is_null() {
            // 安全性：空闲链表节点仅由本分配器在已映射堆区内建立和维护。
            let (block_size, next_block) = unsafe { ((*block).size, (*block).next) };
            let block_start = block as usize;
            let Some(minimum_payload) = block_start
                .checked_add(core::mem::size_of::<FreeBlock>())
                .and_then(|address| address.checked_add(core::mem::size_of::<AllocationHeader>()))
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
            let minimum_free_size =
                core::mem::size_of::<FreeBlock>() + core::mem::align_of::<FreeBlock>();
            let actual_block_size = if remaining_size >= minimum_free_size {
                let remaining_block = (block_start + allocated_size) as *mut FreeBlock;
                // 安全性：分裂位置按 FreeBlock 对齐，剩余区域足以容纳下一个空闲块。
                unsafe {
                    core::ptr::write(
                        remaining_block,
                        FreeBlock {
                            size: remaining_size,
                            next: next_block,
                        },
                    );
                }
                replace_free_block(previous, remaining_block, &mut self.first_free_block);
                allocated_size
            } else {
                remove_free_block(previous, next_block, &mut self.first_free_block);
                block_size
            };

            let header =
                (payload_start - core::mem::size_of::<AllocationHeader>()) as *mut AllocationHeader;
            // 安全性：分配头位于已从空闲链表摘除的当前分配块内部。
            unsafe {
                core::ptr::write(
                    header,
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

    /// 释放此前由本分配器返回的对象内存，并合并相邻空闲块。
    ///
    /// # Safety
    ///
    /// `pointer` 必须是本分配器返回且尚未释放的指针，不能传入偏移后的指针。
    unsafe fn deallocate(&mut self, pointer: NonNull<u8>) -> bool {
        let Some(header_address) =
            (pointer.as_ptr() as usize).checked_sub(core::mem::size_of::<AllocationHeader>())
        else {
            return false;
        };
        if header_address < self.start
            || header_address
                .checked_add(core::mem::size_of::<AllocationHeader>())
                .is_none_or(|end| end > self.end)
        {
            return false;
        }

        // 安全性：GlobalAlloc 契约保证指针属于本分配器且未释放，头部地址也已验证在堆区内。
        let allocation = unsafe { core::ptr::read(header_address as *const AllocationHeader) };
        let Some(block_end) = allocation.block_start.checked_add(allocation.block_size) else {
            return false;
        };
        if allocation.magic != ALLOCATION_MAGIC
            || allocation.block_start < self.start
            || allocation.block_start & (core::mem::align_of::<FreeBlock>() - 1) != 0
            || allocation.block_size < core::mem::size_of::<FreeBlock>()
            || block_end > self.end
        {
            return false;
        }

        self.insert_free_block(allocation.block_start, allocation.block_size)
    }

    fn insert_free_block(&mut self, start: usize, size: usize) -> bool {
        let Some(end) = start.checked_add(size) else {
            return false;
        };
        let alignment = core::mem::align_of::<FreeBlock>();
        if start < self.start
            || end > self.end
            || start & (alignment - 1) != 0
            || size < core::mem::size_of::<FreeBlock>()
            || size & (alignment - 1) != 0
        {
            return false;
        }

        let mut previous = core::ptr::null_mut();
        let mut next = self.first_free_block;
        while !next.is_null() && (next as usize) < start {
            previous = next;
            // 安全性：遍历节点来自本分配器维护的有序空闲链表。
            next = unsafe { (*next).next };
        }

        if !previous.is_null() {
            // 安全性：previous 来自当前堆维护的有效空闲链表。
            let previous_end = (previous as usize).checked_add(unsafe { (*previous).size });
            if previous_end.is_none_or(|previous_end| previous_end > start) {
                return false;
            }
        }
        if !next.is_null() && end > next as usize {
            return false;
        }

        let block = start as *mut FreeBlock;
        // 安全性：范围、对齐与重叠均已校验，区域可以重新作为空闲块头部使用。
        unsafe {
            core::ptr::write(block, FreeBlock { size, next });
        }

        let merged_block = block;
        if !next.is_null() && end == next as usize {
            // 安全性：相邻空闲块均位于堆区内，合并后的区间仍在堆边界内。
            unsafe {
                (*merged_block).size += (*next).size;
                (*merged_block).next = (*next).next;
            }
        }

        if !previous.is_null() {
            // 安全性：前驱来自本堆维护的有效空闲链表。
            let previous_end = unsafe { previous as usize + (*previous).size };
            if previous_end == merged_block as usize {
                // 安全性：两个已验证的相邻空闲块可合并，且不会越过堆边界。
                unsafe {
                    (*previous).size += (*merged_block).size;
                    (*previous).next = (*merged_block).next;
                }
            } else {
                // 安全性：前驱与新空闲块不重叠，按地址顺序插入链表。
                unsafe { (*previous).next = merged_block };
            }
        } else {
            self.first_free_block = merged_block;
        }
        true
    }
}

// 安全性：每个进程只有一个用户态执行流，且该分配器状态位于进程私有地址空间；当前不会并发或重入分配。
unsafe impl GlobalAlloc for UserHeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // 安全性：分配器独占访问当前进程私有的堆状态，brk syscall 不会回调用户态分配器。
        unsafe { (*core::ptr::addr_of_mut!(USER_HEAP)).allocate(layout) }
            .map_or(core::ptr::null_mut(), NonNull::as_ptr)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
        let Some(pointer) = NonNull::new(pointer) else {
            panic!("用户全局分配器收到空释放指针");
        };
        // 安全性：GlobalAlloc 调用方保证指针来自本分配器且尚未释放。
        let released = unsafe { (*core::ptr::addr_of_mut!(USER_HEAP)).deallocate(pointer) };
        if !released {
            panic!("用户全局分配器释放了无效或重复释放的指针");
        }
    }
}

fn replace_free_block(
    previous: *mut FreeBlock,
    new_block: *mut FreeBlock,
    first_free_block: &mut *mut FreeBlock,
) {
    if previous.is_null() {
        *first_free_block = new_block;
    } else {
        // 安全性：前驱来自当前空闲链表，新节点仍位于同一已映射堆区。
        unsafe { (*previous).next = new_block };
    }
}

fn remove_free_block(
    previous: *mut FreeBlock,
    next: *mut FreeBlock,
    first_free_block: &mut *mut FreeBlock,
) {
    if previous.is_null() {
        *first_free_block = next;
    } else {
        // 安全性：前驱来自空闲链表，next 是待移除块的后继。
        unsafe { (*previous).next = next };
    }
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    value
        .checked_add(alignment - 1)
        .map(|aligned| aligned & !(alignment - 1))
}
