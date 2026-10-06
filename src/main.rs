#![no_std]
#![no_main]

//! 从 OpenSBI 进入 Rust，通过 QEMU 串口报告启动信息并运行 U-mode 特权边界演示。

mod arch;

mod kernel;

use fullemu::boot::fdt::{FdtBlob, FdtHeader, FdtStructureEvent};
use fullemu::boot::info::BootInfo;

/// RISC-V 汇编入口完成栈和 `.bss` 初始化后调用此 Rust 入口。
/// OpenSBI 通过 `a0` 传入 hart ID，通过 `a1` 传入 DTB 地址；
/// 本阶段只运行一个 hart，并打印固件交接信息。
#[no_mangle]
pub extern "C" fn kernel_main(hart_id: usize, device_tree: usize) -> ! {
    crate::klog_info!("fullemu 已在 QEMU virt (RISC-V) 启动");
    crate::klog_info!("启动 hart：{hart_id:#018x}");
    crate::klog_info!("设备树地址：{device_tree:#018x}");

    // 安全性：OpenSBI 启动约定保证 a1 指向可读的 FDT；解析器只读取固定的 40 字节头部。
    let fdt_header = match unsafe { FdtHeader::read_from_ptr(device_tree as *const u8) } {
        Ok(header) => header,
        Err(error) => {
            crate::klog_error!("FDT 头部无效：{}", error.description());
            panic!("FDT 头部无效");
        }
    };

    crate::klog_debug!(
        "FDT 头部：版本={}，总长度={} 字节，结构区=偏移 {:#010x}/{} 字节，字符串区=偏移 {:#010x}/{} 字节",
        fdt_header.version,
        fdt_header.total_size,
        fdt_header.structure_offset,
        fdt_header.structure_size,
        fdt_header.strings_offset,
        fdt_header.strings_size
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

    for region in boot_info.memory_regions() {
        crate::klog_debug!(
            "FDT 物理内存区域：起始地址={:#018x}，长度={:#018x}",
            region.start,
            region.size
        );
    }

    for region in boot_info.reservations() {
        crate::klog_debug!(
            "FDT 固件保留区域：起始地址={:#018x}，长度={:#018x}",
            region.address,
            region.size
        );
    }

    let mut structure = fdt.structure();

    loop {
        match structure.next_event() {
            Ok(Some(FdtStructureEvent::BeginNode { name, depth })) => {
                crate::klog_trace!("FDT 节点：深度={depth}，名称={name}");
            }
            Ok(Some(FdtStructureEvent::Property { name, value, depth })) => {
                crate::klog_trace!(
                    "FDT 属性：深度={depth}，名称={name}，长度={} 字节",
                    value.len()
                );
            }
            Ok(Some(FdtStructureEvent::End)) => break,
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(error) => report_fdt_structure_error(error),
        }
    }

    kernel::user_mode::run_privilege_boundary_demonstration();
}

fn report_fdt_structure_error(error: fullemu::boot::fdt::FdtStructureError) -> ! {
    crate::klog_error!("FDT 结构块无效：{}", error.description());
    panic!("FDT 结构块无效");
}

fn report_fdt_blob_error(error: fullemu::boot::fdt::FdtBlobError) -> ! {
    crate::klog_error!("FDT 文件无效：{}", error.description());
    panic!("FDT 文件无效");
}

fn report_boot_info_error(error: fullemu::boot::info::BootInfoError) -> ! {
    crate::klog_error!("启动信息无效：{}", error.description());
    panic!("启动信息无效");
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    crate::klog_error!("内核 panic：{info}");
    loop {
        // 安全性：与 `kernel_main` 中的空闲循环相同，此处等待是有效操作。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
