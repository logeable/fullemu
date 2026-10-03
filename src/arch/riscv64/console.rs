//! 通过轮询方式访问 QEMU virt 的第一个 NS16550 兼容 UART。
//!
//! 当前只适配 QEMU `virt`，因此暂时固定 UART 地址。支持其他平台前，必须从
//! OpenSBI 传入的 FDT 中发现控制台设备，不能继续依赖此常量。

use core::fmt;

const UART_BASE: usize = 0x1000_0000;
const UART_LINE_STATUS: usize = UART_BASE + 5;
// 当前 CPU 饥饿基线不读取串口；该状态位供后续交互实验复用。
#[allow(dead_code)]
const RECEIVER_DATA_READY: u8 = 1 << 0;
const TRANSMITTER_EMPTY: u8 = 1 << 5;

/// 可供 `core::fmt` 使用的串口写入器。
struct UartWriter;

impl fmt::Write for UartWriter {
    /// 通过轮询方式向 QEMU 串口输出 UTF-8 字符串。
    fn write_str(&mut self, message: &str) -> fmt::Result {
        for byte in message.bytes() {
            write_byte(byte);
        }
        Ok(())
    }
}

/// 使用 Rust 核心库的格式化能力输出内容，不申请堆内存。
pub fn write_fmt(arguments: fmt::Arguments<'_>) {
    // 安全性：格式化写入器只通过现有 UART MMIO 输出字节，不解引用调用方数据指针。
    let _ = fmt::Write::write_fmt(&mut UartWriter, arguments);
}

/// 阻塞等待并读取一个串口输入字节。
///
/// 当前通过轮询接收状态寄存器等待数据，不依赖中断或调度器。
// 当前 CPU 饥饿基线不处理输入，保留此接口供后续交互实验使用。
#[allow(dead_code)]
pub fn read_byte() -> u8 {
    loop {
        // 安全性：UART 状态寄存器和接收数据寄存器位于 QEMU `virt` 约定的 MMIO 地址；
        // 只有状态寄存器报告接收数据就绪后，才读取接收数据寄存器。
        unsafe {
            if core::ptr::read_volatile(UART_LINE_STATUS as *const u8) & RECEIVER_DATA_READY != 0 {
                return core::ptr::read_volatile(UART_BASE as *const u8);
            }
        }
        core::hint::spin_loop();
    }
}

/// 通过轮询方式输出一个字节；换行字节会转换为终端常用的 CRLF。
pub fn write_byte(byte: u8) {
    // 串口终端使用 CRLF 作为换行符。
    if byte == b'\n' {
        write_raw_byte(b'\r');
    }
    if byte == b'\r' {
        write_byte(b'\n');
    }
    write_raw_byte(byte);
}

fn write_raw_byte(byte: u8) {
    // 安全性：本阶段的平台约定将 QEMU `virt` 的 UART0 固定在 0x1000_0000。
    // 该设备使用 MMIO，因此必须进行易失性访问。写入一个字节前，先轮询状态
    // 寄存器，直到发送保持寄存器为空。新增平台前，必须改为通过 FDT 发现设备，
    // 并将控制台实现放入对应的平台模块。
    unsafe {
        while core::ptr::read_volatile(UART_LINE_STATUS as *const u8) & TRANSMITTER_EMPTY == 0 {}
        core::ptr::write_volatile(UART_BASE as *mut u8, byte);
    }
}
