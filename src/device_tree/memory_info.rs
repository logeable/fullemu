//! 从 FDT 中提取内核早期内存初始化所需的平台信息。
//!
//! 本模块目前只解释根节点下的内存节点，并复制固件和 DTB 自身的保留范围。
//! 它不负责分配物理页，也不解释 UART、中断控制器或其他设备 binding。

use super::fdt::{FdtBlob, FdtStructureError, FdtStructureEvent};

const MAX_MEMORY_REGIONS: usize = 32;
const MAX_MEMORY_RESERVATIONS: usize = 32;
const DEFAULT_ADDRESS_CELLS: u32 = 2;
const DEFAULT_SIZE_CELLS: u32 = 1;

/// 可供内核后续内存管理使用的物理内存范围。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PhysicalMemoryRegion {
    /// 范围起始物理地址。
    pub start: u64,
    /// 范围长度，单位为字节。
    pub size: u64,
}

impl PhysicalMemoryRegion {
    /// 返回范围末尾的排他地址；只有地址加长度溢出时才返回 `None`。
    pub const fn end_exclusive(self) -> Option<u64> {
        self.start.checked_add(self.size)
    }
}

/// FDT 中的内存描述无法转换为本内核支持的启动信息。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FdtMemoryInfoError {
    /// 结构块不符合 FDT 格式。
    InvalidStructure(FdtStructureError),
    /// FDT 中没有找到根节点下的内存节点。
    MissingMemoryNode,
    /// 内存节点缺少 `reg` 属性。
    MissingMemoryReg,
    /// 根节点的 cell 属性不是单个 32 位大端值。
    InvalidCellProperty,
    /// 地址或长度 cell 数不是本阶段支持的 1 或 2。
    UnsupportedCellCount,
    /// `reg` 的长度无法组成完整的地址/长度项。
    InvalidRegLength,
    /// 内存范围长度为零或地址加长度溢出。
    InvalidMemoryRange,
    /// 内存描述中有重叠范围。
    OverlappingMemoryRegions,
    /// 内存范围数超过固定容量。
    TooManyMemoryRegions,
    /// 固件保留范围数超过固定容量。
    TooManyMemoryReservations,
    /// DTB 起始地址与声明长度发生溢出。
    InvalidDeviceTreeRange,
}

impl FdtMemoryInfoError {
    /// 返回适合启动诊断的错误说明。
    pub const fn description(self) -> &'static str {
        match self {
            Self::InvalidStructure(error) => error.description(),
            Self::MissingMemoryNode => "FDT 缺少内存节点",
            Self::MissingMemoryReg => "内存节点缺少 reg 属性",
            Self::InvalidCellProperty => "根节点 cell 属性格式无效",
            Self::UnsupportedCellCount => "内存地址或长度 cell 数不受支持",
            Self::InvalidRegLength => "内存 reg 属性长度无效",
            Self::InvalidMemoryRange => "内存范围为空或地址溢出",
            Self::OverlappingMemoryRegions => "内存范围相互重叠",
            Self::TooManyMemoryRegions => "内存范围数量超过启动信息容量",
            Self::TooManyMemoryReservations => "固件保留范围数量超过启动信息容量",
            Self::InvalidDeviceTreeRange => "DTB 物理范围地址溢出",
        }
    }
}

/// 从 FDT 提取出的早期启动信息。
///
/// 内存范围、保留范围和 DTB 本身的物理范围均由固定容量数据拥有，不需要堆分配。
pub struct FdtMemoryInfo {
    memory_regions: [PhysicalMemoryRegion; MAX_MEMORY_REGIONS],
    memory_region_count: usize,
    memory_reservations: [PhysicalMemoryRegion; MAX_MEMORY_RESERVATIONS],
    memory_reservation_count: usize,
    device_tree_region: PhysicalMemoryRegion,
}

impl FdtMemoryInfo {
    /// 从已校验 DTB 中提取初始化所需范围；所有结果独立于 DTB 生命周期且不分配堆内存。
    pub fn parse(fdt: &FdtBlob<'_>, device_tree_start: u64) -> Result<Self, FdtMemoryInfoError> {
        let device_tree_region = PhysicalMemoryRegion {
            start: device_tree_start,
            size: fdt.header().total_size as u64,
        };
        if device_tree_region.end_exclusive().is_none() {
            return Err(FdtMemoryInfoError::InvalidDeviceTreeRange);
        }

        let mut memory_info = Self {
            memory_regions: [PhysicalMemoryRegion::default(); MAX_MEMORY_REGIONS],
            memory_region_count: 0,
            memory_reservations: [PhysicalMemoryRegion::default(); MAX_MEMORY_RESERVATIONS],
            memory_reservation_count: 0,
            device_tree_region,
        };
        for reservation in fdt.memory_reservations() {
            let slot = memory_info
                .memory_reservations
                .get_mut(memory_info.memory_reservation_count)
                .ok_or(FdtMemoryInfoError::TooManyMemoryReservations)?;
            *slot = PhysicalMemoryRegion {
                start: reservation.address,
                size: reservation.size,
            };
            memory_info.memory_reservation_count += 1;
        }

        let mut address_cells = DEFAULT_ADDRESS_CELLS;
        let mut size_cells = DEFAULT_SIZE_CELLS;
        let mut current_root_child = None;
        let mut structure = fdt.structure();

        loop {
            let event = structure
                .next_event()
                .map_err(FdtMemoryInfoError::InvalidStructure)?;
            match event {
                Some(FdtStructureEvent::BeginNode { name, depth: 1 }) => {
                    current_root_child = Some(RootChild::new(name));
                }
                Some(FdtStructureEvent::Property {
                    name: "#address-cells",
                    value,
                    depth: 0,
                }) => address_cells = read_cell_property(value)?,
                Some(FdtStructureEvent::Property {
                    name: "#size-cells",
                    value,
                    depth: 0,
                }) => size_cells = read_cell_property(value)?,
                Some(FdtStructureEvent::Property {
                    name,
                    value,
                    depth: 1,
                }) => {
                    if let Some(child) = current_root_child.as_mut() {
                        child.record_property(name, value);
                    }
                }
                Some(FdtStructureEvent::EndNode { depth: 1 }) => {
                    if let Some(child) = current_root_child.take() {
                        if child.is_memory_node() {
                            let reg = child.reg.ok_or(FdtMemoryInfoError::MissingMemoryReg)?;
                            memory_info.append_memory_ranges(reg, address_cells, size_cells)?;
                        }
                    }
                }
                Some(FdtStructureEvent::End) => break,
                Some(_) => {}
                None => break,
            }
        }

        if memory_info.memory_region_count == 0 {
            return Err(FdtMemoryInfoError::MissingMemoryNode);
        }

        Ok(memory_info)
    }

    /// 返回 FDT 描述的物理内存范围。
    pub fn memory_regions(&self) -> &[PhysicalMemoryRegion] {
        &self.memory_regions[..self.memory_region_count]
    }

    /// 返回固件保留范围的独立切片，不再借用原始 DTB。
    pub fn memory_reservations(&self) -> &[PhysicalMemoryRegion] {
        &self.memory_reservations[..self.memory_reservation_count]
    }

    /// 返回固件传入 DTB 所占据的物理范围。
    pub const fn device_tree_region(&self) -> PhysicalMemoryRegion {
        self.device_tree_region
    }

    /// 检查静态内核存储是否处于描述的 RAM 内，且未与 DTB 或固件保留范围重叠。
    ///
    /// 此检查只适用于验证已由链接布局确定的内核存储区域，不等同于通用物理内存分配策略。
    pub fn contains_unreserved_ram_region(&self, region: PhysicalMemoryRegion) -> bool {
        let Some(region_end) = region.end_exclusive() else {
            return false;
        };
        if region.size == 0
            || !self.memory_regions().iter().any(|memory| {
                memory.end_exclusive().is_some_and(|memory_end| {
                    memory.start <= region.start && region_end <= memory_end
                })
            })
            || ranges_overlap(region, self.device_tree_region)
        {
            return false;
        }

        !self
            .memory_reservations()
            .iter()
            .copied()
            .any(|reservation| ranges_overlap(region, reservation))
    }

    fn append_memory_ranges(
        &mut self,
        reg: &[u8],
        address_cells: u32,
        size_cells: u32,
    ) -> Result<(), FdtMemoryInfoError> {
        validate_cell_count(address_cells)?;
        validate_cell_count(size_cells)?;

        let tuple_cells = address_cells + size_cells;
        let tuple_size = tuple_cells as usize * 4;
        if reg.is_empty() || reg.len() % tuple_size != 0 {
            return Err(FdtMemoryInfoError::InvalidRegLength);
        }

        for tuple in reg.chunks_exact(tuple_size) {
            let address_size = address_cells as usize * 4;
            let address = decode_cells(&tuple[..address_size], address_cells)?;
            let size = decode_cells(&tuple[address_size..], size_cells)?;
            if size == 0 || address.checked_add(size).is_none() {
                return Err(FdtMemoryInfoError::InvalidMemoryRange);
            }
            self.push_memory_region(PhysicalMemoryRegion {
                start: address,
                size,
            })?;
        }
        Ok(())
    }

    fn push_memory_region(
        &mut self,
        region: PhysicalMemoryRegion,
    ) -> Result<(), FdtMemoryInfoError> {
        let region_end = region
            .end_exclusive()
            .ok_or(FdtMemoryInfoError::InvalidMemoryRange)?;
        for existing in self.memory_regions() {
            let existing_end = existing
                .end_exclusive()
                .ok_or(FdtMemoryInfoError::InvalidMemoryRange)?;
            if region.start < existing_end && existing.start < region_end {
                return Err(FdtMemoryInfoError::OverlappingMemoryRegions);
            }
        }

        let slot = self
            .memory_regions
            .get_mut(self.memory_region_count)
            .ok_or(FdtMemoryInfoError::TooManyMemoryRegions)?;
        *slot = region;
        self.memory_region_count += 1;
        Ok(())
    }
}

fn ranges_overlap(first: PhysicalMemoryRegion, second: PhysicalMemoryRegion) -> bool {
    let (Some(first_end), Some(second_end)) = (first.end_exclusive(), second.end_exclusive())
    else {
        return true;
    };
    first.start < second_end && second.start < first_end
}

struct RootChild<'a> {
    name: &'a str,
    device_type_is_memory: bool,
    reg: Option<&'a [u8]>,
}

impl<'a> RootChild<'a> {
    fn new(name: &'a str) -> Self {
        Self {
            name,
            device_type_is_memory: false,
            reg: None,
        }
    }

    fn record_property(&mut self, name: &str, value: &'a [u8]) {
        match name {
            "device_type" => self.device_type_is_memory = value == b"memory\0",
            "reg" => self.reg = Some(value),
            _ => {}
        }
    }

    fn is_memory_node(&self) -> bool {
        self.device_type_is_memory || self.name == "memory" || self.name.starts_with("memory@")
    }
}

fn read_cell_property(value: &[u8]) -> Result<u32, FdtMemoryInfoError> {
    let bytes: [u8; 4] = value
        .try_into()
        .map_err(|_| FdtMemoryInfoError::InvalidCellProperty)?;
    Ok(u32::from_be_bytes(bytes))
}

fn validate_cell_count(cells: u32) -> Result<(), FdtMemoryInfoError> {
    if cells == 1 || cells == 2 {
        Ok(())
    } else {
        Err(FdtMemoryInfoError::UnsupportedCellCount)
    }
}

fn decode_cells(bytes: &[u8], cell_count: u32) -> Result<u64, FdtMemoryInfoError> {
    match cell_count {
        1 => Ok(u32::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| FdtMemoryInfoError::InvalidRegLength)?,
        ) as u64),
        2 => Ok(u64::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| FdtMemoryInfoError::InvalidRegLength)?,
        )),
        _ => Err(FdtMemoryInfoError::UnsupportedCellCount),
    }
}

#[cfg(test)]
mod tests {
    use super::{FdtMemoryInfo, FdtMemoryInfoError, PhysicalMemoryRegion};
    use crate::device_tree::fdt::FdtBlob;
    use std::vec::Vec;

    const HEADER_SIZE: usize = 40;
    const STRUCTURE_OFFSET: usize = 56;
    const STRINGS: &[u8] = b"#address-cells\0#size-cells\0device_type\0reg\0";

    fn append_token(bytes: &mut Vec<u8>, token: u32) {
        bytes.extend_from_slice(&token.to_be_bytes());
    }

    fn append_node(bytes: &mut Vec<u8>, name: &[u8]) {
        append_token(bytes, 1);
        bytes.extend_from_slice(name);
        bytes.push(0);
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
    }

    fn append_property(bytes: &mut Vec<u8>, name: &[u8], value: &[u8]) {
        let mut name_with_nul = name.to_vec();
        name_with_nul.push(0);
        let name_offset = STRINGS
            .windows(name_with_nul.len())
            .position(|window| window == name_with_nul)
            .unwrap() as u32;

        append_token(bytes, 3);
        bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&name_offset.to_be_bytes());
        bytes.extend_from_slice(value);
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
    }

    fn append_root_cells(bytes: &mut Vec<u8>, address_cells: u32, size_cells: u32) {
        append_property(bytes, b"#address-cells", &address_cells.to_be_bytes());
        append_property(bytes, b"#size-cells", &size_cells.to_be_bytes());
    }

    fn append_memory_node(bytes: &mut Vec<u8>, name: &[u8], reg: &[u8]) {
        append_node(bytes, name);
        append_property(bytes, b"device_type", b"memory\0");
        append_property(bytes, b"reg", reg);
        append_token(bytes, 2);
    }

    fn make_fdt(structure: &[u8]) -> Vec<u8> {
        let strings_offset = STRUCTURE_OFFSET + structure.len();
        let total_size = strings_offset + STRINGS.len();
        let mut blob = std::vec![0; total_size];
        write_u32_be(&mut blob, 0, 0xd00d_feed);
        write_u32_be(&mut blob, 4, total_size as u32);
        write_u32_be(&mut blob, 8, STRUCTURE_OFFSET as u32);
        write_u32_be(&mut blob, 12, strings_offset as u32);
        write_u32_be(&mut blob, 16, HEADER_SIZE as u32);
        write_u32_be(&mut blob, 20, 17);
        write_u32_be(&mut blob, 24, 16);
        write_u32_be(&mut blob, 32, STRINGS.len() as u32);
        write_u32_be(&mut blob, 36, structure.len() as u32);
        blob[STRUCTURE_OFFSET..strings_offset].copy_from_slice(structure);
        blob[strings_offset..].copy_from_slice(STRINGS);
        blob
    }

    fn write_u32_be(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn append_cell_pair(reg: &mut Vec<u8>, address: u64, size: u64) {
        reg.extend_from_slice(&address.to_be_bytes());
        reg.extend_from_slice(&size.to_be_bytes());
    }

    fn valid_structure(reg: &[u8]) -> Vec<u8> {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        append_root_cells(&mut structure, 2, 2);
        append_memory_node(&mut structure, b"memory@80000000", reg);
        append_token(&mut structure, 2);
        append_token(&mut structure, 9);
        structure
    }

    #[test]
    fn extracts_memory_regions_and_firmware_reservations() {
        let mut reg = Vec::new();
        append_cell_pair(&mut reg, 0x8000_0000, 0x0800_0000);
        append_cell_pair(&mut reg, 0x9000_0000, 0x0100_0000);
        let blob = make_fdt(&valid_structure(&reg));
        let fdt = FdtBlob::parse(&blob).unwrap();
        let memory_info = FdtMemoryInfo::parse(&fdt, 0x1000).unwrap();

        assert_eq!(
            memory_info.memory_regions(),
            &[
                PhysicalMemoryRegion {
                    start: 0x8000_0000,
                    size: 0x0800_0000,
                },
                PhysicalMemoryRegion {
                    start: 0x9000_0000,
                    size: 0x0100_0000,
                },
            ]
        );
        assert!(memory_info.memory_reservations().is_empty());
    }

    #[test]
    fn uses_standard_cell_defaults_when_the_root_omits_them() {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        let mut reg = Vec::new();
        reg.extend_from_slice(&0u32.to_be_bytes());
        reg.extend_from_slice(&0x8000_0000u32.to_be_bytes());
        reg.extend_from_slice(&0x1000u32.to_be_bytes());
        append_memory_node(&mut structure, b"ram", &reg);
        append_token(&mut structure, 2);
        append_token(&mut structure, 9);
        let blob = make_fdt(&structure);
        let fdt = FdtBlob::parse(&blob).unwrap();
        let memory_info = FdtMemoryInfo::parse(&fdt, 0x1000).unwrap();

        assert_eq!(
            memory_info.memory_regions(),
            &[PhysicalMemoryRegion {
                start: 0x8000_0000,
                size: 0x1000,
            }]
        );
    }

    #[test]
    fn rejects_a_memory_node_without_reg() {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        append_root_cells(&mut structure, 2, 2);
        append_node(&mut structure, b"memory@80000000");
        append_property(&mut structure, b"device_type", b"memory\0");
        append_token(&mut structure, 2);
        append_token(&mut structure, 2);
        append_token(&mut structure, 9);
        let blob = make_fdt(&structure);
        let fdt = FdtBlob::parse(&blob).unwrap();

        assert!(matches!(
            FdtMemoryInfo::parse(&fdt, 0x1000),
            Err(FdtMemoryInfoError::MissingMemoryReg)
        ));
    }

    #[test]
    fn rejects_an_incomplete_reg_tuple() {
        let blob = make_fdt(&valid_structure(&[0; 15]));
        let fdt = FdtBlob::parse(&blob).unwrap();

        assert!(matches!(
            FdtMemoryInfo::parse(&fdt, 0x1000),
            Err(FdtMemoryInfoError::InvalidRegLength)
        ));
    }

    #[test]
    fn rejects_overlapping_memory_regions() {
        let mut reg = Vec::new();
        append_cell_pair(&mut reg, 0x8000_0000, 0x2000);
        append_cell_pair(&mut reg, 0x8000_1000, 0x1000);
        let blob = make_fdt(&valid_structure(&reg));
        let fdt = FdtBlob::parse(&blob).unwrap();

        assert!(matches!(
            FdtMemoryInfo::parse(&fdt, 0x1000),
            Err(FdtMemoryInfoError::OverlappingMemoryRegions)
        ));
    }

    #[test]
    fn rejects_cell_counts_that_do_not_fit_rv64_addresses() {
        let mut structure = Vec::new();
        append_node(&mut structure, b"");
        append_root_cells(&mut structure, 3, 2);
        let mut reg = Vec::new();
        append_cell_pair(&mut reg, 0x8000_0000, 0x1000);
        append_memory_node(&mut structure, b"memory@80000000", &reg);
        append_token(&mut structure, 2);
        append_token(&mut structure, 9);
        let blob = make_fdt(&structure);
        let fdt = FdtBlob::parse(&blob).unwrap();

        assert!(matches!(
            FdtMemoryInfo::parse(&fdt, 0x1000),
            Err(FdtMemoryInfoError::UnsupportedCellCount)
        ));
    }
}
