//! 从 FDT 中提取 QEMU `virt` 所用的 VirtIO MMIO transport 范围。

use super::fdt::{FdtBlob, FdtStructureError, FdtStructureEvent};
use super::FdtDeviceRegisterRegion;

const DEFAULT_ADDRESS_CELLS: u32 = 2;
const DEFAULT_SIZE_CELLS: u32 = 1;
const MAX_VIRTIO_MMIO_DEVICES: usize = 32;
const MINIMUM_TRANSPORT_SIZE: u64 = 0x100;

/// FDT 中 VirtIO MMIO transport 的数量或结构不受支持。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FdtVirtioInfoError {
    /// FDT 结构块不符合格式。
    InvalidStructure(FdtStructureError),
    /// `/soc` 节点不存在。
    MissingSocNode,
    /// `/soc` 没有声明空 `ranges`，当前实现无法转换子总线地址。
    UnsupportedSocAddressTranslation,
    /// 没有找到启用的 VirtIO MMIO transport。
    MissingVirtioMmioDevice,
    /// 有效设备数量超过固定启动期容量。
    TooManyVirtioMmioDevices,
    /// 候选节点缺少 `reg` 属性。
    MissingReg,
    /// 地址或长度 cell 数不受支持。
    UnsupportedCellCount,
    /// `reg` 属性无法组成完整的地址/长度项。
    InvalidRegLength,
    /// transport 范围为空、过小或发生地址溢出。
    InvalidRegisterRegion,
}

impl FdtVirtioInfoError {
    /// 返回适合启动诊断的错误说明。
    pub const fn description(self) -> &'static str {
        match self {
            Self::InvalidStructure(error) => error.description(),
            Self::MissingSocNode => "FDT 缺少 /soc 总线节点",
            Self::UnsupportedSocAddressTranslation => "FDT /soc ranges 不是空的恒等映射",
            Self::MissingVirtioMmioDevice => "FDT 中没有启用的 VirtIO MMIO transport",
            Self::TooManyVirtioMmioDevices => "VirtIO MMIO transport 数量超过启动期容量",
            Self::MissingReg => "FDT VirtIO MMIO 节点缺少 reg 属性",
            Self::UnsupportedCellCount => "VirtIO 地址或长度 cell 数不受支持",
            Self::InvalidRegLength => "FDT VirtIO reg 属性长度无效",
            Self::InvalidRegisterRegion => "FDT VirtIO MMIO 寄存器范围无效",
        }
    }
}

/// 启动期从设备树复制出的 VirtIO MMIO transport 列表。
pub struct FdtVirtioInfo {
    regions: [FdtDeviceRegisterRegion; MAX_VIRTIO_MMIO_DEVICES],
    region_count: usize,
}

impl FdtVirtioInfo {
    /// 收集 `/soc` 下启用的 `virtio,mmio` 子节点，不分配堆内存。
    pub fn parse(fdt: &FdtBlob<'_>) -> Result<Self, FdtVirtioInfoError> {
        let mut info = Self {
            regions: [FdtDeviceRegisterRegion { start: 0, size: 0 }; MAX_VIRTIO_MMIO_DEVICES],
            region_count: 0,
        };
        let mut found_soc = false;
        let mut soc = None;
        let mut transport = None;
        let mut structure = fdt.structure();

        loop {
            let event = structure
                .next_event()
                .map_err(FdtVirtioInfoError::InvalidStructure)?;
            match event {
                Some(FdtStructureEvent::BeginNode { name, depth: 1 }) if is_soc_node(name) => {
                    found_soc = true;
                    soc = Some(SocNode::new());
                }
                Some(FdtStructureEvent::BeginNode { depth: 2, .. }) if soc.is_some() => {
                    transport = Some(TransportNode::new());
                }
                Some(FdtStructureEvent::Property {
                    name,
                    value,
                    depth: 1,
                }) => {
                    if let Some(soc) = soc.as_mut() {
                        soc.record_property(name, value)?;
                    }
                }
                Some(FdtStructureEvent::Property {
                    name,
                    value,
                    depth: 2,
                }) => {
                    if let Some(transport) = transport.as_mut() {
                        transport.record_property(name, value);
                    }
                }
                Some(FdtStructureEvent::EndNode { depth: 2 }) => {
                    let Some(transport) = transport.take() else {
                        continue;
                    };
                    if !transport.is_enabled_virtio_mmio() {
                        continue;
                    }
                    let region = decode_region(
                        transport.reg.ok_or(FdtVirtioInfoError::MissingReg)?,
                        soc.as_ref().ok_or(FdtVirtioInfoError::MissingSocNode)?,
                    )?;
                    let Some(slot) = info.regions.get_mut(info.region_count) else {
                        return Err(FdtVirtioInfoError::TooManyVirtioMmioDevices);
                    };
                    *slot = region;
                    info.region_count += 1;
                }
                Some(FdtStructureEvent::EndNode { depth: 1 }) => soc = None,
                Some(FdtStructureEvent::End) | None => break,
                Some(_) => {}
            }
        }

        if !found_soc {
            return Err(FdtVirtioInfoError::MissingSocNode);
        }
        if info.region_count == 0 {
            return Err(FdtVirtioInfoError::MissingVirtioMmioDevice);
        }
        Ok(info)
    }

    /// 返回已解析的 VirtIO MMIO transport 范围。
    pub fn regions(&self) -> &[FdtDeviceRegisterRegion] {
        &self.regions[..self.region_count]
    }
}

struct SocNode {
    address_cells: u32,
    size_cells: u32,
    has_identity_ranges: bool,
}

impl SocNode {
    const fn new() -> Self {
        Self {
            address_cells: DEFAULT_ADDRESS_CELLS,
            size_cells: DEFAULT_SIZE_CELLS,
            has_identity_ranges: false,
        }
    }

    fn record_property(&mut self, name: &str, value: &[u8]) -> Result<(), FdtVirtioInfoError> {
        match name {
            "#address-cells" => self.address_cells = read_cell(value)?,
            "#size-cells" => self.size_cells = read_cell(value)?,
            "ranges" => self.has_identity_ranges = value.is_empty(),
            _ => {}
        }
        Ok(())
    }
}

struct TransportNode<'a> {
    compatible: bool,
    enabled: bool,
    reg: Option<&'a [u8]>,
}

impl<'a> TransportNode<'a> {
    const fn new() -> Self {
        Self {
            compatible: false,
            enabled: true,
            reg: None,
        }
    }

    fn record_property(&mut self, name: &str, value: &'a [u8]) {
        match name {
            "compatible" => {
                self.compatible = value
                    .split(|byte| *byte == 0)
                    .any(|compatible| compatible == b"virtio,mmio");
            }
            "status" => self.enabled = value == b"okay\0" || value == b"ok\0",
            "reg" => self.reg = Some(value),
            _ => {}
        }
    }

    const fn is_enabled_virtio_mmio(&self) -> bool {
        self.compatible && self.enabled
    }
}

fn is_soc_node(name: &str) -> bool {
    name == "soc" || name.starts_with("soc@")
}

fn decode_region(reg: &[u8], soc: &SocNode) -> Result<FdtDeviceRegisterRegion, FdtVirtioInfoError> {
    if !soc.has_identity_ranges {
        return Err(FdtVirtioInfoError::UnsupportedSocAddressTranslation);
    }
    let tuple_size = soc
        .address_cells
        .checked_add(soc.size_cells)
        .and_then(|cells| usize::try_from(cells).ok())
        .and_then(|cells| cells.checked_mul(4))
        .ok_or(FdtVirtioInfoError::InvalidRegLength)?;
    if soc.address_cells == 0
        || soc.address_cells > 2
        || soc.size_cells == 0
        || soc.size_cells > 2
        || reg.len() < tuple_size
        || reg.len() % tuple_size != 0
    {
        return Err(FdtVirtioInfoError::InvalidRegLength);
    }

    let address_size = soc.address_cells as usize * 4;
    let start = decode_cells(&reg[..address_size])?;
    let size = decode_cells(&reg[address_size..tuple_size])?;
    if start == 0 || size < MINIMUM_TRANSPORT_SIZE || start.checked_add(size).is_none() {
        return Err(FdtVirtioInfoError::InvalidRegisterRegion);
    }
    Ok(FdtDeviceRegisterRegion { start, size })
}

fn read_cell(value: &[u8]) -> Result<u32, FdtVirtioInfoError> {
    let bytes: [u8; 4] = value
        .try_into()
        .map_err(|_| FdtVirtioInfoError::UnsupportedCellCount)?;
    let cells = u32::from_be_bytes(bytes);
    if (1..=2).contains(&cells) {
        Ok(cells)
    } else {
        Err(FdtVirtioInfoError::UnsupportedCellCount)
    }
}

fn decode_cells(bytes: &[u8]) -> Result<u64, FdtVirtioInfoError> {
    match bytes.len() {
        4 => Ok(u32::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| FdtVirtioInfoError::InvalidRegLength)?,
        ) as u64),
        8 => Ok(u64::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| FdtVirtioInfoError::InvalidRegLength)?,
        )),
        _ => Err(FdtVirtioInfoError::InvalidRegLength),
    }
}
