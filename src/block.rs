//! 定义上层访问块设备时使用的扇区读写契约。

/// 一个逻辑扇区的字节数。
pub(crate) const SECTOR_SIZE: usize = 512;

/// 块设备调用方可区分的基础 I/O 错误。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BlockDeviceError {
    /// 扇区编号超出设备容量。
    SectorOutOfRange,
    /// 设备未能完成读写请求。
    Io,
}

/// 为块设备消费者提供固定大小扇区读写能力。
///
/// 每次调用只处理一个 512 字节扇区；实现负责验证扇区范围并等待设备完成请求。
pub(crate) trait BlockDevice {
    /// 设备可访问的逻辑扇区数量。
    fn sector_count(&self) -> u64;

    /// 将一个扇区读入调用方提供的缓冲区。
    fn read_sector(
        &mut self,
        sector: u64,
        destination: &mut [u8; SECTOR_SIZE],
    ) -> Result<(), BlockDeviceError>;

    /// 将调用方提供的数据写入一个扇区。
    fn write_sector(
        &mut self,
        sector: u64,
        source: &[u8; SECTOR_SIZE],
    ) -> Result<(), BlockDeviceError>;
}

impl<T: BlockDevice + ?Sized> BlockDevice for alloc::boxed::Box<T> {
    fn sector_count(&self) -> u64 {
        (**self).sector_count()
    }

    fn read_sector(
        &mut self,
        sector: u64,
        destination: &mut [u8; SECTOR_SIZE],
    ) -> Result<(), BlockDeviceError> {
        (**self).read_sector(sector, destination)
    }

    fn write_sector(
        &mut self,
        sector: u64,
        source: &[u8; SECTOR_SIZE],
    ) -> Result<(), BlockDeviceError> {
        (**self).write_sector(sector, source)
    }
}
