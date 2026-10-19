//! 定义上层访问块设备时使用的扇区读写契约。

/// 一个逻辑扇区的字节数。
pub(crate) const SECTOR_SIZE: usize = 512;

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
    ) -> Result<(), Self::Error>;

    /// 将调用方提供的数据写入一个扇区。
    fn write_sector(&mut self, sector: u64, source: &[u8; SECTOR_SIZE]) -> Result<(), Self::Error>;

    /// 设备实现返回的具体 I/O 错误类型。
    type Error;
}
