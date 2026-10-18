//! 读取本阶段支持的静态 RV64 ELF 可执行文件。

use alloc::vec::Vec;

const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const ELF_CLASS_64: u8 = 2;
const ELF_LITTLE_ENDIAN: u8 = 1;
const ELF_VERSION_CURRENT: u32 = 1;
const ELF_TYPE_EXECUTABLE: u16 = 2;
const ELF_MACHINE_RISCV: u16 = 243;
const PROGRAM_TYPE_LOAD: u32 = 1;
const PROGRAM_TYPE_DYNAMIC: u32 = 2;
const PROGRAM_TYPE_INTERPRETER: u32 = 3;
const PROGRAM_FLAG_EXECUTE: u32 = 1;
const PROGRAM_FLAG_WRITE: u32 = 2;
const PROGRAM_FLAG_READ: u32 = 4;

/// ELF 装载段描述；文件偏移和虚拟地址均以字节为单位。
#[derive(Clone, Copy)]
pub(super) struct LoadSegment {
    pub(super) file_offset: usize,
    pub(super) virtual_start: usize,
    pub(super) file_size: usize,
    pub(super) memory_size: usize,
    pub(super) readable: bool,
    pub(super) writable: bool,
    pub(super) executable: bool,
}

/// 经过格式与边界检查的 ELF 映像元数据。
pub(super) struct ElfImage {
    pub(super) entry: usize,
    pub(super) segments: Vec<LoadSegment>,
}

/// 解析 ELF64 little-endian RISC-V 静态可执行文件的程序头。
pub(super) fn parse(bytes: &[u8]) -> Result<ElfImage, ElfError> {
    if bytes.len() < ELF_HEADER_SIZE
        || bytes.get(0..4) != Some(b"\x7fELF")
        || bytes[4] != ELF_CLASS_64
        || bytes[5] != ELF_LITTLE_ENDIAN
        || bytes[6] != ELF_VERSION_CURRENT as u8
        || read_u16(bytes, 16)? != ELF_TYPE_EXECUTABLE
        || read_u16(bytes, 18)? != ELF_MACHINE_RISCV
        || read_u32(bytes, 20)? != ELF_VERSION_CURRENT
        || read_u16(bytes, 52)? as usize != ELF_HEADER_SIZE
        || read_u16(bytes, 54)? as usize != PROGRAM_HEADER_SIZE
    {
        return Err(ElfError::UnsupportedFormat);
    }

    let entry = usize::try_from(read_u64(bytes, 24)?).map_err(|_| ElfError::InvalidRange)?;
    let program_header_offset =
        usize::try_from(read_u64(bytes, 32)?).map_err(|_| ElfError::InvalidRange)?;
    let program_header_count = read_u16(bytes, 56)? as usize;
    let table_size = program_header_count
        .checked_mul(PROGRAM_HEADER_SIZE)
        .ok_or(ElfError::InvalidRange)?;
    let table_end = program_header_offset
        .checked_add(table_size)
        .ok_or(ElfError::InvalidRange)?;
    if program_header_count == 0 || table_end > bytes.len() {
        return Err(ElfError::InvalidRange);
    }

    let mut segments = Vec::new();
    segments
        .try_reserve_exact(program_header_count)
        .map_err(|_| ElfError::MetadataAllocation)?;
    let mut entry_is_executable = false;

    for index in 0..program_header_count {
        let header_offset = program_header_offset + index * PROGRAM_HEADER_SIZE;
        let segment_type = read_u32(bytes, header_offset)?;
        if segment_type == PROGRAM_TYPE_INTERPRETER || segment_type == PROGRAM_TYPE_DYNAMIC {
            return Err(ElfError::DynamicLinkingUnsupported);
        }
        if segment_type != PROGRAM_TYPE_LOAD {
            continue;
        }

        let flags = read_u32(bytes, header_offset + 4)?;
        let file_offset = usize::try_from(read_u64(bytes, header_offset + 8)?)
            .map_err(|_| ElfError::InvalidRange)?;
        let virtual_start = usize::try_from(read_u64(bytes, header_offset + 16)?)
            .map_err(|_| ElfError::InvalidRange)?;
        let file_size = usize::try_from(read_u64(bytes, header_offset + 32)?)
            .map_err(|_| ElfError::InvalidRange)?;
        let memory_size = usize::try_from(read_u64(bytes, header_offset + 40)?)
            .map_err(|_| ElfError::InvalidRange)?;
        let alignment = usize::try_from(read_u64(bytes, header_offset + 48)?)
            .map_err(|_| ElfError::InvalidRange)?;
        let file_end = file_offset
            .checked_add(file_size)
            .ok_or(ElfError::InvalidRange)?;
        let virtual_end = virtual_start
            .checked_add(memory_size)
            .ok_or(ElfError::InvalidRange)?;

        if flags & !(PROGRAM_FLAG_READ | PROGRAM_FLAG_WRITE | PROGRAM_FLAG_EXECUTE) != 0
            || flags & PROGRAM_FLAG_READ == 0
            || (flags & PROGRAM_FLAG_WRITE != 0 && flags & PROGRAM_FLAG_EXECUTE != 0)
            || file_size > memory_size
            || memory_size == 0
            || file_end > bytes.len()
            || virtual_start < crate::kernel::user_program::USER_PROGRAM_VIRTUAL_BASE
            || virtual_end > crate::kernel::memory::address::SV39_USER_ADDRESS_END_EXCLUSIVE
            || !valid_alignment(alignment, file_offset, virtual_start)
        {
            return Err(ElfError::InvalidSegment);
        }

        let executable = flags & PROGRAM_FLAG_EXECUTE != 0;
        if executable && entry >= virtual_start && entry < virtual_end {
            entry_is_executable = true;
        }
        segments.push(LoadSegment {
            file_offset,
            virtual_start,
            file_size,
            memory_size,
            readable: flags & PROGRAM_FLAG_READ != 0,
            writable: flags & PROGRAM_FLAG_WRITE != 0,
            executable,
        });
    }

    if segments.is_empty() || !entry_is_executable {
        return Err(ElfError::InvalidEntry);
    }

    Ok(ElfImage { entry, segments })
}

fn valid_alignment(alignment: usize, file_offset: usize, virtual_start: usize) -> bool {
    alignment <= 1
        || (alignment.is_power_of_two() && file_offset % alignment == virtual_start % alignment)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ElfError> {
    let end = offset.checked_add(2).ok_or(ElfError::InvalidRange)?;
    let value = bytes.get(offset..end).ok_or(ElfError::InvalidRange)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ElfError> {
    let end = offset.checked_add(4).ok_or(ElfError::InvalidRange)?;
    let value = bytes.get(offset..end).ok_or(ElfError::InvalidRange)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ElfError> {
    let end = offset.checked_add(8).ok_or(ElfError::InvalidRange)?;
    let value = bytes.get(offset..end).ok_or(ElfError::InvalidRange)?;
    Ok(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}

/// 本阶段 ELF 解析失败的原因。
pub(in crate::kernel) enum ElfError {
    UnsupportedFormat,
    DynamicLinkingUnsupported,
    InvalidRange,
    InvalidSegment,
    InvalidEntry,
    MetadataAllocation,
}

impl ElfError {
    pub(super) const fn is_invalid_image(&self) -> bool {
        !matches!(self, Self::MetadataAllocation)
    }

    pub(super) const fn description(&self) -> &'static str {
        match self {
            Self::UnsupportedFormat => "ELF 格式不是 64 位 little-endian RISC-V ET_EXEC",
            Self::DynamicLinkingUnsupported => "ELF 需要动态链接器，本阶段不支持",
            Self::InvalidRange => "ELF 头部或程序头范围越界",
            Self::InvalidSegment => "ELF 装载段布局或权限无效",
            Self::InvalidEntry => "ELF 没有可执行装载段或入口不在可执行段中",
            Self::MetadataAllocation => "无法为 ELF 程序头分配内核堆空间",
        }
    }
}
