//! 校验并解释固件或引导程序提供的设备树信息。

pub mod console_info;
pub mod fdt;
pub mod memory_info;
pub mod virtio_info;

/// FDT 中由 `reg` 属性描述的 MMIO 设备寄存器范围。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FdtDeviceRegisterRegion {
    /// 设备寄存器的物理起始地址。
    pub start: u64,
    /// 设备寄存器范围的字节数。
    pub size: u64,
}
