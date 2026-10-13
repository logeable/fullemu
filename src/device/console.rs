//! 为内核日志和用户态读写系统调用提供统一的字节控制台接口。

use core::fmt;

use super::{uart16550::Uart16550, ConsoleDevice, MmioRegion};

static CONSOLE_DEVICE: Uart16550 = Uart16550;

fn active_device() -> &'static dyn ConsoleDevice {
    &CONSOLE_DEVICE
}

/// 阻塞等待并读取一个控制台输入字节。
pub(crate) fn read_byte() -> u8 {
    loop {
        if let Some(byte) = active_device().try_read_byte() {
            return byte;
        }
        core::hint::spin_loop();
    }
}

/// 若控制台已有输入字节则读取，否则立即返回。
pub(crate) fn try_read_byte() -> Option<u8> {
    active_device().try_read_byte()
}

/// 返回当前控制台设备寄存器所在的物理地址范围。
pub(crate) fn mmio_region() -> MmioRegion {
    active_device().mmio_region()
}

/// 输出一个字节，并将换行符转换为终端常用的 CRLF。
pub(crate) fn write_byte(byte: u8) {
    write_byte_to(active_device(), byte);
}

fn write_byte_to(device: &dyn ConsoleDevice, byte: u8) {
    if byte == b'\n' {
        device.write_byte(b'\r');
    }
    device.write_byte(byte);
}

/// 使用 Rust 核心库的格式化能力输出内容，不申请堆内存。
pub(crate) fn write_fmt(arguments: fmt::Arguments<'_>) {
    let mut writer = ConsoleWriter(active_device());
    let _ = fmt::Write::write_fmt(&mut writer, arguments);
}

struct ConsoleWriter(&'static dyn ConsoleDevice);

impl fmt::Write for ConsoleWriter {
    fn write_str(&mut self, message: &str) -> fmt::Result {
        for byte in message.bytes() {
            write_byte_to(self.0, byte);
        }
        Ok(())
    }
}
