//! 查找并装入构建清单中的用户程序，管理其镜像页帧的所有权。

use alloc::vec::Vec;

use super::frame_allocator::{self, FrameError, PhysicalFrame};
use super::memory::{address, page::PAGE_SIZE};

const USER_PROGRAM_VIRTUAL_BASE: usize = 0x4000_0000;

struct UserProgramImage {
    name: &'static str,
    bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/user_program_catalog.rs"));

/// 描述用户程序装载失败的阶段和原因。
pub(super) enum UserProgramLoadError {
    Frame(FrameError),
    MetadataAllocation,
    AddressRangeOverflow,
}

impl UserProgramLoadError {
    /// 返回用于进程创建诊断的简要说明。
    pub(super) fn description(&self) -> &'static str {
        match self {
            Self::Frame(error) => error.description(),
            Self::MetadataAllocation => "无法为用户程序镜像页帧分配内核堆空间",
            Self::AddressRangeOverflow => "用户程序镜像地址范围溢出",
        }
    }
}

/// 已装入物理页帧、可映射到进程地址空间的用户程序。
pub(super) struct LoadedUserProgram {
    /// 用户程序在构建清单中的名称。
    pub(super) name: &'static str,
    /// 用户程序链接时使用的固定虚拟起始地址，也是当前的执行入口。
    pub(super) virtual_start: usize,
    /// 镜像在用户虚拟地址空间中实际占用的字节数。
    pub(super) image_size: usize,
    frames: Vec<PhysicalFrame>,
}

impl LoadedUserProgram {
    pub(super) fn frames(&self) -> &[PhysicalFrame] {
        &self.frames
    }

    /// 复制程序的当前镜像页，为独立用户地址空间准备新的 backing。
    pub(super) fn duplicate(&self) -> Result<Self, UserProgramLoadError> {
        let frames = allocate_image_frames(self.frames.len())?;
        for (source, destination) in self.frames.iter().zip(frames.iter()) {
            super::memory::copy_frame(source, destination);
        }
        // 安全性：本 hart 刚复制的新页帧包含可执行指令，需要同步指令缓存。
        unsafe { core::arch::asm!("fence.i", options(nostack)) };

        Ok(Self {
            name: self.name,
            virtual_start: self.virtual_start,
            image_size: self.image_size,
            frames,
        })
    }
}

impl Drop for LoadedUserProgram {
    fn drop(&mut self) {
        release_frames(&mut self.frames);
    }
}

/// 按构建清单名称查找程序索引。
pub(super) fn find_index(name: &str) -> Option<usize> {
    USER_PROGRAMS
        .iter()
        .position(|program| program.name == name)
}

/// 返回构建清单中的用户程序数量。
pub(super) fn count() -> usize {
    USER_PROGRAMS.len()
}

/// 将指定程序复制到新分配的物理页帧；索引超出清单时表示没有该程序。
pub(super) fn load(index: usize) -> Result<Option<LoadedUserProgram>, UserProgramLoadError> {
    let Some(program) = USER_PROGRAMS.get(index) else {
        return Ok(None);
    };
    let image_end = USER_PROGRAM_VIRTUAL_BASE
        .checked_add(program.bytes.len())
        .ok_or(UserProgramLoadError::AddressRangeOverflow)?;
    if program.bytes.is_empty() || image_end > address::SV39_USER_ADDRESS_END_EXCLUSIVE {
        return Err(UserProgramLoadError::AddressRangeOverflow);
    }
    let frame_count = program
        .bytes
        .len()
        .checked_add(PAGE_SIZE - 1)
        .ok_or(UserProgramLoadError::AddressRangeOverflow)?
        / PAGE_SIZE;
    if frame_count == 0 {
        return Err(UserProgramLoadError::AddressRangeOverflow);
    }
    let frames = allocate_image_frames(frame_count)?;

    copy_image_to_frames(program.bytes, &frames);

    Ok(Some(LoadedUserProgram {
        name: program.name,
        virtual_start: USER_PROGRAM_VIRTUAL_BASE,
        image_size: program.bytes.len(),
        frames,
    }))
}

fn allocate_image_frames(frame_count: usize) -> Result<Vec<PhysicalFrame>, UserProgramLoadError> {
    let mut frames = Vec::new();
    frames
        .try_reserve_exact(frame_count)
        .map_err(|_| UserProgramLoadError::MetadataAllocation)?;

    for _ in 0..frame_count {
        match frame_allocator::allocate_zeroed_frame() {
            Ok(frame) => frames.push(frame),
            Err(error) => {
                release_frames(&mut frames);
                return Err(UserProgramLoadError::Frame(error));
            }
        }
    }
    Ok(frames)
}

fn copy_image_to_frames(bytes: &[u8], frames: &[PhysicalFrame]) {
    let mut copied_bytes = 0;
    for frame in frames {
        let Some(virtual_address) = address::physical_to_kernel_virtual(frame.start_address())
        else {
            panic!("用户程序镜像页帧无法映射到内核地址");
        };
        let bytes_in_frame = core::cmp::min(PAGE_SIZE, bytes.len() - copied_bytes);
        if bytes_in_frame > 0 {
            // 安全性：目标页由当前装载器独占，且已映射到内核高半区；源切片长度已限制在单页内。
            unsafe {
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr().add(copied_bytes),
                    virtual_address as *mut u8,
                    bytes_in_frame,
                );
            }
            copied_bytes += bytes_in_frame;
        }
    }
    // 安全性：只同步本 hart 刚复制到新分配页帧中的可执行指令。
    unsafe { core::arch::asm!("fence.i", options(nostack)) };
}

fn release_frames(frames: &mut Vec<PhysicalFrame>) {
    while let Some(frame) = frames.pop() {
        if let Err(error) = frame_allocator::deallocate_frame(frame) {
            panic!("回收用户程序镜像页帧失败：{}", error.description());
        }
    }
}
