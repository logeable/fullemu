//! 定义驱动共享的数据接口，并组织具体硬件驱动。

pub(crate) mod uart16550;
pub(crate) mod virtio;

/// 描述设备寄存器所在的物理地址范围。
#[derive(Clone, Copy)]
pub(crate) struct MmioRegion {
    /// MMIO 寄存器区域的物理起始地址。
    pub(crate) start: usize,
    /// MMIO 寄存器区域的字节长度。
    pub(crate) size: usize,
}

/// 字节型控制台驱动需要提供的最小收发能力。
pub(crate) trait ConsoleDevice {
    /// 向设备发送一个原始字节，不负责换行转换。
    fn write_byte(&self, byte: u8);

    /// 若设备已有输入字节则读取，否则立即返回。
    fn try_read_byte(&self) -> Option<u8>;

    /// 返回设备寄存器所在的物理地址范围。
    fn mmio_region(&self) -> MmioRegion;
}
