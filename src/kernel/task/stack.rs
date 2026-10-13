//! 管理任务内核栈和用户栈的页帧所有权。
//!
//! 内核栈要求物理页连续，以恒等映射形成连续栈地址；用户栈通过页表逐页映射，物理页可以分散。

use alloc::vec::Vec;

use crate::kernel::frame_allocator::{self, FrameError, PhysicalFrame};

const PAGE_SIZE: usize = 4096;
const USER_STACK_SIZE: usize = 16 * 1024;
const KERNEL_STACK_SIZE: usize = 16 * 1024;
const USER_STACK_TOP: usize = (1usize << 38) - PAGE_SIZE;
const STACK_PAGE_COUNT: usize = USER_STACK_SIZE / PAGE_SIZE;

struct StackBacking {
    frames: Vec<PhysicalFrame>,
}

impl StackBacking {
    fn allocate_contiguous(page_count: usize) -> Result<Self, FrameError> {
        let frames = frame_allocator::allocate_contiguous_zeroed_frames(page_count)?;
        Ok(Self { frames })
    }

    fn allocate_scattered(page_count: usize) -> Result<Self, FrameError> {
        let frames = frame_allocator::allocate_zeroed_frames(page_count)?;
        Ok(Self { frames })
    }

    fn first_address(&self) -> usize {
        self.frames[0].start_address()
    }

    fn size(&self) -> usize {
        self.frames.len() * PAGE_SIZE
    }
}

impl Drop for StackBacking {
    fn drop(&mut self) {
        while let Some(frame) = self.frames.pop() {
            if let Err(error) = frame_allocator::deallocate_frame(frame) {
                crate::klog_error!("释放任务栈页帧失败：{}", error.description());
                super::stop_forever();
            }
        }
    }
}

pub(super) struct KernelStack {
    backing: StackBacking,
}

impl KernelStack {
    pub(super) fn allocate() -> Result<Self, FrameError> {
        Ok(Self {
            backing: StackBacking::allocate_contiguous(KERNEL_STACK_SIZE / PAGE_SIZE)?,
        })
    }

    pub(super) fn top(&self) -> usize {
        self.backing.first_address() + self.backing.size()
    }

    pub(super) fn range(&self) -> (usize, usize) {
        (self.top() - KERNEL_STACK_SIZE, self.top())
    }
}

pub(super) struct UserStack {
    backing: StackBacking,
}

impl UserStack {
    pub(super) fn allocate() -> Result<Self, FrameError> {
        Ok(Self {
            backing: StackBacking::allocate_scattered(STACK_PAGE_COUNT)?,
        })
    }

    pub(super) fn start(&self) -> usize {
        USER_STACK_TOP - self.backing.size()
    }

    pub(super) fn top(&self) -> usize {
        USER_STACK_TOP
    }

    pub(super) fn range(&self) -> (usize, usize) {
        (self.start(), self.top())
    }

    pub(super) fn frames(&self) -> &[PhysicalFrame] {
        &self.backing.frames
    }
}
