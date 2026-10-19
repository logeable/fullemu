//! 从 FDT 中提取早期控制台所需的 UART 寄存器范围。
//!
//! 当前仅支持 QEMU `virt` 的 `/soc` 简单总线，以及 NS16550 兼容串口。
//! 解析结果不分配堆内存，也不依赖具体 UART 驱动。

use super::fdt::{FdtBlob, FdtStructureError, FdtStructureEvent};
use super::FdtDeviceRegisterRegion;

const DEFAULT_ADDRESS_CELLS: u32 = 2;
const DEFAULT_SIZE_CELLS: u32 = 1;
const MINIMUM_UART_REGISTER_SIZE: u64 = 8;

/// UART 节点无法转换为本阶段支持的控制台启动信息。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FdtConsoleInfoError {
    /// FDT 结构块不符合格式。
    InvalidStructure(FdtStructureError),
    /// `/soc` 节点不存在。
    MissingSocNode,
    /// `/soc` 没有声明空 `ranges`，当前实现无法转换其子总线地址。
    UnsupportedSocAddressTranslation,
    /// 没有找到受支持且启用的 NS16550 UART。
    MissingSupportedUart,
    /// 找到 UART 节点，但没有 `reg` 属性。
    MissingUartReg,
    /// 地址或长度 cell 数不是本阶段支持的 1 或 2。
    UnsupportedCellCount,
    /// UART `reg` 属性不包含完整的地址/长度项。
    InvalidRegLength,
    /// UART 寄存器范围为空、长度不足或地址溢出。
    InvalidRegisterRegion,
}

impl FdtConsoleInfoError {
    /// 返回适合启动诊断的错误说明。
    pub const fn description(self) -> &'static str {
        match self {
            Self::InvalidStructure(error) => error.description(),
            Self::MissingSocNode => "FDT 缺少 /soc 总线节点",
            Self::UnsupportedSocAddressTranslation => "FDT /soc ranges 不是空的恒等映射",
            Self::MissingSupportedUart => "FDT 中没有启用的 NS16550 UART",
            Self::MissingUartReg => "FDT UART 节点缺少 reg 属性",
            Self::UnsupportedCellCount => "UART 地址或长度 cell 数不受支持",
            Self::InvalidRegLength => "FDT UART reg 属性长度无效",
            Self::InvalidRegisterRegion => "FDT UART 寄存器范围无效",
        }
    }
}

/// 从 FDT 中提取出的早期控制台设备信息。
pub struct FdtConsoleInfo {
    register_region: FdtDeviceRegisterRegion,
}

impl FdtConsoleInfo {
    /// 查找 `/soc` 下第一个启用的 NS16550 兼容 UART，并解析其 `reg` 范围。
    ///
    /// 当前支持 `/soc/ranges` 为空的情况，即子总线地址与物理地址相同；不处理
    /// `stdout-path`、地址窗口转换或其他串口控制器 binding。
    pub fn parse(fdt: &FdtBlob<'_>) -> Result<Self, FdtConsoleInfoError> {
        let mut soc = None;
        let mut uart = None;
        let mut found_soc = false;
        let mut register_region = None;
        let mut structure = fdt.structure();

        loop {
            let event = structure
                .next_event()
                .map_err(FdtConsoleInfoError::InvalidStructure)?;
            match event {
                Some(FdtStructureEvent::BeginNode { name, depth: 1 }) if is_soc_node(name) => {
                    found_soc = true;
                    soc = Some(SocNode::new());
                }
                Some(FdtStructureEvent::BeginNode { depth: 2, .. }) if soc.is_some() => {
                    uart = Some(UartNode::new());
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
                    if let Some(uart) = uart.as_mut() {
                        uart.record_property(name, value);
                    }
                }
                Some(FdtStructureEvent::EndNode { depth: 2 }) => {
                    let Some(uart) = uart.take() else {
                        continue;
                    };
                    if !uart.is_supported_enabled_uart() {
                        continue;
                    }
                    let reg = uart.reg.ok_or(FdtConsoleInfoError::MissingUartReg)?;
                    let soc = soc.ok_or(FdtConsoleInfoError::MissingSocNode)?;
                    if !soc.has_identity_ranges {
                        return Err(FdtConsoleInfoError::UnsupportedSocAddressTranslation);
                    }
                    register_region =
                        Some(decode_uart_region(reg, soc.address_cells, soc.size_cells)?);
                    break;
                }
                Some(FdtStructureEvent::EndNode { depth: 1 }) => soc = None,
                Some(FdtStructureEvent::End) | None => break,
                Some(_) => {}
            }
        }

        if !found_soc {
            return Err(FdtConsoleInfoError::MissingSocNode);
        }
        let register_region = register_region.ok_or(FdtConsoleInfoError::MissingSupportedUart)?;
        Ok(Self { register_region })
    }

    /// 返回 UART 节点声明的物理寄存器范围。
    pub const fn register_region(&self) -> FdtDeviceRegisterRegion {
        self.register_region
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

    fn record_property(&mut self, name: &str, value: &[u8]) -> Result<(), FdtConsoleInfoError> {
        match name {
            "#address-cells" => self.address_cells = read_cell(value)?,
            "#size-cells" => self.size_cells = read_cell(value)?,
            "ranges" => self.has_identity_ranges = value.is_empty(),
            _ => {}
        }
        Ok(())
    }
}

struct UartNode<'a> {
    compatible: bool,
    enabled: bool,
    reg: Option<&'a [u8]>,
}

impl<'a> UartNode<'a> {
    const fn new() -> Self {
        Self {
            compatible: false,
            enabled: true,
            reg: None,
        }
    }

    fn record_property(&mut self, name: &str, value: &'a [u8]) {
        match name {
            "compatible" => self.compatible = has_supported_compatible(value),
            "status" => self.enabled = is_enabled(value),
            "reg" => self.reg = Some(value),
            _ => {}
        }
    }

    const fn is_supported_enabled_uart(&self) -> bool {
        self.compatible && self.enabled
    }
}

fn is_soc_node(name: &str) -> bool {
    name == "soc" || name.starts_with("soc@")
}

fn has_supported_compatible(value: &[u8]) -> bool {
    value
        .split(|byte| *byte == 0)
        .any(|compatible| compatible == b"ns16550a" || compatible == b"ns16550")
}

fn is_enabled(value: &[u8]) -> bool {
    value == b"okay\0" || value == b"ok\0"
}

fn read_cell(value: &[u8]) -> Result<u32, FdtConsoleInfoError> {
    let bytes: [u8; 4] = value
        .try_into()
        .map_err(|_| FdtConsoleInfoError::UnsupportedCellCount)?;
    let cells = u32::from_be_bytes(bytes);
    if (1..=2).contains(&cells) {
        Ok(cells)
    } else {
        Err(FdtConsoleInfoError::UnsupportedCellCount)
    }
}

fn decode_uart_region(
    reg: &[u8],
    address_cells: u32,
    size_cells: u32,
) -> Result<FdtDeviceRegisterRegion, FdtConsoleInfoError> {
    let tuple_size = address_cells
        .checked_add(size_cells)
        .and_then(|cells| usize::try_from(cells).ok())
        .and_then(|cells| cells.checked_mul(4))
        .ok_or(FdtConsoleInfoError::InvalidRegLength)?;
    if address_cells == 0
        || address_cells > 2
        || size_cells == 0
        || size_cells > 2
        || reg.len() < tuple_size
        || reg.len() % tuple_size != 0
    {
        return Err(FdtConsoleInfoError::InvalidRegLength);
    }

    let address_size = address_cells as usize * 4;
    let start = decode_cells(&reg[..address_size])?;
    let size = decode_cells(&reg[address_size..tuple_size])?;
    if start == 0 || size < MINIMUM_UART_REGISTER_SIZE || start.checked_add(size).is_none() {
        return Err(FdtConsoleInfoError::InvalidRegisterRegion);
    }
    Ok(FdtDeviceRegisterRegion { start, size })
}

fn decode_cells(bytes: &[u8]) -> Result<u64, FdtConsoleInfoError> {
    match bytes.len() {
        4 => Ok(u32::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| FdtConsoleInfoError::InvalidRegLength)?,
        ) as u64),
        8 => Ok(u64::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| FdtConsoleInfoError::InvalidRegLength)?,
        )),
        _ => Err(FdtConsoleInfoError::InvalidRegLength),
    }
}
