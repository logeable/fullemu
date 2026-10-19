//! 通过轮询访问 QEMU `virt` 上的 NS16550 兼容 UART。
//!
//! 寄存器地址由启动阶段从 FDT 解析后设置；本驱动只负责 NS16550 寄存器访问。

use core::sync::atomic::{AtomicUsize, Ordering};

use super::{ConsoleDevice, MmioRegion};

const UART_LINE_STATUS_OFFSET: usize = 5;
const RECEIVER_DATA_READY: u8 = 1 << 0;
const TRANSMITTER_EMPTY: u8 = 1 << 5;

pub(crate) struct Uart16550 {
    register_base: AtomicUsize,
    register_size: AtomicUsize,
}

impl Uart16550 {
    pub(crate) const fn new() -> Self {
        Self {
            register_base: AtomicUsize::new(0),
            register_size: AtomicUsize::new(0),
        }
    }

    pub(crate) fn initialize(&self, region: MmioRegion) {
        self.register_size.store(region.size, Ordering::Relaxed);
        self.register_base.store(region.start, Ordering::Release);
    }

    fn register_base(&self) -> Option<usize> {
        let base = self.register_base.load(Ordering::Acquire);
        (base != 0).then_some(base)
    }
}

impl ConsoleDevice for Uart16550 {
    fn write_byte(&self, byte: u8) {
        let Some(register_base) = self.register_base() else {
            return;
        };
        let line_status = register_base + UART_LINE_STATUS_OFFSET;

        // 安全性：启动阶段已从 FDT 验证寄存器范围至少覆盖 8 字节；发送前轮询状态，确保保持寄存器可写。
        unsafe {
            while core::ptr::read_volatile(line_status as *const u8) & TRANSMITTER_EMPTY == 0 {}
            core::ptr::write_volatile(register_base as *mut u8, byte);
        }
    }

    fn try_read_byte(&self) -> Option<u8> {
        let register_base = self.register_base()?;
        let line_status = register_base + UART_LINE_STATUS_OFFSET;

        // 安全性：启动阶段已从 FDT 验证寄存器范围至少覆盖 8 字节；
        // 只有状态寄存器报告接收数据就绪后，才读取接收数据寄存器。
        unsafe {
            if core::ptr::read_volatile(line_status as *const u8) & RECEIVER_DATA_READY != 0 {
                Some(core::ptr::read_volatile(register_base as *const u8))
            } else {
                None
            }
        }
    }

    fn mmio_region(&self) -> super::MmioRegion {
        super::MmioRegion {
            start: self.register_base.load(Ordering::Acquire),
            size: self.register_size.load(Ordering::Relaxed),
        }
    }
}
