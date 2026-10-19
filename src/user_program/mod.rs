//! 查找并装入构建清单中的用户程序，管理其镜像页帧的所有权。

use alloc::vec::Vec;

use super::frame_allocator::{self, FrameError, PhysicalFrame};
use super::memory::{
    address,
    page::{align_down_to_page, align_up_to_page, PAGE_SIZE},
    UserImagePermissions,
};

mod elf;

const USER_PROGRAM_VIRTUAL_BASE: usize = 0x4000_0000;

struct UserProgramImage {
    name: &'static str,
    elf_bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/user_program_catalog.rs"));

/// 描述用户程序装载失败的阶段和原因。
pub(super) enum UserProgramLoadError {
    Frame(FrameError),
    MetadataAllocation,
    AddressRangeOverflow,
    InvalidElf(elf::ElfError),
    UnsupportedLayout,
}

impl UserProgramLoadError {
    /// 返回用于进程创建诊断的简要说明。
    pub(super) fn description(&self) -> &'static str {
        match self {
            Self::Frame(error) => error.description(),
            Self::MetadataAllocation => "无法为用户程序镜像页帧分配内核堆空间",
            Self::AddressRangeOverflow => "用户程序镜像地址范围溢出",
            Self::InvalidElf(error) => error.description(),
            Self::UnsupportedLayout => "ELF 装载段必须连续且不能共享同一虚拟页",
        }
    }

    pub(super) const fn is_invalid_image(&self) -> bool {
        match self {
            Self::InvalidElf(error) => error.is_invalid_image(),
            Self::AddressRangeOverflow | Self::UnsupportedLayout => true,
            Self::Frame(_) | Self::MetadataAllocation => false,
        }
    }
}

/// 已装入物理页帧、可映射到进程地址空间的用户程序。
pub(super) struct LoadedUserProgram {
    /// 用户程序在构建清单中的名称。
    pub(super) name: &'static str,
    /// ELF 头指定的用户程序入口地址。
    pub(super) entry: usize,
    /// 所有装载段覆盖范围的页对齐起点。
    pub(super) virtual_start: usize,
    /// 所有装载段覆盖范围的页对齐长度。
    pub(super) image_size: usize,
    /// 程序映像中连续可写页的字节范围。
    writable_range: Option<(usize, usize)>,
    frames: Vec<PhysicalFrame>,
    page_permissions: Vec<UserImagePermissions>,
}

impl LoadedUserProgram {
    pub(super) fn frames(&self) -> &[PhysicalFrame] {
        &self.frames
    }

    pub(super) fn page_permissions(&self) -> &[UserImagePermissions] {
        &self.page_permissions
    }

    pub(super) const fn writable_range(&self) -> Option<(usize, usize)> {
        self.writable_range
    }

    /// 复制程序的当前镜像页，为独立用户地址空间准备新的 backing。
    pub(super) fn duplicate(&self) -> Result<Self, UserProgramLoadError> {
        let mut page_permissions = Vec::new();
        page_permissions
            .try_reserve_exact(self.page_permissions.len())
            .map_err(|_| UserProgramLoadError::MetadataAllocation)?;
        page_permissions.extend_from_slice(&self.page_permissions);
        let frames = allocate_image_frames(self.frames.len())?;
        for (source, destination) in self.frames.iter().zip(frames.iter()) {
            super::memory::copy_frame(source, destination);
        }
        // 安全性：本 hart 刚复制的新页帧包含可执行指令，需要同步指令缓存。
        unsafe { core::arch::asm!("fence.i", options(nostack)) };

        Ok(Self {
            name: self.name,
            entry: self.entry,
            virtual_start: self.virtual_start,
            image_size: self.image_size,
            writable_range: self.writable_range,
            frames,
            page_permissions,
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

/// 将指定程序复制到新分配的物理页帧；索引超出清单时表示没有该程序。
pub(super) fn load(index: usize) -> Result<Option<LoadedUserProgram>, UserProgramLoadError> {
    let Some(program) = USER_PROGRAMS.get(index) else {
        return Ok(None);
    };
    let elf_image = elf::parse(program.elf_bytes).map_err(UserProgramLoadError::InvalidElf)?;
    let virtual_start = elf_image
        .segments
        .iter()
        .map(|segment| align_down_to_page(segment.virtual_start))
        .min()
        .ok_or(UserProgramLoadError::InvalidElf(
            elf::ElfError::InvalidEntry,
        ))?;
    let mut virtual_end = virtual_start;
    for segment in &elf_image.segments {
        let segment_end = segment
            .virtual_start
            .checked_add(segment.memory_size)
            .and_then(align_up_to_page)
            .ok_or(UserProgramLoadError::AddressRangeOverflow)?;
        virtual_end = virtual_end.max(segment_end);
    }
    let image_size = virtual_end
        .checked_sub(virtual_start)
        .ok_or(UserProgramLoadError::AddressRangeOverflow)?;
    let frame_count = image_size / PAGE_SIZE;
    if frame_count == 0 {
        return Err(UserProgramLoadError::AddressRangeOverflow);
    }

    let mut page_permissions = Vec::new();
    page_permissions
        .try_reserve_exact(frame_count)
        .map_err(|_| UserProgramLoadError::MetadataAllocation)?;
    page_permissions.resize(frame_count, None);
    for segment in &elf_image.segments {
        let segment_start = align_down_to_page(segment.virtual_start);
        let segment_end = segment
            .virtual_start
            .checked_add(segment.memory_size)
            .and_then(align_up_to_page)
            .ok_or(UserProgramLoadError::AddressRangeOverflow)?;
        let first_page = (segment_start - virtual_start) / PAGE_SIZE;
        let end_page = (segment_end - virtual_start) / PAGE_SIZE;
        let permissions = UserImagePermissions {
            readable: segment.readable,
            writable: segment.writable,
            executable: segment.executable,
        };
        for page in first_page..end_page {
            let Some(slot) = page_permissions.get_mut(page) else {
                return Err(UserProgramLoadError::UnsupportedLayout);
            };
            if slot.replace(permissions).is_some() {
                return Err(UserProgramLoadError::UnsupportedLayout);
            }
        }
    }
    let mut finalized_permissions = Vec::new();
    finalized_permissions
        .try_reserve_exact(frame_count)
        .map_err(|_| UserProgramLoadError::MetadataAllocation)?;
    for permissions in page_permissions {
        finalized_permissions.push(permissions.ok_or(UserProgramLoadError::UnsupportedLayout)?);
    }
    let writable_range = writable_image_range(virtual_start, &finalized_permissions)?;

    let frames = allocate_image_frames(frame_count)?;
    for segment in &elf_image.segments {
        let file_end = segment
            .file_offset
            .checked_add(segment.file_size)
            .ok_or(UserProgramLoadError::AddressRangeOverflow)?;
        let destination_offset = segment
            .virtual_start
            .checked_sub(virtual_start)
            .ok_or(UserProgramLoadError::AddressRangeOverflow)?;
        copy_segment_to_frames(
            &program.elf_bytes[segment.file_offset..file_end],
            destination_offset,
            &frames,
        );
    }
    // 安全性：新分配的镜像页已写入 ELF 可执行段，需要同步本 hart 的指令缓存。
    unsafe { core::arch::asm!("fence.i", options(nostack)) };

    Ok(Some(LoadedUserProgram {
        name: program.name,
        entry: elf_image.entry,
        virtual_start,
        image_size,
        writable_range,
        frames,
        page_permissions: finalized_permissions,
    }))
}

fn writable_image_range(
    virtual_start: usize,
    page_permissions: &[UserImagePermissions],
) -> Result<Option<(usize, usize)>, UserProgramLoadError> {
    let mut writable_start = None;
    let mut writable_end = None;
    let mut writable_region_ended = false;

    for (page_index, permissions) in page_permissions.iter().enumerate() {
        let page_start = virtual_start + page_index * PAGE_SIZE;
        if permissions.writable {
            if writable_region_ended {
                return Err(UserProgramLoadError::UnsupportedLayout);
            }
            writable_start.get_or_insert(page_start);
            writable_end = Some(page_start + PAGE_SIZE);
        } else if writable_start.is_some() {
            writable_region_ended = true;
        }
    }

    Ok(writable_start.zip(writable_end))
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

fn copy_segment_to_frames(bytes: &[u8], destination_offset: usize, frames: &[PhysicalFrame]) {
    let mut copied_bytes = 0;
    while copied_bytes < bytes.len() {
        let image_offset = destination_offset + copied_bytes;
        let frame_index = image_offset / PAGE_SIZE;
        let frame_offset = image_offset % PAGE_SIZE;
        let frame = frames
            .get(frame_index)
            .unwrap_or_else(|| panic!("ELF 段写入超出已分配镜像页"));
        let Some(virtual_address) = address::physical_to_kernel_virtual(frame.start_address())
        else {
            panic!("用户程序镜像页帧无法映射到内核地址");
        };
        let bytes_in_frame = core::cmp::min(PAGE_SIZE - frame_offset, bytes.len() - copied_bytes);
        // 安全性：目标页由当前装载器独占且已映射到内核高半区；源和目标范围均限制在单页内。
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr().add(copied_bytes),
                (virtual_address + frame_offset) as *mut u8,
                bytes_in_frame,
            );
        }
        copied_bytes += bytes_in_frame;
    }
}

fn release_frames(frames: &mut Vec<PhysicalFrame>) {
    while let Some(frame) = frames.pop() {
        if let Err(error) = frame_allocator::deallocate_frame(frame) {
            panic!("回收用户程序镜像页帧失败：{}", error.description());
        }
    }
}
