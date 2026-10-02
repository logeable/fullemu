//! 扁平设备树（FDT）固定头部的读取和边界校验。
//!
//! 本模块只解析头部，不遍历设备树结构块，也不访问硬件设备属性。

const FDT_MAGIC: u32 = 0xd00d_feed;
const HEADER_SIZE: usize = 40;
const HEADER_VERSION: u32 = 17;

/// 已通过基础边界校验的 FDT 头部字段。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FdtHeader {
    /// 整个 DTB 的字节数。
    pub total_size: u32,
    /// 设备树结构块相对 DTB 起始位置的偏移。
    pub structure_offset: u32,
    /// 属性名称字符串块相对 DTB 起始位置的偏移。
    pub strings_offset: u32,
    /// 内存保留表相对 DTB 起始位置的偏移。
    pub reserve_map_offset: u32,
    /// FDT 二进制格式版本。
    pub version: u32,
    /// 最低兼容格式版本。
    pub last_compatible_version: u32,
    /// 字符串块的字节数。
    pub strings_size: u32,
    /// 结构块的字节数。
    pub structure_size: u32,
}

/// FDT 头部不符合本阶段支持的格式或范围约束。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FdtHeaderError {
    /// 输入字节数不足以包含本阶段读取的 40 字节头部。
    Truncated,
    /// 头部 magic 与 FDT 标准值不符。
    InvalidMagic,
    /// FDT 声明的总长度小于头部长度。
    InvalidTotalSize,
    /// FDT 版本早于 17，或不兼容本解析器理解的头部布局。
    UnsupportedVersion,
    /// 内存保留表偏移落在头部之前或 DTB 范围之外。
    InvalidReserveMapOffset,
    /// 结构块偏移或长度越出 DTB 范围。
    InvalidStructureRange,
    /// 字符串块偏移或长度越出 DTB 范围。
    InvalidStringsRange,
    /// 传入的 DTB 指针为空。
    NullPointer,
}

impl FdtHeaderError {
    /// 返回适合内核启动诊断的错误说明。
    pub const fn description(self) -> &'static str {
        match self {
            Self::Truncated => "头部不足 40 字节",
            Self::InvalidMagic => "magic 不匹配",
            Self::InvalidTotalSize => "总长度小于头部长度",
            Self::UnsupportedVersion => "格式版本不兼容",
            Self::InvalidReserveMapOffset => "保留表偏移越界",
            Self::InvalidStructureRange => "结构块范围越界",
            Self::InvalidStringsRange => "字符串块范围越界",
            Self::NullPointer => "DTB 指针为空",
        }
    }
}

impl FdtHeader {
    /// 从 FDT 的前 40 字节解析头部，并检查主要区块的边界。
    ///
    /// `bytes` 只需覆盖固定头部；区块边界根据头部中的 `total_size` 检查。
    pub fn parse(bytes: &[u8]) -> Result<Self, FdtHeaderError> {
        if bytes.len() < HEADER_SIZE {
            return Err(FdtHeaderError::Truncated);
        }

        if read_u32_be(bytes, 0) != FDT_MAGIC {
            return Err(FdtHeaderError::InvalidMagic);
        }

        let header = Self {
            total_size: read_u32_be(bytes, 4),
            structure_offset: read_u32_be(bytes, 8),
            strings_offset: read_u32_be(bytes, 12),
            reserve_map_offset: read_u32_be(bytes, 16),
            version: read_u32_be(bytes, 20),
            last_compatible_version: read_u32_be(bytes, 24),
            strings_size: read_u32_be(bytes, 32),
            structure_size: read_u32_be(bytes, 36),
        };

        if header.total_size < HEADER_SIZE as u32 {
            return Err(FdtHeaderError::InvalidTotalSize);
        }

        if header.version < HEADER_VERSION
            || header.last_compatible_version > HEADER_VERSION
            || header.last_compatible_version > header.version
        {
            return Err(FdtHeaderError::UnsupportedVersion);
        }

        if header.reserve_map_offset < HEADER_SIZE as u32
            || header.reserve_map_offset >= header.total_size
        {
            return Err(FdtHeaderError::InvalidReserveMapOffset);
        }

        if !range_within_total(
            header.structure_offset,
            header.structure_size,
            header.total_size,
        ) {
            return Err(FdtHeaderError::InvalidStructureRange);
        }

        if !range_within_total(
            header.strings_offset,
            header.strings_size,
            header.total_size,
        ) {
            return Err(FdtHeaderError::InvalidStringsRange);
        }

        Ok(header)
    }

    /// 从固件传入的 DTB 地址读取固定头部。
    ///
    /// # 安全要求
    /// 空指针会返回错误；对于非空指针，调用方必须保证至少有 40 个可读字节。
    /// 在本项目的启动路径中，该地址来自 OpenSBI 传入的 FDT 指针。
    pub unsafe fn read_from_ptr(pointer: *const u8) -> Result<Self, FdtHeaderError> {
        if pointer.is_null() {
            return Err(FdtHeaderError::NullPointer);
        }

        // 安全性：调用方保证指针指向至少 40 个可读字节；u8 对齐要求为 1。
        let bytes = unsafe { core::slice::from_raw_parts(pointer, HEADER_SIZE) };
        Self::parse(bytes)
    }
}

fn read_u32_be(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn range_within_total(offset: u32, size: u32, total_size: u32) -> bool {
    offset >= HEADER_SIZE as u32
        && offset
            .checked_add(size)
            .is_some_and(|end| end <= total_size)
}

#[cfg(test)]
mod tests {
    use super::{FdtHeader, FdtHeaderError};

    const HEADER_SIZE: usize = 40;

    fn valid_header_bytes() -> [u8; HEADER_SIZE] {
        let mut bytes = [0; HEADER_SIZE];
        write_u32_be(&mut bytes, 0, 0xd00d_feed);
        write_u32_be(&mut bytes, 4, 0x1000);
        write_u32_be(&mut bytes, 8, 0x40);
        write_u32_be(&mut bytes, 12, 0x200);
        write_u32_be(&mut bytes, 16, 0x28);
        write_u32_be(&mut bytes, 20, 17);
        write_u32_be(&mut bytes, 24, 16);
        write_u32_be(&mut bytes, 32, 0x80);
        write_u32_be(&mut bytes, 36, 0x100);
        bytes
    }

    fn write_u32_be(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    #[test]
    fn accepts_a_valid_version_17_header() {
        let header = FdtHeader::parse(&valid_header_bytes()).unwrap();

        assert_eq!(header.version, 17);
        assert_eq!(header.total_size, 0x1000);
        assert_eq!(header.structure_offset, 0x40);
        assert_eq!(header.structure_size, 0x100);
        assert_eq!(header.strings_offset, 0x200);
        assert_eq!(header.strings_size, 0x80);
    }

    #[test]
    fn rejects_a_truncated_header() {
        assert_eq!(
            FdtHeader::parse(&valid_header_bytes()[..HEADER_SIZE - 1]),
            Err(FdtHeaderError::Truncated)
        );
    }

    #[test]
    fn rejects_an_invalid_magic_value() {
        let mut bytes = valid_header_bytes();
        write_u32_be(&mut bytes, 0, 0);

        assert_eq!(FdtHeader::parse(&bytes), Err(FdtHeaderError::InvalidMagic));
    }

    #[test]
    fn rejects_a_total_size_smaller_than_the_header() {
        let mut bytes = valid_header_bytes();
        write_u32_be(&mut bytes, 4, 32);

        assert_eq!(
            FdtHeader::parse(&bytes),
            Err(FdtHeaderError::InvalidTotalSize)
        );
    }

    #[test]
    fn rejects_an_unsupported_header_version() {
        let mut bytes = valid_header_bytes();
        write_u32_be(&mut bytes, 20, 16);

        assert_eq!(
            FdtHeader::parse(&bytes),
            Err(FdtHeaderError::UnsupportedVersion)
        );
    }

    #[test]
    fn rejects_a_future_version_not_compatible_with_version_17() {
        let mut bytes = valid_header_bytes();
        write_u32_be(&mut bytes, 20, 18);
        write_u32_be(&mut bytes, 24, 18);

        assert_eq!(
            FdtHeader::parse(&bytes),
            Err(FdtHeaderError::UnsupportedVersion)
        );
    }

    #[test]
    fn rejects_a_null_pointer() {
        // 安全性：解析函数会在解引用前检查空指针。
        let result = unsafe { FdtHeader::read_from_ptr(core::ptr::null()) };

        assert_eq!(result, Err(FdtHeaderError::NullPointer));
    }

    #[test]
    fn rejects_a_reserve_map_outside_the_blob() {
        let mut bytes = valid_header_bytes();
        write_u32_be(&mut bytes, 16, 0x1000);

        assert_eq!(
            FdtHeader::parse(&bytes),
            Err(FdtHeaderError::InvalidReserveMapOffset)
        );
    }

    #[test]
    fn rejects_a_structure_block_outside_the_blob() {
        let mut bytes = valid_header_bytes();
        write_u32_be(&mut bytes, 8, 0xffff_fffc);
        write_u32_be(&mut bytes, 36, 8);

        assert_eq!(
            FdtHeader::parse(&bytes),
            Err(FdtHeaderError::InvalidStructureRange)
        );
    }

    #[test]
    fn rejects_a_strings_block_outside_the_blob() {
        let mut bytes = valid_header_bytes();
        write_u32_be(&mut bytes, 12, 0xff0);
        write_u32_be(&mut bytes, 32, 0x20);

        assert_eq!(
            FdtHeader::parse(&bytes),
            Err(FdtHeaderError::InvalidStringsRange)
        );
    }
}
