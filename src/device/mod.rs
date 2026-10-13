//! 定义内核使用的设备接口，并组织当前平台的具体设备实现。

pub(crate) mod console;
mod uart16550;

/// 描述需要由页表映射的设备物理地址范围。
#[derive(Clone, Copy)]
pub(crate) struct MmioRegion {
    /// MMIO 寄存器区域的物理起始地址。
    pub(crate) start: usize,
    /// MMIO 寄存器区域的字节长度。
    pub(crate) size: usize,
}

/// 字节型控制台设备需要提供的最小收发能力。
pub(crate) trait ConsoleDevice {
    /// 向设备发送一个原始字节，不负责换行转换。
    fn write_byte(&self, byte: u8);

    /// 若设备已有输入字节则读取，否则立即返回。
    fn try_read_byte(&self) -> Option<u8>;

    /// 返回设备寄存器所在的物理地址范围。
    fn mmio_region(&self) -> MmioRegion;
}
