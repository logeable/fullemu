#![no_std]
#![no_main]

//! 从 OpenSBI 进入 Rust，通过 QEMU 串口报告启动信息并回显输入字节。

mod arch;

/// 使用当前平台的串口输出 Rust 格式化内容。
macro_rules! print {
    ($($argument:tt)*) => {{
        arch::riscv64::console::write_fmt(core::format_args!($($argument)*));
    }};
}

/// 输出一行格式化内容，并追加换行符。
macro_rules! println {
    () => {
        print!("\n");
    };
    ($($argument:tt)*) => {{
        print!($($argument)*);
        print!("\n");
    }};
}

use fullemu::boot::fdt::{FdtBlob, FdtHeader, FdtStructureEvent};
use fullemu::boot::info::BootInfo;

/// RISC-V 汇编入口完成栈和 `.bss` 初始化后调用此 Rust 入口。
/// OpenSBI 通过 `a0` 传入 hart ID，通过 `a1` 传入 DTB 地址；
/// 本阶段只运行一个 hart，并打印固件交接信息。
#[no_mangle]
pub extern "C" fn kernel_main(hart_id: usize, device_tree: usize) -> ! {
    println!("fullemu: booted on QEMU virt (RISC-V)");
    println!("hart: {hart_id:#018x}");
    println!("DTB:  {device_tree:#018x}");

    // 安全性：OpenSBI 启动约定保证 a1 指向可读的 FDT；解析器只读取固定的 40 字节头部。
    let fdt_header = match unsafe { FdtHeader::read_from_ptr(device_tree as *const u8) } {
        Ok(header) => header,
        Err(error) => {
            println!("fullemu: FDT 头部无效：{}", error.description());
            panic!("FDT 头部无效");
        }
    };

    println!("FDT version:      {}", fdt_header.version);
    println!("FDT total size:   {} bytes", fdt_header.total_size);
    println!(
        "FDT structure:    offset {:#010x}, size {} bytes",
        fdt_header.structure_offset, fdt_header.structure_size
    );
    println!(
        "FDT strings:      offset {:#010x}, size {} bytes",
        fdt_header.strings_offset, fdt_header.strings_size
    );

    // 安全性：OpenSBI 提供完整且可读的 FDT；头部已验证总长度至少覆盖所有声明区块。
    let fdt_blob = unsafe {
        core::slice::from_raw_parts(device_tree as *const u8, fdt_header.total_size as usize)
    };
    let fdt = match FdtBlob::parse(fdt_blob) {
        Ok(fdt) => fdt,
        Err(error) => report_fdt_blob_error(error),
    };
    let boot_info = match BootInfo::parse(&fdt) {
        Ok(boot_info) => boot_info,
        Err(error) => report_boot_info_error(error),
    };

    println!("物理内存范围：");
    for region in boot_info.memory_regions() {
        println!(
            "  起始地址：{:#018x}，长度：{:#018x}",
            region.start, region.size
        );
    }

    println!("固件保留范围：");
    for region in boot_info.reservations() {
        println!(
            "  起始地址：{:#018x}，长度：{:#018x}",
            region.address, region.size
        );
    }

    let mut structure = fdt.structure();

    println!("FDT 结构摘要：");
    loop {
        match structure.next_event() {
            Ok(Some(FdtStructureEvent::BeginNode { name, depth })) => {
                for _ in 0..depth {
                    print!("  ");
                }
                println!("- {name}");
            }
            Ok(Some(FdtStructureEvent::Property { name, value, depth })) => {
                for _ in 0..depth {
                    print!("  ");
                }
                println!("  {name}: {} bytes", value.len());
            }
            Ok(Some(FdtStructureEvent::End)) => break,
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(error) => report_fdt_structure_error(error),
        }
    }

    // 当前尚无调度器或 UART 中断。正常启动时由当前 hart 轮询 UART，等待用户输入。
    println!("串口字节回显已就绪，请输入字符：");
    loop {
        let byte = arch::riscv64::console::read_byte();
        arch::riscv64::console::write_byte(byte);
    }
}

fn report_fdt_structure_error(error: fullemu::boot::fdt::FdtStructureError) -> ! {
    println!("fullemu: FDT 结构块无效：{}", error.description());
    panic!("FDT 结构块无效");
}

fn report_fdt_blob_error(error: fullemu::boot::fdt::FdtBlobError) -> ! {
    println!("fullemu: FDT 文件无效：{}", error.description());
    panic!("FDT 文件无效");
}

fn report_boot_info_error(error: fullemu::boot::info::BootInfoError) -> ! {
    println!("fullemu: 启动信息无效：{}", error.description());
    panic!("启动信息无效");
}

/// 报告 S-mode 陷入时保存的 CSR，并停机，不尝试恢复被打断的执行流。
/// 汇编入口必须在有效的内核栈和已初始化 `gp` 上调用此函数；它不会返回。
#[no_mangle]
pub extern "C" fn supervisor_trap_handler(
    cause: usize,
    exception_pc: usize,
    trap_value: usize,
) -> ! {
    println!("\nfullemu: fatal S-mode trap");
    println!("scause: {cause:#018x}");
    println!("sepc:   {exception_pc:#018x}");
    println!("stval:  {trap_value:#018x}");

    loop {
        // 安全性：陷入被视为致命错误；WFI 让 hart 停机等待，不访问内存或改变特权级。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    println!("\nfullemu: kernel panic");
    println!("{info}");
    loop {
        // 安全性：与 `kernel_main` 中的空闲循环相同，此处等待是有效操作。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
