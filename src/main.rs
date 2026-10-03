#![no_std]
#![no_main]

//! 第一个启动阶段：从 OpenSBI 进入 Rust，并通过 QEMU 串口报告启动信息。

mod arch;

use fullemu::boot::fdt::{FdtBlob, FdtHeader, FdtStructureEvent};
use fullemu::boot::info::BootInfo;

/// RISC-V 汇编入口完成栈和 `.bss` 初始化后调用此 Rust 入口。
/// OpenSBI 通过 `a0` 传入 hart ID，通过 `a1` 传入 DTB 地址；
/// 本阶段只运行一个 hart，并打印固件交接信息。
#[no_mangle]
pub extern "C" fn kernel_main(hart_id: usize, device_tree: usize) -> ! {
    arch::riscv64::console::write_str("fullemu: booted on QEMU virt (RISC-V)\n");
    arch::riscv64::console::write_str("hart: ");
    arch::riscv64::console::write_hex(hart_id);
    arch::riscv64::console::write_str("\nDTB:  ");
    arch::riscv64::console::write_hex(device_tree);
    arch::riscv64::console::write_str("\n");

    // 安全性：OpenSBI 启动约定保证 a1 指向可读的 FDT；解析器只读取固定的 40 字节头部。
    let fdt_header = match unsafe { FdtHeader::read_from_ptr(device_tree as *const u8) } {
        Ok(header) => header,
        Err(error) => {
            arch::riscv64::console::write_str("fullemu: FDT 头部无效：");
            arch::riscv64::console::write_str(error.description());
            arch::riscv64::console::write_str("\n");
            panic!("FDT 头部无效");
        }
    };

    arch::riscv64::console::write_str("FDT version: ");
    arch::riscv64::console::write_hex(fdt_header.version as usize);
    arch::riscv64::console::write_str("\nFDT total size: ");
    arch::riscv64::console::write_hex(fdt_header.total_size as usize);
    arch::riscv64::console::write_str("\nFDT structure: offset ");
    arch::riscv64::console::write_hex(fdt_header.structure_offset as usize);
    arch::riscv64::console::write_str(", size ");
    arch::riscv64::console::write_hex(fdt_header.structure_size as usize);
    arch::riscv64::console::write_str("\nFDT strings: offset ");
    arch::riscv64::console::write_hex(fdt_header.strings_offset as usize);
    arch::riscv64::console::write_str(", size ");
    arch::riscv64::console::write_hex(fdt_header.strings_size as usize);
    arch::riscv64::console::write_str("\n");

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

    arch::riscv64::console::write_str("物理内存范围：\n");
    for region in boot_info.memory_regions() {
        arch::riscv64::console::write_str("  起始地址：");
        arch::riscv64::console::write_hex(region.start as usize);
        arch::riscv64::console::write_str("，长度：");
        arch::riscv64::console::write_hex(region.size as usize);
        arch::riscv64::console::write_str("\n");
    }

    arch::riscv64::console::write_str("固件保留范围：\n");
    for region in boot_info.reservations() {
        arch::riscv64::console::write_str("  起始地址：");
        arch::riscv64::console::write_hex(region.address as usize);
        arch::riscv64::console::write_str("，长度：");
        arch::riscv64::console::write_hex(region.size as usize);
        arch::riscv64::console::write_str("\n");
    }

    let mut structure = fdt.structure();

    arch::riscv64::console::write_str("FDT 结构摘要：\n");
    loop {
        match structure.next_event() {
            Ok(Some(FdtStructureEvent::BeginNode { name, depth })) => {
                for _ in 0..depth {
                    arch::riscv64::console::write_str("  ");
                }
                arch::riscv64::console::write_str("- ");
                arch::riscv64::console::write_str(name);
                arch::riscv64::console::write_str("\n");
            }
            Ok(Some(FdtStructureEvent::Property { name, value, depth })) => {
                for _ in 0..depth {
                    arch::riscv64::console::write_str("  ");
                }
                arch::riscv64::console::write_str("  ");
                arch::riscv64::console::write_str(name);
                arch::riscv64::console::write_str(": ");
                arch::riscv64::console::write_hex(value.len());
                arch::riscv64::console::write_str("\n");
            }
            Ok(Some(FdtStructureEvent::End)) => break,
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(error) => report_fdt_structure_error(error),
        }
    }

    #[cfg(feature = "trap-demo")]
    {
        arch::riscv64::console::write_str("trap-demo: 即将执行 ebreak\n");
        // 安全性：此功能有意触发断点异常；已安装的致命异常入口会报告状态并停机，
        // 不会尝试从异常指令返回。
        unsafe { core::arch::asm!("ebreak", options(noreturn)) };
    }

    // 当前尚无调度器或关机服务。正常启动时让 hart 保持运行，避免反复轮询设备
    // 或持续占用整个 CPU 核心。
    #[cfg(not(feature = "trap-demo"))]
    loop {
        // 安全性：WFI 只让当前 hart 等待中断，不访问内存，也不改变特权级。
        // 本阶段没有启用中断，因此可由宿主机直接停止 QEMU。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

fn report_fdt_structure_error(error: fullemu::boot::fdt::FdtStructureError) -> ! {
    arch::riscv64::console::write_str("fullemu: FDT 结构块无效：");
    arch::riscv64::console::write_str(error.description());
    arch::riscv64::console::write_str("\n");
    panic!("FDT 结构块无效");
}

fn report_fdt_blob_error(error: fullemu::boot::fdt::FdtBlobError) -> ! {
    arch::riscv64::console::write_str("fullemu: FDT 文件无效：");
    arch::riscv64::console::write_str(error.description());
    arch::riscv64::console::write_str("\n");
    panic!("FDT 文件无效");
}

fn report_boot_info_error(error: fullemu::boot::info::BootInfoError) -> ! {
    arch::riscv64::console::write_str("fullemu: 启动信息无效：");
    arch::riscv64::console::write_str(error.description());
    arch::riscv64::console::write_str("\n");
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
    arch::riscv64::console::write_str("\nfullemu: fatal S-mode trap\n");
    arch::riscv64::console::write_str("scause: ");
    arch::riscv64::console::write_hex(cause);
    arch::riscv64::console::write_str("\nsepc:   ");
    arch::riscv64::console::write_hex(exception_pc);
    arch::riscv64::console::write_str("\nstval:  ");
    arch::riscv64::console::write_hex(trap_value);
    arch::riscv64::console::write_str("\n");

    loop {
        // 安全性：陷入被视为致命错误；WFI 让 hart 停机等待，不访问内存或改变特权级。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    arch::riscv64::console::write_str("\nfullemu: kernel panic\n");
    loop {
        // 安全性：与 `kernel_main` 中的空闲循环相同，此处等待是有效操作。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
