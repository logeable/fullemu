//! 串接平台启动信息读取、内核子系统初始化和用户进程入口选择。

use core::fmt;

use fullemu::device_tree::fdt::{FdtBlob, FdtHeader, FdtStructureEvent};
use fullemu::device_tree::memory_info::FdtMemoryInfo;

use super::frame_allocator::FrameError;
use super::heap::HeapError;

/// 描述启动自检失败的子系统。
enum StartupCheckError {
    HeapAllocator(HeapError),
    GlobalAllocator(HeapError),
    PhysicalFrameAllocator(FrameError),
}

impl StartupCheckError {
    fn description(&self) -> &'static str {
        match self {
            Self::HeapAllocator(error) => error.description(),
            Self::GlobalAllocator(error) => error.description(),
            Self::PhysicalFrameAllocator(error) => error.description(),
        }
    }
}

extern "C" {
    static __boot_virtual_start: u8;
    static __boot_virtual_end: u8;
    static __boot_bss_virtual_start: u8;
    static __boot_bss_virtual_end: u8;
    static __boot_stack_virtual_start: u8;
    static __boot_stack_virtual_end: u8;
    static __kernel_start: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __data_end: u8;
    static __bss_start: u8;
    static __bss_data_end: u8;
    static __bss_end: u8;
}

/// 按固定顺序初始化内核，并根据构建配置启动用户进程。
pub(crate) fn run(hart_id: usize, device_tree_physical_address: usize) -> ! {
    crate::kernel::clock::initialize();

    let Some(device_tree_virtual_address) =
        super::memory::address::physical_to_kernel_virtual(device_tree_physical_address)
    else {
        panic!("FDT 物理地址无法转换为内核高半区地址");
    };

    // 安全性：OpenSBI 提供 FDT 物理地址，且启动页表将 QEMU RAM 映射到此高半区别名；解析器只读固定头部。
    let fdt_header =
        match unsafe { FdtHeader::read_from_ptr(device_tree_virtual_address as *const u8) } {
            Ok(header) => header,
            Err(error) => {
                crate::klog_error!("FDT 头部无效：{}", error.description());
                panic!("FDT 头部无效");
            }
        };

    // 安全性：高半区 FDT 别名可读，且头部已验证总长度至少覆盖所有声明区块。
    let fdt_blob = unsafe {
        core::slice::from_raw_parts(
            device_tree_virtual_address as *const u8,
            fdt_header.total_size as usize,
        )
    };
    let fdt = match FdtBlob::parse(fdt_blob) {
        Ok(fdt) => fdt,
        Err(error) => report_fdt_blob_error(error),
    };
    let console_info = match fullemu::device_tree::console_info::FdtConsoleInfo::parse(&fdt) {
        Ok(console_info) => console_info,
        Err(error) => report_console_info_error(error),
    };
    let uart_region = console_info.register_region();
    let uart_start = match usize::try_from(uart_region.start) {
        Ok(start) => start,
        Err(_) => panic!("FDT UART 地址超出当前平台可寻址范围"),
    };
    let uart_size = match usize::try_from(uart_region.size) {
        Ok(size) => size,
        Err(_) => panic!("FDT UART 寄存器范围超出当前平台可寻址范围"),
    };
    crate::device::console::initialize(crate::device::MmioRegion {
        start: uart_start,
        size: uart_size,
    });

    log_kernel_sections();
    crate::klog_info!("fullemu 已在 QEMU virt (RISC-V) 启动");
    crate::klog_info!("启动 hart：{hart_id:#018x}");
    crate::klog_info!("设备树物理地址：{device_tree_physical_address:#018x}");
    crate::klog_info!(
        "FDT UART：寄存器范围={:#018x}..{:#018x}",
        uart_region.start,
        uart_region.start + uart_region.size
    );
    crate::klog_debug!(
        "FDT 头部：版本={}，总长度={} 字节，结构区=偏移 {:#010x}/{} 字节，字符串区=偏移 {:#010x}/{} 字节",
        fdt_header.version,
        fdt_header.total_size,
        fdt_header.structure_offset,
        fdt_header.structure_size,
        fdt_header.strings_offset,
        fdt_header.strings_size
    );

    let memory_info = match FdtMemoryInfo::parse(&fdt, device_tree_physical_address as u64) {
        Ok(memory_info) => memory_info,
        Err(error) => report_memory_info_error(error),
    };

    for region in memory_info.memory_regions() {
        crate::klog_debug!(
            "FDT 物理内存区域：起始地址={:#018x}，长度={:#018x}",
            region.start,
            region.size
        );
    }

    for region in memory_info.memory_reservations() {
        crate::klog_debug!(
            "FDT 固件保留区域：起始地址={:#018x}，长度={:#018x}",
            region.start,
            region.size
        );
    }
    let device_tree_region = memory_info.device_tree_region();
    crate::klog_debug!(
        "FDT 占用区域：起始地址={:#018x}，长度={:#018x}",
        device_tree_region.start,
        device_tree_region.size
    );

    if let Err(error) = crate::kernel::frame_allocator::initialize(&memory_info) {
        crate::klog_error!("物理页帧分配器初始化失败：{}", error.description());
        panic!("物理页帧分配器初始化失败");
    }
    if let Err(error) = crate::kernel::heap::initialize() {
        crate::klog_error!("内核堆初始化失败：{}", error.description());
        panic!("内核堆初始化失败");
    }
    if let Err(error) = run_startup_checks() {
        crate::klog_error!("内核启动自检失败：{}", error.description());
        panic!("内核启动自检失败");
    }

    if crate::kernel::logging::is_enabled(crate::kernel::logging::Level::Trace) {
        let mut structure = fdt.structure();

        loop {
            match structure.next_event() {
                Ok(Some(FdtStructureEvent::BeginNode { name, depth })) => {
                    crate::klog_trace!("FDT 节点：深度={depth}，名称={name}");
                }
                Ok(Some(FdtStructureEvent::Property { name, value, depth })) => {
                    crate::klog_trace!(
                        "FDT 属性：深度={depth}，名称={name}，长度={} 字节，原始值={}",
                        value.len(),
                        HexBytes(value)
                    );
                }
                Ok(Some(FdtStructureEvent::End)) => break,
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(error) => report_fdt_structure_error(error),
            }
        }
    }

    match env!("FULLEMU_BOOT_MODE") {
        "shell" => crate::kernel::process::run_boot_program(),
        "batch" => crate::kernel::process::run_batch_programs(),
        _ => unreachable!("build.rs 已验证启动模式"),
    }
}

/// 以连续十六进制字节显示 FDT 属性值，不分配临时缓冲区。
struct HexBytes<'a>(&'a [u8]);

impl fmt::Display for HexBytes<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("0x")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// 运行启动自检；调用前必须初始化内核堆和物理页帧分配器。
fn run_startup_checks() -> Result<(), StartupCheckError> {
    super::heap::check_allocator().map_err(StartupCheckError::HeapAllocator)?;
    super::heap::check_global_allocator().map_err(StartupCheckError::GlobalAllocator)?;
    super::frame_allocator::check_allocation()
        .map_err(StartupCheckError::PhysicalFrameAllocator)?;
    Ok(())
}

/// 输出链接器定义的内核映像和各段范围；结束地址采用不包含式。
fn log_kernel_sections() {
    log_kernel_range(
        "启动区",
        core::ptr::addr_of!(__boot_virtual_start) as usize
            - super::memory::address::KERNEL_DIRECT_MAP_BASE,
        core::ptr::addr_of!(__boot_virtual_end) as usize
            - super::memory::address::KERNEL_DIRECT_MAP_BASE,
    );
    log_kernel_range(
        "启动区 BSS",
        core::ptr::addr_of!(__boot_bss_virtual_start) as usize
            - super::memory::address::KERNEL_DIRECT_MAP_BASE,
        core::ptr::addr_of!(__boot_bss_virtual_end) as usize
            - super::memory::address::KERNEL_DIRECT_MAP_BASE,
    );
    log_kernel_range(
        "启动栈",
        core::ptr::addr_of!(__boot_stack_virtual_start) as usize,
        core::ptr::addr_of!(__boot_stack_virtual_end) as usize,
    );
    log_kernel_range(
        "内核映像",
        core::ptr::addr_of!(__kernel_start) as usize,
        core::ptr::addr_of!(__bss_end) as usize,
    );
    log_kernel_range(
        ".text",
        core::ptr::addr_of!(__text_start) as usize,
        core::ptr::addr_of!(__text_end) as usize,
    );
    log_kernel_range(
        ".rodata",
        core::ptr::addr_of!(__rodata_start) as usize,
        core::ptr::addr_of!(__rodata_end) as usize,
    );
    log_kernel_range(
        ".data",
        core::ptr::addr_of!(__data_start) as usize,
        core::ptr::addr_of!(__data_end) as usize,
    );
    log_kernel_range(
        ".bss",
        core::ptr::addr_of!(__bss_start) as usize,
        core::ptr::addr_of!(__bss_data_end) as usize,
    );
}

fn log_kernel_range(name: &str, start: usize, end: usize) {
    crate::klog_info!(
        "内核范围 {}：{:#018x}..{:#018x}，大小={} 字节",
        name,
        start,
        end,
        end - start
    );
}

fn report_fdt_structure_error(error: fullemu::device_tree::fdt::FdtStructureError) -> ! {
    crate::klog_error!("FDT 结构块无效：{}", error.description());
    panic!("FDT 结构块无效");
}

fn report_fdt_blob_error(error: fullemu::device_tree::fdt::FdtBlobError) -> ! {
    crate::klog_error!("FDT 文件无效：{}", error.description());
    panic!("FDT 文件无效");
}

fn report_memory_info_error(error: fullemu::device_tree::memory_info::FdtMemoryInfoError) -> ! {
    crate::klog_error!("启动信息无效：{}", error.description());
    panic!("启动信息无效");
}

fn report_console_info_error(error: fullemu::device_tree::console_info::FdtConsoleInfoError) -> ! {
    crate::klog_error!("FDT 控制台信息无效：{}", error.description());
    panic!("FDT 控制台信息无效");
}
