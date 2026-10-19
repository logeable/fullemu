//! 封装现代 VirtIO MMIO transport 的通用寄存器和队列配置操作。

use core::ptr::{read_volatile, write_volatile};

use super::super::MmioRegion;

const VIRTIO_MAGIC: u32 = 0x7472_6976;
const VIRTIO_MMIO_VERSION_MODERN: u32 = 2;

const REG_MAGIC: usize = 0x000;
const REG_VERSION: usize = 0x004;
const REG_DEVICE_ID: usize = 0x008;
const REG_DEVICE_FEATURES: usize = 0x010;
const REG_DEVICE_FEATURES_SELECT: usize = 0x014;
const REG_DRIVER_FEATURES: usize = 0x020;
const REG_DRIVER_FEATURES_SELECT: usize = 0x024;
const REG_QUEUE_SELECT: usize = 0x030;
const REG_QUEUE_SIZE_MAX: usize = 0x034;
const REG_QUEUE_SIZE: usize = 0x038;
const REG_QUEUE_READY: usize = 0x044;
const REG_QUEUE_NOTIFY: usize = 0x050;
const REG_DEVICE_STATUS: usize = 0x070;
const REG_QUEUE_DESCRIPTOR: usize = 0x080;
const REG_QUEUE_DRIVER: usize = 0x090;
const REG_QUEUE_DEVICE: usize = 0x0a0;

const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

pub(super) const VIRTIO_F_VERSION_1: u32 = 1;

/// VirtIO MMIO transport 探测或寄存器访问失败。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum VirtioMmioError {
    /// FDT 提供的寄存器范围或对齐无效。
    InvalidRegisterRegion,
    /// 请求访问的寄存器超出 FDT 描述的 MMIO 范围。
    RegisterOutsideRegion,
    /// transport 拒绝设备状态转换。
    DeviceRejectedStatus,
}

/// 已验证的现代 VirtIO MMIO transport 寄存器窗口。
#[derive(Clone, Copy)]
pub(super) struct VirtioMmioTransport {
    region: MmioRegion,
}

impl VirtioMmioTransport {
    /// 探测现代 VirtIO MMIO transport；范围有效但不匹配时返回 `Ok(None)`。
    pub(super) fn probe(region: MmioRegion) -> Result<Option<Self>, VirtioMmioError> {
        if region.start % core::mem::align_of::<u32>() != 0
            || region.start.checked_add(region.size).is_none()
            || region.size < REG_DEVICE_STATUS + core::mem::size_of::<u32>()
        {
            return Err(VirtioMmioError::InvalidRegisterRegion);
        }

        let transport = Self { region };
        if transport.read_register(REG_MAGIC)? != VIRTIO_MAGIC
            || transport.read_register(REG_VERSION)? != VIRTIO_MMIO_VERSION_MODERN
        {
            return Ok(None);
        }
        Ok(Some(transport))
    }

    /// 返回 transport 声明的设备类型编号。
    pub(super) fn device_id(self) -> Result<u32, VirtioMmioError> {
        self.read_register(REG_DEVICE_ID)
    }

    /// 将设备状态复位到零。
    pub(super) fn reset(self) -> Result<(), VirtioMmioError> {
        self.write_register(REG_DEVICE_STATUS, 0)?;
        if self.read_register(REG_DEVICE_STATUS)? != 0 {
            return Err(VirtioMmioError::DeviceRejectedStatus);
        }
        Ok(())
    }

    /// 添加通用初始化状态位。
    pub(super) fn add_status_bits(self, status_bits: u32) -> Result<(), VirtioMmioError> {
        let status = self.read_register(REG_DEVICE_STATUS)?;
        self.write_register(REG_DEVICE_STATUS, status | status_bits)
    }

    /// 返回当前设备状态寄存器。
    pub(super) fn status(self) -> Result<u32, VirtioMmioError> {
        self.read_register(REG_DEVICE_STATUS)
    }

    /// 标记驱动已识别设备。
    pub(super) fn acknowledge(self) -> Result<(), VirtioMmioError> {
        self.add_status_bits(STATUS_ACKNOWLEDGE | STATUS_DRIVER)
    }

    /// 读取指定 32 位设备特性字。
    pub(super) fn device_features(self, selector: u32) -> Result<u32, VirtioMmioError> {
        self.write_register(REG_DEVICE_FEATURES_SELECT, selector)?;
        self.read_register(REG_DEVICE_FEATURES)
    }

    /// 写入驱动接受的指定 32 位特性字。
    pub(super) fn set_driver_features(
        self,
        selector: u32,
        features: u32,
    ) -> Result<(), VirtioMmioError> {
        self.write_register(REG_DRIVER_FEATURES_SELECT, selector)?;
        self.write_register(REG_DRIVER_FEATURES, features)
    }

    /// 通知设备驱动已提交特性协商结果，并返回设备是否接受。
    pub(super) fn confirm_features(self) -> Result<bool, VirtioMmioError> {
        self.add_status_bits(STATUS_FEATURES_OK)?;
        Ok(self.status()? & STATUS_FEATURES_OK != 0)
    }

    /// 选择 virtqueue 编号。
    pub(super) fn select_queue(self, queue_index: u32) -> Result<(), VirtioMmioError> {
        self.write_register(REG_QUEUE_SELECT, queue_index)
    }

    /// 返回当前选中队列的最大容量。
    pub(super) fn queue_size_max(self) -> Result<u32, VirtioMmioError> {
        self.read_register(REG_QUEUE_SIZE_MAX)
    }

    /// 设置当前选中队列的容量。
    pub(super) fn set_queue_size(self, queue_size: u32) -> Result<(), VirtioMmioError> {
        self.write_register(REG_QUEUE_SIZE, queue_size)
    }

    /// 设置当前选中队列的 descriptor、available 和 used 环物理地址。
    pub(super) fn set_queue_addresses(
        self,
        descriptor_address: u64,
        driver_address: u64,
        device_address: u64,
    ) -> Result<(), VirtioMmioError> {
        self.write_address(REG_QUEUE_DESCRIPTOR, descriptor_address)?;
        self.write_address(REG_QUEUE_DRIVER, driver_address)?;
        self.write_address(REG_QUEUE_DEVICE, device_address)
    }

    /// 通知设备当前选中队列已经准备好。
    pub(super) fn set_queue_ready(self) -> Result<(), VirtioMmioError> {
        self.write_register(REG_QUEUE_READY, 1)
    }

    /// 通知设备检查指定 virtqueue。
    pub(super) fn notify_queue(self, queue_index: u32) -> Result<(), VirtioMmioError> {
        self.write_register(REG_QUEUE_NOTIFY, queue_index)
    }

    /// 读取设备配置空间中的一个 32 位字。
    pub(super) fn read_config(self, offset: usize) -> Result<u32, VirtioMmioError> {
        self.read_register(offset)
    }

    /// 标记驱动已就绪，并返回设备是否接受该状态。
    pub(super) fn confirm_driver_ready(self) -> Result<bool, VirtioMmioError> {
        self.add_status_bits(STATUS_DRIVER_OK)?;
        Ok(self.status()? & STATUS_DRIVER_OK != 0)
    }

    fn read_register(self, offset: usize) -> Result<u32, VirtioMmioError> {
        self.validate_register(offset)?;
        let address = self.region.start + offset;
        // 安全性：探测时校验了 MMIO 起始地址对齐，且 validate_register 确保访问落在 FDT 范围内。
        Ok(unsafe { read_volatile(address as *const u32) })
    }

    fn write_register(self, offset: usize, value: u32) -> Result<(), VirtioMmioError> {
        self.validate_register(offset)?;
        let address = self.region.start + offset;
        // 安全性：探测时校验了 MMIO 起始地址对齐，且 validate_register 确保访问落在 FDT 范围内。
        unsafe { write_volatile(address as *mut u32, value) };
        Ok(())
    }

    fn write_address(self, low_offset: usize, address: u64) -> Result<(), VirtioMmioError> {
        let high_offset = low_offset
            .checked_add(core::mem::size_of::<u32>())
            .ok_or(VirtioMmioError::RegisterOutsideRegion)?;
        self.validate_register(low_offset)?;
        self.validate_register(high_offset)?;
        self.write_register(low_offset, address as u32)?;
        self.write_register(high_offset, (address >> 32) as u32)
    }

    fn validate_register(self, offset: usize) -> Result<(), VirtioMmioError> {
        if offset % core::mem::align_of::<u32>() != 0
            || offset
                .checked_add(core::mem::size_of::<u32>())
                .is_none_or(|end| end > self.region.size)
        {
            return Err(VirtioMmioError::RegisterOutsideRegion);
        }
        Ok(())
    }
}
