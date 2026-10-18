//! 管理进程内核栈和用户栈的页帧所有权。
//!
//! 内核栈要求物理页连续，并由每张进程页表映射到内核高半区；用户栈逐页映射，物理页可以分散。

use alloc::vec::Vec;

use crate::arch::riscv64::trap::TrapFrame;
use crate::kernel::frame_allocator::{self, FrameError, PhysicalFrame, PhysicalFrameRun};
use crate::kernel::memory::{address, page::PAGE_SIZE};

const USER_STACK_SIZE: usize = 16 * 1024;
const KERNEL_STACK_SIZE: usize = 16 * 1024;
const USER_STACK_TOP: usize = address::SV39_USER_ADDRESS_END_EXCLUSIVE - PAGE_SIZE;
const STACK_PAGE_COUNT: usize = USER_STACK_SIZE / PAGE_SIZE;

/// 描述进程栈页帧分配失败。
pub(super) enum StackError {
    Frame(FrameError),
    MetadataAllocation,
    ArgumentsTooLarge,
}

impl StackError {
    pub(super) fn description(&self) -> &'static str {
        match self {
            Self::Frame(error) => error.description(),
            Self::MetadataAllocation => "无法为用户栈页帧所有权分配内核堆内存",
            Self::ArgumentsTooLarge => "用户程序参数无法放入用户栈",
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
        let physical_top = self.physical_top();
        let Some(virtual_top) = address::physical_to_kernel_virtual(physical_top) else {
            panic!("计算内核栈高地址时溢出");
        };
        virtual_top
    }

    /// 返回固定放在内核栈顶部的陷入帧地址。
    pub(super) fn trap_frame_pointer(&self) -> *mut TrapFrame {
        let stack_top = self.top();
        let stack_start = stack_top - KERNEL_STACK_SIZE;
        let Some(frame_address) = stack_top.checked_sub(core::mem::size_of::<TrapFrame>()) else {
            panic!("内核栈无法容纳陷入帧");
        };
        if frame_address < stack_start {
            panic!("陷入帧超出内核栈范围");
        }
        frame_address as *mut TrapFrame
    }

    pub(super) fn physical_top(&self) -> usize {
        let Some(backing) = self.backing.as_ref() else {
            panic!("进程内核栈的页帧 backing 已被释放");
        };
        backing.start_address() + backing.size()
    }

    pub(super) fn range(&self) -> (usize, usize) {
        (self.top() - KERNEL_STACK_SIZE, self.top())
    }

    pub(super) fn physical_start(&self) -> usize {
        self.physical_top() - KERNEL_STACK_SIZE
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        if let Some(backing) = self.backing.take() {
            if let Err(error) = frame_allocator::deallocate_run(backing) {
                panic!("释放进程内核栈页帧失败：{}", error.description());
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

    /// 在新用户栈中构造 `argc`、`argv`、空 `envp` 和空辅助向量。
    pub(super) fn initialize_arguments(
        &mut self,
        arguments: &[alloc::vec::Vec<u8>],
    ) -> Result<usize, StackError> {
        let mut argument_addresses = Vec::new();
        argument_addresses
            .try_reserve_exact(arguments.len())
            .map_err(|_| StackError::MetadataAllocation)?;

        let stack_start = self.start();
        let mut stack_pointer = self.top();
        for argument in arguments.iter().rev() {
            let argument_size = argument
                .len()
                .checked_add(1)
                .ok_or(StackError::ArgumentsTooLarge)?;
            stack_pointer = stack_pointer
                .checked_sub(argument_size)
                .filter(|address| *address >= stack_start)
                .ok_or(StackError::ArgumentsTooLarge)?;
            self.write_bytes(stack_pointer, argument)?;
            self.write_bytes(stack_pointer + argument.len(), &[0])?;
            argument_addresses.push(stack_pointer);
        }
        argument_addresses.reverse();

        let metadata_word_count = arguments
            .len()
            .checked_add(5)
            .ok_or(StackError::ArgumentsTooLarge)?;
        let metadata_size = metadata_word_count
            .checked_mul(core::mem::size_of::<usize>())
            .ok_or(StackError::ArgumentsTooLarge)?;
        stack_pointer = stack_pointer
            .checked_sub(metadata_size)
            .filter(|address| *address >= stack_start)
            .ok_or(StackError::ArgumentsTooLarge)?;
        stack_pointer &= !15;
        if stack_pointer < stack_start {
            return Err(StackError::ArgumentsTooLarge);
        }

        let mut word_index = 0;
        self.write_word(stack_pointer, arguments.len())?;
        word_index += 1;
        for argument_address in argument_addresses {
            self.write_word(
                stack_pointer + word_index * core::mem::size_of::<usize>(),
                argument_address,
            )?;
            word_index += 1;
        }
        // argv、envp 和 AT_NULL 辅助向量项依次以零结尾。
        for _ in 0..4 {
            self.write_word(
                stack_pointer + word_index * core::mem::size_of::<usize>(),
                0,
            )?;
            word_index += 1;
        }

        Ok(stack_pointer)
    }

    fn write_word(&self, address: usize, value: usize) -> Result<(), StackError> {
        self.write_bytes(address, &value.to_le_bytes())
    }

    fn write_bytes(&self, mut address: usize, mut bytes: &[u8]) -> Result<(), StackError> {
        let stack_start = self.start();
        let stack_end = self.top();
        let Some(write_end) = address.checked_add(bytes.len()) else {
            return Err(StackError::ArgumentsTooLarge);
        };
        if address < stack_start || write_end > stack_end {
            return Err(StackError::ArgumentsTooLarge);
        }

        while !bytes.is_empty() {
            let stack_offset = address - stack_start;
            let page_index = stack_offset / PAGE_SIZE;
            let page_offset = stack_offset % PAGE_SIZE;
            let byte_count = core::cmp::min(PAGE_SIZE - page_offset, bytes.len());
            let frame = self
                .frames
                .get(page_index)
                .unwrap_or_else(|| panic!("用户栈写入超出页帧范围"));
            let Some(kernel_address) = address::physical_to_kernel_virtual(frame.start_address())
            else {
                panic!("用户栈页帧无法映射到内核高半区");
            };
            // 安全性：页帧由当前新建的用户栈独占；目标区间已按栈范围和页内偏移限制。
            unsafe {
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    (kernel_address + page_offset) as *mut u8,
                    byte_count,
                );
            }
            address += byte_count;
            bytes = &bytes[byte_count..];
        }

        Ok(())
    }

    /// 复制用户栈的页内容，保持父子进程相同的用户虚拟栈布局。
    pub(super) fn duplicate(&self) -> Result<Self, StackError> {
        let duplicate = Self::allocate()?;
        for (source, destination) in self.frames.iter().zip(duplicate.frames.iter()) {
            crate::kernel::memory::copy_frame(source, destination);
        }
        Ok(duplicate)
    }
}

impl Drop for UserStack {
    fn drop(&mut self) {
        while let Some(frame) = self.frames.pop() {
            if let Err(error) = frame_allocator::deallocate_frame(frame) {
                panic!("释放进程用户栈页帧失败：{}", error.description());
            }
        }
    }
}
