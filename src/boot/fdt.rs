//! 扁平设备树（FDT）二进制布局校验、保留表读取和结构块只读遍历。

const FDT_MAGIC: u32 = 0xd00d_feed;
const HEADER_SIZE: usize = 40;
const HEADER_VERSION: u32 = 17;
const RESERVE_ENTRY_SIZE: usize = 16;
const MAX_NODE_DEPTH: usize = 64;

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
    /// 固件启动处理器的物理 ID。
    pub boot_cpuid_phys: u32,
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

fn read_u64_be(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
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
            boot_cpuid_phys: read_u32_be(bytes, 28),
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

/// 完整 DTB 中一个由固件保留的物理内存范围。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FdtMemoryReservation {
    /// 保留范围的物理起始地址。
    pub address: u64,
    /// 保留范围的字节数。
    pub size: u64,
}

/// 完整 DTB 布局或内存保留表不符合格式要求。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FdtBlobError {
    /// 头部解析失败。
    Header(FdtHeaderError),
    /// DTB 实际字节数小于头部声明的总长度。
    BlobTooShort,
    /// 内存保留表偏移没有按 8 字节对齐。
    MisalignedReserveMap,
    /// 结构块偏移没有按 4 字节对齐。
    MisalignedStructure,
    /// 结构块范围无法切分。
    InvalidStructureRange,
    /// 字符串块范围无法切分。
    InvalidStringsRange,
    /// 头部或各区块在 DTB 中互相重叠。
    OverlappingSections,
    /// 内存保留表没有完整的零终止项。
    UnterminatedReserveMap,
    /// 保留范围的地址加长度发生溢出。
    InvalidReservationRange,
    /// 两个保留范围发生重叠。
    OverlappingReservations,
}

impl FdtBlobError {
    /// 返回适合启动诊断的错误说明。
    pub const fn description(self) -> &'static str {
        match self {
            Self::Header(error) => error.description(),
            Self::BlobTooShort => "DTB 总长度超出可读数据",
            Self::MisalignedReserveMap => "内存保留表未按 8 字节对齐",
            Self::MisalignedStructure => "结构块未按 4 字节对齐",
            Self::InvalidStructureRange => "结构块范围无效",
            Self::InvalidStringsRange => "字符串块范围无效",
            Self::OverlappingSections => "DTB 头部或区块相互重叠",
            Self::UnterminatedReserveMap => "内存保留表缺少完整终止项",
            Self::InvalidReservationRange => "内存保留范围地址溢出",
            Self::OverlappingReservations => "内存保留范围相互重叠",
        }
    }
}

/// 通过头部、区块布局和内存保留表基础校验的借用式 DTB 视图。
///
/// 所有区块都借用调用方的字节切片，不复制 DTB 内容，也不申请堆内存。
pub struct FdtBlob<'a> {
    header: FdtHeader,
    reserve_map: &'a [u8],
    structure: &'a [u8],
    strings: &'a [u8],
}

impl<'a> FdtBlob<'a> {
    /// 校验头部、区块布局和内存保留表，并建立不分配内存的只读视图。
    ///
    /// 结构 token 在后续遍历时逐步校验；遍历到 `FdtStructureEvent::End` 才表示结构块完整通过。
    pub fn parse(blob: &'a [u8]) -> Result<Self, FdtBlobError> {
        let header = FdtHeader::parse(blob).map_err(FdtBlobError::Header)?;
        let total_size = header.total_size as usize;
        if blob.len() < total_size {
            return Err(FdtBlobError::BlobTooShort);
        }
        if header.reserve_map_offset as usize % 8 != 0 {
            return Err(FdtBlobError::MisalignedReserveMap);
        }
        if header.structure_offset as usize % 4 != 0 {
            return Err(FdtBlobError::MisalignedStructure);
        }

        let bounded_blob = blob.get(..total_size).ok_or(FdtBlobError::BlobTooShort)?;
        let structure = checked_section(
            bounded_blob,
            header.structure_offset,
            header.structure_size,
            total_size,
        )
        .ok_or(FdtBlobError::InvalidStructureRange)?;
        let strings = checked_section(
            bounded_blob,
            header.strings_offset,
            header.strings_size,
            total_size,
        )
        .ok_or(FdtBlobError::InvalidStringsRange)?;

        let reserve_start = header.reserve_map_offset as usize;
        let reserve_map_end = find_reserve_map_end(bounded_blob, reserve_start, total_size)?;
        let reserve_map = bounded_blob
            .get(reserve_start..reserve_map_end - RESERVE_ENTRY_SIZE)
            .ok_or(FdtBlobError::UnterminatedReserveMap)?;

        let ranges = [
            (0, HEADER_SIZE),
            (reserve_start, reserve_map_end),
            (
                header.structure_offset as usize,
                header.structure_offset as usize + header.structure_size as usize,
            ),
            (
                header.strings_offset as usize,
                header.strings_offset as usize + header.strings_size as usize,
            ),
        ];
        for first in 0..ranges.len() {
            for second in first + 1..ranges.len() {
                if ranges_overlap(ranges[first], ranges[second]) {
                    return Err(FdtBlobError::OverlappingSections);
                }
            }
        }

        validate_reservations(reserve_map)?;

        Ok(Self {
            header,
            reserve_map,
            structure,
            strings,
        })
    }

    /// 返回已经校验过的 DTB 头部。
    pub const fn header(&self) -> &FdtHeader {
        &self.header
    }

    /// 遍历内存保留表；终止项不作为保留范围返回。
    pub fn memory_reservations(&self) -> FdtMemoryReservations<'a> {
        FdtMemoryReservations {
            bytes: self.reserve_map,
            cursor: 0,
        }
    }

    /// 建立结构块事件遍历器；属性事件会借用原始属性值。
    ///
    /// 调用方应持续读取事件直到 `End`，否则尚未访问的结构块尾部也尚未完成校验。
    pub fn structure(&self) -> FdtStructureWalker<'a> {
        FdtStructureWalker {
            structure: self.structure,
            strings: self.strings,
            cursor: 0,
            depth: 0,
            root_seen: false,
            ended: false,
            node_has_child: [false; MAX_NODE_DEPTH],
        }
    }
}

/// 已通过 DTB 校验的内存保留范围迭代器。
pub struct FdtMemoryReservations<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl Iterator for FdtMemoryReservations<'_> {
    type Item = FdtMemoryReservation;

    fn next(&mut self) -> Option<Self::Item> {
        let end = self.cursor.checked_add(RESERVE_ENTRY_SIZE)?;
        let entry = self.bytes.get(self.cursor..end)?;
        self.cursor = end;
        Some(FdtMemoryReservation {
            address: read_u64_be(entry, 0),
            size: read_u64_be(entry, 8),
        })
    }
}

fn find_reserve_map_end(
    blob: &[u8],
    mut cursor: usize,
    total_size: usize,
) -> Result<usize, FdtBlobError> {
    loop {
        let end = cursor
            .checked_add(RESERVE_ENTRY_SIZE)
            .filter(|end| *end <= total_size)
            .ok_or(FdtBlobError::UnterminatedReserveMap)?;
        let entry = blob
            .get(cursor..end)
            .ok_or(FdtBlobError::UnterminatedReserveMap)?;
        if read_u64_be(entry, 0) == 0 && read_u64_be(entry, 8) == 0 {
            return Ok(end);
        }
        cursor = end;
    }
}

fn validate_reservations(entries: &[u8]) -> Result<(), FdtBlobError> {
    let count = entries.len() / RESERVE_ENTRY_SIZE;
    for first in 0..count {
        let first_entry = &entries[first * RESERVE_ENTRY_SIZE..(first + 1) * RESERVE_ENTRY_SIZE];
        let first_start = read_u64_be(first_entry, 0);
        let first_end = first_start
            .checked_add(read_u64_be(first_entry, 8))
            .ok_or(FdtBlobError::InvalidReservationRange)?;

        for second in first + 1..count {
            let second_entry =
                &entries[second * RESERVE_ENTRY_SIZE..(second + 1) * RESERVE_ENTRY_SIZE];
            let second_start = read_u64_be(second_entry, 0);
            let second_end = second_start
                .checked_add(read_u64_be(second_entry, 8))
                .ok_or(FdtBlobError::InvalidReservationRange)?;
            if first_start < second_end && second_start < first_end {
                return Err(FdtBlobError::OverlappingReservations);
            }
        }
    }
    Ok(())
}

fn ranges_overlap(first: (usize, usize), second: (usize, usize)) -> bool {
    first.0 < second.1 && second.0 < first.1
}

const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

/// FDT 结构块遍历时发现的格式错误。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FdtStructureError {
    /// 结构块末尾存在不完整的 token。
    TruncatedToken,
    /// 节点名称缺少结尾零字节。
    UnterminatedNodeName,
    /// 节点名称不是有效的 ASCII 文本。
    InvalidNodeName,
    /// 根节点名称不为空。
    InvalidRootNodeName,
    /// 非根节点的名称为空。
    EmptyChildNodeName,
    /// 节点名称后的对齐范围越出结构块。
    NodeNameOutOfBounds,
    /// 节点名称后的规范要求填充字节不为零。
    InvalidNodePadding,
    /// 根节点之后又出现了另一个根节点。
    MultipleRootNodes,
    /// 节点嵌套超过本实现使用的固定状态栈容量。
    NodeDepthLimitExceeded,
    /// 当前没有打开的节点，却遇到了节点结束 token。
    UnexpectedEndNode,
    /// 根节点之外出现属性。
    PropertyOutsideNode,
    /// 同一节点的属性出现在子节点之后。
    PropertyAfterChild,
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
            Self::TruncatedToken => "结构块末尾存在不完整 token",
            Self::UnterminatedNodeName => "节点名称未结束",
            Self::InvalidNodeName => "节点名称不是有效 ASCII 文本",
            Self::InvalidRootNodeName => "根节点名称必须为空",
            Self::EmptyChildNodeName => "子节点名称不能为空",
            Self::NodeNameOutOfBounds => "节点名称对齐范围越界",
            Self::InvalidNodePadding => "节点名称填充字节不是零",
            Self::MultipleRootNodes => "出现多个根节点",
            Self::NodeDepthLimitExceeded => "节点嵌套超过解析器深度上限",
            Self::UnexpectedEndNode => "节点结束 token 没有对应的开始节点",
            Self::PropertyOutsideNode => "根节点之外出现属性",
            Self::PropertyAfterChild => "节点属性出现在子节点之后",
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

/// 结构块遍历器返回的事件；属性值以借用切片提供，不复制 DTB 数据。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FdtStructureEvent<'a> {
    /// 开始一个节点，根节点深度为 0。
    BeginNode { name: &'a str, depth: usize },
    /// 结束一个节点，根节点深度为 0。
    EndNode { depth: usize },
    /// 遇到一个属性，提供名称、原始值切片和所属节点深度。
    Property {
        name: &'a str,
        value: &'a [u8],
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
    node_has_child: [bool; MAX_NODE_DEPTH],
}

impl<'a> FdtStructureWalker<'a> {
    /// 读取下一个事件；返回 `None` 表示已读到结构块末尾的 FDT_END。
    ///
    /// 返回错误后遍历器不再保证可继续使用；调用方应停止遍历并报告错误。
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
        if self.depth == MAX_NODE_DEPTH {
            return Err(FdtStructureError::NodeDepthLimitExceeded);
        }
        if self.depth > 0 {
            self.node_has_child[self.depth - 1] = true;
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
        if self.depth == 0 && !name.is_empty() {
            return Err(FdtStructureError::InvalidRootNodeName);
        }
        if self.depth > 0 && name.is_empty() {
            return Err(FdtStructureError::EmptyChildNodeName);
        }

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
        self.node_has_child[depth] = false;
        self.depth += 1;

        Ok(FdtStructureEvent::BeginNode { name, depth })
    }

    fn property(&mut self) -> Result<FdtStructureEvent<'a>, FdtStructureError> {
        if self.depth == 0 {
            return Err(FdtStructureError::PropertyOutsideNode);
        }
        if self.node_has_child[self.depth - 1] {
            return Err(FdtStructureError::PropertyAfterChild);
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

        let value = self
            .structure
            .get(self.cursor..value_end)
            .ok_or(FdtStructureError::PropertyValueOutOfBounds)?;
        self.cursor = padded_end;
        Ok(FdtStructureEvent::Property {
            name,
            value,
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
        FdtBlob, FdtBlobError, FdtHeader, FdtHeaderError, FdtStructureError, FdtStructureEvent,
        FDT_BEGIN_NODE, FDT_END, FDT_END_NODE, FDT_PROP,
    };
    use std::vec::Vec;

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
        write_u32_be(&mut bytes, 28, 2);
        write_u32_be(&mut bytes, 32, 0x80);
        write_u32_be(&mut bytes, 36, 0x100);
        bytes
    }

    fn write_u32_be(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn make_blob(structure: &[u8], strings: &[u8]) -> Vec<u8> {
        make_blob_with_reservations(structure, strings, &[])
    }

    fn make_blob_with_reservations(
        structure: &[u8],
        strings: &[u8],
        reservations: &[(u64, u64)],
    ) -> Vec<u8> {
        let structure_offset = HEADER_SIZE + (reservations.len() + 1) * 16;
        let strings_offset = structure_offset + structure.len();
        let total_size = strings_offset + strings.len();
        let mut blob = std::vec![0; total_size];

        write_u32_be(&mut blob, 0, 0xd00d_feed);
        write_u32_be(&mut blob, 4, total_size as u32);
        write_u32_be(&mut blob, 8, structure_offset as u32);
        write_u32_be(&mut blob, 12, strings_offset as u32);
        write_u32_be(&mut blob, 16, HEADER_SIZE as u32);
        write_u32_be(&mut blob, 20, 17);
        write_u32_be(&mut blob, 24, 16);
        write_u32_be(&mut blob, 32, strings.len() as u32);
        write_u32_be(&mut blob, 36, structure.len() as u32);

        for (index, (address, size)) in reservations.iter().enumerate() {
            let offset = HEADER_SIZE + index * 16;
            blob[offset..offset + 8].copy_from_slice(&address.to_be_bytes());
            blob[offset + 8..offset + 16].copy_from_slice(&size.to_be_bytes());
        }

        blob[structure_offset..strings_offset].copy_from_slice(structure);
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

    fn blob_for(structure: &[u8], strings: &[u8]) -> Vec<u8> {
        make_blob(structure, strings)
    }

    #[test]
    fn accepts_a_valid_version_17_header() {
        let header = FdtHeader::parse(&valid_header_bytes()).unwrap();

        assert_eq!(header.version, 17);
        assert_eq!(header.total_size, 0x1000);
        assert_eq!(header.boot_cpuid_phys, 2);
        assert_eq!(header.structure_offset, 0x40);
        assert_eq!(header.structure_size, 0x100);
        assert_eq!(header.strings_offset, 0x200);
        assert_eq!(header.strings_size, 0x80);
    }

    #[test]
    fn reads_memory_reservations_without_copying_the_table() {
        let structure = valid_structure_block();
        let blob = make_blob_with_reservations(
            &structure,
            b"compatible\0",
            &[(0x8000_0000, 0x1000), (0x9000_0000, 0x2000)],
        );
        let fdt = FdtBlob::parse(&blob).unwrap();
        let reservations: Vec<_> = fdt.memory_reservations().collect();

        assert_eq!(reservations.len(), 2);
        assert_eq!(reservations[0].address, 0x8000_0000);
        assert_eq!(reservations[0].size, 0x1000);
        assert_eq!(reservations[1].address, 0x9000_0000);
        assert_eq!(reservations[1].size, 0x2000);
    }

    #[test]
    fn rejects_a_blob_shorter_than_its_declared_total_size() {
        let structure = valid_structure_block();
        let blob = make_blob(&structure, b"compatible\0");

        assert!(matches!(
            FdtBlob::parse(&blob[..blob.len() - 1]),
            Err(FdtBlobError::BlobTooShort)
        ));
    }

    #[test]
    fn rejects_misaligned_reservation_and_structure_blocks() {
        let structure = valid_structure_block();
        let mut blob = make_blob(&structure, b"compatible\0");
        write_u32_be(&mut blob, 16, 41);
        assert!(matches!(
            FdtBlob::parse(&blob),
            Err(FdtBlobError::MisalignedReserveMap)
        ));

        let mut blob = make_blob(&structure, b"compatible\0");
        write_u32_be(&mut blob, 8, 57);
        assert!(matches!(
            FdtBlob::parse(&blob),
            Err(FdtBlobError::MisalignedStructure)
        ));
    }

    #[test]
    fn rejects_overlapping_dtb_sections() {
        let structure = valid_structure_block();
        let mut blob = make_blob(&structure, b"compatible\0");
        let structure_offset = u32::from_be_bytes(blob[8..12].try_into().unwrap());
        write_u32_be(&mut blob, 12, structure_offset + 4);
        assert!(matches!(
            FdtBlob::parse(&blob),
            Err(FdtBlobError::OverlappingSections)
        ));
    }

    #[test]
    fn rejects_a_reservation_table_without_its_terminator() {
        let structure = valid_structure_block();
        let mut blob = make_blob(&structure, b"compatible\0");
        blob[47] = 1;

        assert!(matches!(
            FdtBlob::parse(&blob),
            Err(FdtBlobError::UnterminatedReserveMap)
        ));
    }

    #[test]
    fn rejects_overlapping_memory_reservations() {
        let structure = valid_structure_block();
        let blob = make_blob_with_reservations(
            &structure,
            b"compatible\0",
            &[(0x8000_0000, 0x2000), (0x8000_1000, 0x1000)],
        );

        assert!(matches!(
            FdtBlob::parse(&blob),
            Err(FdtBlobError::OverlappingReservations)
        ));
    }

    #[test]
    fn rejects_a_memory_reservation_that_overflows() {
        let structure = valid_structure_block();
        let blob = make_blob_with_reservations(&structure, b"compatible\0", &[(u64::MAX - 7, 16)]);

        assert!(matches!(
            FdtBlob::parse(&blob),
            Err(FdtBlobError::InvalidReservationRange)
        ));
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
    fn yields_borrowed_property_values_while_walking_nodes() {
        let structure_bytes = valid_structure_block();
        let blob = blob_for(&structure_bytes, b"compatible\0");
        let fdt = FdtBlob::parse(&blob).unwrap();
        let mut walker = fdt.structure();

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
                value: b"qemu",
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
    fn rejects_a_property_after_a_child_node() {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        append_node(&mut structure, b"child");
        append_token(&mut structure, FDT_END_NODE);
        append_property(&mut structure, 0, b"value");
        append_token(&mut structure, FDT_END_NODE);
        append_token(&mut structure, FDT_END);
        let blob = blob_for(&structure, b"property\0");
        let fdt = FdtBlob::parse(&blob).unwrap();
        let mut walker = fdt.structure();

        assert!(matches!(
            walker.next_event(),
            Ok(Some(FdtStructureEvent::BeginNode { name: "", .. }))
        ));
        assert!(matches!(
            walker.next_event(),
            Ok(Some(FdtStructureEvent::BeginNode { name: "child", .. }))
        ));
        assert_eq!(
            walker.next_event(),
            Ok(Some(FdtStructureEvent::EndNode { depth: 1 }))
        );
        assert_eq!(
            walker.next_event(),
            Err(FdtStructureError::PropertyAfterChild)
        );
    }

    #[test]
    fn rejects_nesting_beyond_the_documented_depth_limit() {
        let mut structure = Vec::new();
        for depth in 0..=64 {
            append_node(&mut structure, if depth == 0 { b"" } else { b"n" });
        }
        let blob = blob_for(&structure, b"");
        let fdt = FdtBlob::parse(&blob).unwrap();
        let mut walker = fdt.structure();

        for _ in 0..64 {
            assert!(matches!(
                walker.next_event(),
                Ok(Some(FdtStructureEvent::BeginNode { .. }))
            ));
        }
        assert_eq!(
            walker.next_event(),
            Err(FdtStructureError::NodeDepthLimitExceeded)
        );
    }

    #[test]
    fn accepts_nonzero_property_alignment_bytes() {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        append_property(&mut structure, 0, b"x");
        append_token(&mut structure, FDT_END_NODE);
        append_token(&mut structure, FDT_END);
        structure[21..24].copy_from_slice(&[0xaa, 0xbb, 0xcc]);
        let blob = blob_for(&structure, b"name\0");
        let fdt = FdtBlob::parse(&blob).unwrap();
        let mut walker = fdt.structure();

        assert_eq!(
            walker.next_event(),
            Ok(Some(FdtStructureEvent::BeginNode { name: "", depth: 0 }))
        );
        assert_eq!(
            walker.next_event(),
            Ok(Some(FdtStructureEvent::Property {
                name: "name",
                value: b"x",
                depth: 0,
            }))
        );
    }

    #[test]
    fn rejects_nonzero_node_name_padding() {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        structure[5] = 0xff;
        let blob = blob_for(&structure, b"");
        let fdt = FdtBlob::parse(&blob).unwrap();
        let mut walker = fdt.structure();

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
        let blob = blob_for(&structure, b"compatible\0");
        let fdt = FdtBlob::parse(&blob).unwrap();
        let mut walker = fdt.structure();

        while matches!(walker.next_event(), Ok(Some(_))) {}
        assert_eq!(walker.next_event(), Err(FdtStructureError::TruncatedToken));
    }

    #[test]
    fn rejects_an_unterminated_node_name() {
        let mut structure = Vec::new();
        append_token(&mut structure, FDT_BEGIN_NODE);
        structure.extend_from_slice(b"root");
        let blob = blob_for(&structure, b"");
        let fdt = FdtBlob::parse(&blob).unwrap();
        let mut walker = fdt.structure();

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
        let blob = blob_for(&structure, b"compatible\0");
        let fdt = FdtBlob::parse(&blob).unwrap();
        let mut walker = fdt.structure();

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
        let blob = blob_for(&structure, b"compatible\0");
        let fdt = FdtBlob::parse(&blob).unwrap();
        let mut walker = fdt.structure();

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
        let blob = blob_for(&structure, b"");
        let fdt = FdtBlob::parse(&blob).unwrap();
        let mut walker = fdt.structure();

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
