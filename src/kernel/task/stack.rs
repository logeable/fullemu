//! 管理任务内核栈和用户栈的页帧所有权。
//!
//! 内核栈要求物理页连续，以恒等映射形成连续栈地址；用户栈逐页映射，物理页可以分散。

use alloc::vec::Vec;

use crate::kernel::frame_allocator::{self, FrameError, PhysicalFrame, PhysicalFrameRun};

const PAGE_SIZE: usize = 4096;
const USER_STACK_SIZE: usize = 16 * 1024;
const KERNEL_STACK_SIZE: usize = 16 * 1024;
const USER_STACK_TOP: usize = (1usize << 38) - PAGE_SIZE;
const STACK_PAGE_COUNT: usize = USER_STACK_SIZE / PAGE_SIZE;

/// 描述任务栈页帧分配失败。
pub(super) enum StackError {
    Frame(FrameError),
    MetadataAllocation,
}

impl StackError {
    pub(super) fn description(&self) -> &'static str {
        match self {
            Self::Frame(error) => error.description(),
            Self::MetadataAllocation => "无法为用户栈页帧所有权分配内核堆内存",
        }
    }
}

impl From<FrameError> for StackError {
    fn from(error: FrameError) -> Self {
        Self::Frame(error)
    }
}

pub(super) struct KernelStack {
    backing: Option<PhysicalFrameRun>,
}

impl KernelStack {
    pub(super) fn allocate() -> Result<Self, StackError> {
        let backing = frame_allocator::allocate_zeroed_run(KERNEL_STACK_SIZE / PAGE_SIZE)?;
        Ok(Self {
            backing: Some(backing),
        })
    }

    pub(super) fn top(&self) -> usize {
        let Some(backing) = self.backing.as_ref() else {
            super::stop_forever();
        };
        backing.start_address() + backing.size()
    }

    pub(super) fn range(&self) -> (usize, usize) {
        (self.top() - KERNEL_STACK_SIZE, self.top())
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        if let Some(backing) = self.backing.take() {
            if let Err(error) = frame_allocator::deallocate_run(backing) {
                crate::klog_error!("释放任务内核栈页帧失败：{}", error.description());
                super::stop_forever();
            }
        }
    }
}

pub(super) struct UserStack {
    frames: Vec<PhysicalFrame>,
}

impl UserStack {
    pub(super) fn allocate() -> Result<Self, StackError> {
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(STACK_PAGE_COUNT)
            .map_err(|_| StackError::MetadataAllocation)?;

        let mut stack = Self { frames };
        for _ in 0..STACK_PAGE_COUNT {
            match frame_allocator::allocate_zeroed_frame() {
                Ok(frame) => stack.frames.push(frame),
                Err(error) => return Err(StackError::Frame(error)),
            }
        }
        Ok(stack)
    }

    pub(super) fn start(&self) -> usize {
        USER_STACK_TOP - self.frames.len() * PAGE_SIZE
    }

    pub(super) fn top(&self) -> usize {
        USER_STACK_TOP
    }

    pub(super) fn range(&self) -> (usize, usize) {
        (self.start(), self.top())
    }

    pub(super) fn frames(&self) -> &[PhysicalFrame] {
        &self.frames
    }
}

impl Drop for UserStack {
    fn drop(&mut self) {
        while let Some(frame) = self.frames.pop() {
            if let Err(error) = frame_allocator::deallocate_frame(frame) {
                crate::klog_error!("释放任务用户栈页帧失败：{}", error.description());
                super::stop_forever();
            }
        }
    }
}
