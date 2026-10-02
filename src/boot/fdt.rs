//! 扁平设备树（FDT）头部校验和结构块的只读遍历。

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

const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

/// FDT 结构块遍历时发现的格式错误。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FdtStructureError {
    /// 输入字节数短于头部声明的 DTB 总长度。
    BlobTooShort,
    /// 结构块范围超出头部或 DTB 边界。
    InvalidStructureRange,
    /// 字符串块范围超出头部或 DTB 边界。
    InvalidStringsRange,
    /// 结构块末尾存在不完整的 token。
    TruncatedToken,
    /// 节点名称缺少结尾零字节。
    UnterminatedNodeName,
    /// 节点名称不是有效的 ASCII 文本。
    InvalidNodeName,
    /// 节点名称后的对齐范围越出结构块。
    NodeNameOutOfBounds,
    /// 节点名称后的规范要求填充字节不为零。
    InvalidNodePadding,
    /// 根节点之后又出现了另一个根节点。
    MultipleRootNodes,
    /// 当前没有打开的节点，却遇到了节点结束 token。
    UnexpectedEndNode,
    /// 根节点之外出现属性。
    PropertyOutsideNode,
    /// 属性 token 后的长度和名称偏移字段不完整。
    TruncatedPropertyHeader,
    /// 属性名称偏移超出字符串块范围。
    InvalidPropertyNameOffset,
    /// 属性名称缺少结尾零字节。
    UnterminatedPropertyName,
    /// 属性名称为空或不是有效的 ASCII 文本。
    InvalidPropertyName,
    /// 属性值或对齐填充越出结构块范围。
    PropertyValueOutOfBounds,
    /// FDT_END 出现时根节点尚未完整关闭。
    UnexpectedEnd,
    /// 结构块结束前没有出现 FDT_END。
    MissingEnd,
    /// FDT_END 之后仍有多余数据。
    TrailingData,
    /// 遇到本实现不认识的结构 token。
    UnknownToken,
}

impl FdtStructureError {
    /// 返回适合内核启动诊断的错误说明。
    pub const fn description(self) -> &'static str {
        match self {
            Self::BlobTooShort => "DTB 总长度超出可读数据",
            Self::InvalidStructureRange => "结构块范围无效",
            Self::InvalidStringsRange => "字符串块范围无效",
            Self::TruncatedToken => "结构块末尾存在不完整 token",
            Self::UnterminatedNodeName => "节点名称未结束",
            Self::InvalidNodeName => "节点名称不是有效 ASCII 文本",
            Self::NodeNameOutOfBounds => "节点名称对齐范围越界",
            Self::InvalidNodePadding => "节点名称填充字节不是零",
            Self::MultipleRootNodes => "出现多个根节点",
            Self::UnexpectedEndNode => "节点结束 token 没有对应的开始节点",
            Self::PropertyOutsideNode => "根节点之外出现属性",
            Self::TruncatedPropertyHeader => "属性头部不完整",
            Self::InvalidPropertyNameOffset => "属性名称偏移越界",
            Self::UnterminatedPropertyName => "属性名称未结束",
            Self::InvalidPropertyName => "属性名称为空或不是有效 ASCII 文本",
            Self::PropertyValueOutOfBounds => "属性值或填充越界",
            Self::UnexpectedEnd => "根节点未闭合时遇到 FDT_END",
            Self::MissingEnd => "结构块缺少 FDT_END",
            Self::TrailingData => "FDT_END 后存在多余数据",
            Self::UnknownToken => "遇到未知结构 token",
        }
    }
}

/// 结构块遍历器返回的事件；属性值只被跳过，不会暴露给调用方。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FdtStructureEvent<'a> {
    /// 开始一个节点，根节点深度为 0。
    BeginNode { name: &'a str, depth: usize },
    /// 结束一个节点，根节点深度为 0。
    EndNode { depth: usize },
    /// 遇到一个属性，只提供名称、值长度和所属节点深度。
    Property {
        name: &'a str,
        value_size: usize,
        depth: usize,
    },
    /// 根节点已关闭，结构块正常结束。
    End,
}

/// 逐 token 检查 FDT 结构块并产生只读事件。
pub struct FdtStructureWalker<'a> {
    structure: &'a [u8],
    strings: &'a [u8],
    cursor: usize,
    depth: usize,
    root_seen: bool,
    ended: bool,
}

impl<'a> FdtStructureWalker<'a> {
    /// 从完整 DTB 字节切片和已验证头部建立结构块遍历器。
    pub fn new(blob: &'a [u8], header: &FdtHeader) -> Result<Self, FdtStructureError> {
        let total_size = header.total_size as usize;
        if blob.len() < total_size {
            return Err(FdtStructureError::BlobTooShort);
        }
        if header.structure_offset % 4 != 0 {
            return Err(FdtStructureError::InvalidStructureRange);
        }

        let structure = checked_section(
            blob,
            header.structure_offset,
            header.structure_size,
            total_size,
        )
        .ok_or(FdtStructureError::InvalidStructureRange)?;
        let strings = checked_section(blob, header.strings_offset, header.strings_size, total_size)
            .ok_or(FdtStructureError::InvalidStringsRange)?;

        Ok(Self {
            structure,
            strings,
            cursor: 0,
            depth: 0,
            root_seen: false,
            ended: false,
        })
    }

    /// 读取下一个事件；返回 `None` 表示已读到结构块末尾的 FDT_END。
    pub fn next_event(&mut self) -> Result<Option<FdtStructureEvent<'a>>, FdtStructureError> {
        if self.ended {
            return Ok(None);
        }

        loop {
            if self.cursor == self.structure.len() {
                return Err(FdtStructureError::MissingEnd);
            }

            let token = read_u32_at(self.structure, self.cursor)
                .ok_or(FdtStructureError::TruncatedToken)?;
            self.cursor += 4;

            match token {
                FDT_BEGIN_NODE => return self.begin_node().map(Some),
                FDT_END_NODE => {
                    if self.depth == 0 {
                        return Err(FdtStructureError::UnexpectedEndNode);
                    }
                    self.depth -= 1;
                    return Ok(Some(FdtStructureEvent::EndNode { depth: self.depth }));
                }
                FDT_PROP => return self.property().map(Some),
                FDT_NOP => {}
                FDT_END => {
                    if !self.root_seen || self.depth != 0 {
                        return Err(FdtStructureError::UnexpectedEnd);
                    }
                    if self.cursor != self.structure.len() {
                        return Err(FdtStructureError::TrailingData);
                    }
                    self.ended = true;
                    return Ok(Some(FdtStructureEvent::End));
                }
                _ => return Err(FdtStructureError::UnknownToken),
            }
        }
    }

    fn begin_node(&mut self) -> Result<FdtStructureEvent<'a>, FdtStructureError> {
        if self.depth == 0 && self.root_seen {
            return Err(FdtStructureError::MultipleRootNodes);
        }

        let name_start = self.cursor;
        let name_bytes = self
            .structure
            .get(name_start..)
            .ok_or(FdtStructureError::UnterminatedNodeName)?;
        let name_end = name_bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(FdtStructureError::UnterminatedNodeName)?;
        let name = core::str::from_utf8(&name_bytes[..name_end])
            .ok()
            .filter(|name| name.is_ascii())
            .ok_or(FdtStructureError::InvalidNodeName)?;

        let after_name = name_start
            .checked_add(name_end + 1)
            .ok_or(FdtStructureError::NodeNameOutOfBounds)?;
        let padded_end = align_to_word(after_name).ok_or(FdtStructureError::NodeNameOutOfBounds)?;
        let padding = self
            .structure
            .get(after_name..padded_end)
            .ok_or(FdtStructureError::NodeNameOutOfBounds)?;
        if padding.iter().any(|byte| *byte != 0) {
            return Err(FdtStructureError::InvalidNodePadding);
        }

        self.cursor = padded_end;
        self.root_seen = true;
        let depth = self.depth;
        self.depth = self
            .depth
            .checked_add(1)
            .ok_or(FdtStructureError::MultipleRootNodes)?;

        Ok(FdtStructureEvent::BeginNode { name, depth })
    }

    fn property(&mut self) -> Result<FdtStructureEvent<'a>, FdtStructureError> {
        if self.depth == 0 {
            return Err(FdtStructureError::PropertyOutsideNode);
        }

        let property_header_end = self
            .cursor
            .checked_add(8)
            .ok_or(FdtStructureError::TruncatedPropertyHeader)?;
        let property_header = self
            .structure
            .get(self.cursor..property_header_end)
            .ok_or(FdtStructureError::TruncatedPropertyHeader)?;
        let value_size = read_u32_be(property_header, 0) as usize;
        let name_offset = read_u32_be(property_header, 4) as usize;
        self.cursor = property_header_end;

        let name_bytes = self
            .strings
            .get(name_offset..)
            .ok_or(FdtStructureError::InvalidPropertyNameOffset)?;
        let name_end = name_bytes
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(FdtStructureError::UnterminatedPropertyName)?;
        let name = core::str::from_utf8(&name_bytes[..name_end])
            .ok()
            .filter(|name| !name.is_empty() && name.is_ascii())
            .ok_or(FdtStructureError::InvalidPropertyName)?;

        let value_end = self
            .cursor
            .checked_add(value_size)
            .ok_or(FdtStructureError::PropertyValueOutOfBounds)?;
        let padded_end =
            align_to_word(value_end).ok_or(FdtStructureError::PropertyValueOutOfBounds)?;
        if self.structure.get(value_end..padded_end).is_none() {
            return Err(FdtStructureError::PropertyValueOutOfBounds);
        }

        self.cursor = padded_end;
        Ok(FdtStructureEvent::Property {
            name,
            value_size,
            depth: self.depth - 1,
        })
    }
}

fn checked_section(blob: &[u8], offset: u32, size: u32, total_size: usize) -> Option<&[u8]> {
    let start = offset as usize;
    let end = start.checked_add(size as usize)?;
    if start < HEADER_SIZE || end > total_size || end > blob.len() {
        return None;
    }
    blob.get(start..end)
}

fn align_to_word(value: usize) -> Option<usize> {
    value.checked_add(3).map(|aligned| aligned & !3)
}

fn read_u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let word = bytes.get(offset..end)?;
    Some(read_u32_be(word, 0))
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
    use super::{
        FdtHeader, FdtHeaderError, FdtStructureError, FdtStructureEvent, FdtStructureWalker,
        FDT_BEGIN_NODE, FDT_END, FDT_END_NODE, FDT_PROP,
    };
    use std::vec::Vec;

    const HEADER_SIZE: usize = 40;
    const STRUCTURE_OFFSET: usize = 56;

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

    fn make_blob(structure: &[u8], strings: &[u8]) -> Vec<u8> {
        let strings_offset = STRUCTURE_OFFSET + structure.len();
        let total_size = strings_offset + strings.len();
        let mut blob = std::vec![0; total_size];

        write_u32_be(&mut blob, 0, 0xd00d_feed);
        write_u32_be(&mut blob, 4, total_size as u32);
        write_u32_be(&mut blob, 8, STRUCTURE_OFFSET as u32);
        write_u32_be(&mut blob, 12, strings_offset as u32);
        write_u32_be(&mut blob, 16, HEADER_SIZE as u32);
        write_u32_be(&mut blob, 20, 17);
        write_u32_be(&mut blob, 24, 16);
        write_u32_be(&mut blob, 32, strings.len() as u32);
        write_u32_be(&mut blob, 36, structure.len() as u32);

        blob[STRUCTURE_OFFSET..strings_offset].copy_from_slice(structure);
        blob[strings_offset..].copy_from_slice(strings);
        blob
    }

    fn append_token(bytes: &mut Vec<u8>, token: u32) {
        bytes.extend_from_slice(&token.to_be_bytes());
    }

    fn append_node(bytes: &mut Vec<u8>, name: &[u8]) {
        append_token(bytes, FDT_BEGIN_NODE);
        bytes.extend_from_slice(name);
        bytes.push(0);
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
    }

    fn append_property(bytes: &mut Vec<u8>, name_offset: u32, value: &[u8]) {
        append_token(bytes, FDT_PROP);
        bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&name_offset.to_be_bytes());
        bytes.extend_from_slice(value);
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
    }

    fn valid_structure_block() -> Vec<u8> {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        append_node(&mut structure, b"soc");
        append_property(&mut structure, 0, b"qemu");
        append_token(&mut structure, FDT_END_NODE);
        append_token(&mut structure, FDT_END_NODE);
        append_token(&mut structure, FDT_END);
        structure
    }

    fn walker_for(structure: &[u8], strings: &[u8]) -> (Vec<u8>, FdtHeader) {
        let blob = make_blob(structure, strings);
        let header = FdtHeader::parse(&blob[..HEADER_SIZE]).unwrap();
        (blob, header)
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

    #[test]
    fn walks_nodes_and_skips_property_values() {
        let structure_bytes = valid_structure_block();
        let (blob, header) = walker_for(&structure_bytes, b"compatible\0");
        let mut walker = FdtStructureWalker::new(&blob, &header).unwrap();

        assert_eq!(
            walker.next_event().unwrap(),
            Some(FdtStructureEvent::BeginNode { name: "", depth: 0 })
        );
        assert_eq!(
            walker.next_event().unwrap(),
            Some(FdtStructureEvent::BeginNode {
                name: "soc",
                depth: 1
            })
        );
        assert_eq!(
            walker.next_event().unwrap(),
            Some(FdtStructureEvent::Property {
                name: "compatible",
                value_size: 4,
                depth: 1
            })
        );
        assert_eq!(
            walker.next_event().unwrap(),
            Some(FdtStructureEvent::EndNode { depth: 1 })
        );
        assert_eq!(
            walker.next_event().unwrap(),
            Some(FdtStructureEvent::EndNode { depth: 0 })
        );
        assert_eq!(walker.next_event().unwrap(), Some(FdtStructureEvent::End));
        assert_eq!(walker.next_event().unwrap(), None);
    }

    #[test]
    fn accepts_nonzero_alignment_bytes() {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        append_property(&mut structure, 0, b"x");
        append_token(&mut structure, FDT_END_NODE);
        append_token(&mut structure, FDT_END);
        structure[21..24].copy_from_slice(&[0xaa, 0xbb, 0xcc]);
        let (blob, header) = walker_for(&structure, b"name\0");
        let mut walker = FdtStructureWalker::new(&blob, &header).unwrap();

        assert_eq!(
            walker.next_event(),
            Ok(Some(FdtStructureEvent::BeginNode { name: "", depth: 0 }))
        );
        assert_eq!(
            walker.next_event(),
            Ok(Some(FdtStructureEvent::Property {
                name: "name",
                value_size: 1,
                depth: 0,
            }))
        );
    }

    #[test]
    fn rejects_nonzero_node_name_padding() {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        structure[5] = 0xff;
        let (blob, header) = walker_for(&structure, b"");
        let mut walker = FdtStructureWalker::new(&blob, &header).unwrap();

        assert_eq!(
            walker.next_event(),
            Err(FdtStructureError::InvalidNodePadding)
        );
    }

    #[test]
    fn rejects_a_truncated_token_at_the_end_of_the_block() {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        append_token(&mut structure, FDT_END_NODE);
        structure.extend_from_slice(&[0, 0]);
        let (blob, header) = walker_for(&structure, b"compatible\0");
        let mut walker = FdtStructureWalker::new(&blob, &header).unwrap();

        while matches!(walker.next_event(), Ok(Some(_))) {}
        assert_eq!(walker.next_event(), Err(FdtStructureError::TruncatedToken));
    }

    #[test]
    fn rejects_an_unterminated_node_name() {
        let mut structure = Vec::new();
        append_token(&mut structure, FDT_BEGIN_NODE);
        structure.extend_from_slice(b"root");
        let (blob, header) = walker_for(&structure, b"");
        let mut walker = FdtStructureWalker::new(&blob, &header).unwrap();

        assert_eq!(
            walker.next_event(),
            Err(FdtStructureError::UnterminatedNodeName)
        );
    }

    #[test]
    fn rejects_a_property_value_outside_the_block() {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        append_token(&mut structure, FDT_PROP);
        structure.extend_from_slice(&0x100u32.to_be_bytes());
        structure.extend_from_slice(&0u32.to_be_bytes());
        let (blob, header) = walker_for(&structure, b"compatible\0");
        let mut walker = FdtStructureWalker::new(&blob, &header).unwrap();

        assert!(matches!(
            walker.next_event(),
            Ok(Some(FdtStructureEvent::BeginNode { .. }))
        ));
        assert_eq!(
            walker.next_event(),
            Err(FdtStructureError::PropertyValueOutOfBounds)
        );
    }

    #[test]
    fn rejects_a_property_name_offset_outside_the_strings_block() {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        append_token(&mut structure, FDT_PROP);
        structure.extend_from_slice(&0u32.to_be_bytes());
        structure.extend_from_slice(&20u32.to_be_bytes());
        let (blob, header) = walker_for(&structure, b"compatible\0");
        let mut walker = FdtStructureWalker::new(&blob, &header).unwrap();

        assert!(matches!(
            walker.next_event(),
            Ok(Some(FdtStructureEvent::BeginNode { .. }))
        ));
        assert_eq!(
            walker.next_event(),
            Err(FdtStructureError::InvalidPropertyNameOffset)
        );
    }

    #[test]
    fn rejects_a_structure_block_without_fdt_end() {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        append_token(&mut structure, FDT_END_NODE);
        let (blob, header) = walker_for(&structure, b"");
        let mut walker = FdtStructureWalker::new(&blob, &header).unwrap();

        assert!(matches!(
            walker.next_event(),
            Ok(Some(FdtStructureEvent::BeginNode { .. }))
        ));
        assert_eq!(
            walker.next_event(),
            Ok(Some(FdtStructureEvent::EndNode { depth: 0 }))
        );
        assert_eq!(walker.next_event(), Err(FdtStructureError::MissingEnd));
    }
}
