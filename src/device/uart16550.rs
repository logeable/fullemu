//! 通过轮询访问 QEMU `virt` 上的 NS16550 兼容 UART。
//!
//! UART0 当前固定映射在 `0x1000_0000`。支持其他平台前，应根据设备树发现设备并提供对应实现。

use super::ConsoleDevice;

const UART_BASE: usize = 0x1000_0000;
const UART_REGISTER_SIZE: usize = 8;
const UART_LINE_STATUS: usize = UART_BASE + 5;
const RECEIVER_DATA_READY: u8 = 1 << 0;
const TRANSMITTER_EMPTY: u8 = 1 << 5;

pub(super) struct Uart16550;

impl ConsoleDevice for Uart16550 {
    fn write_byte(&self, byte: u8) {
        // 安全性：当前 QEMU `virt` 平台约定 UART0 位于该 MMIO 地址；发送前轮询状态，确保保持寄存器可写。
        unsafe {
            while core::ptr::read_volatile(UART_LINE_STATUS as *const u8) & TRANSMITTER_EMPTY == 0 {
            }
            core::ptr::write_volatile(UART_BASE as *mut u8, byte);
        }
    }

    fn try_read_byte(&self) -> Option<u8> {
        // 安全性：UART 状态寄存器和接收数据寄存器位于 QEMU `virt` 约定的 MMIO 地址；
        // 只有状态寄存器报告接收数据就绪后，才读取接收数据寄存器。
        unsafe {
            if core::ptr::read_volatile(UART_LINE_STATUS as *const u8) & RECEIVER_DATA_READY != 0 {
                Some(core::ptr::read_volatile(UART_BASE as *const u8))
            } else {
                None
            }
        }
    }

    fn mmio_region(&self) -> super::MmioRegion {
        super::MmioRegion {
            start: UART_BASE,
            size: UART_REGISTER_SIZE,
        }
    }
}
