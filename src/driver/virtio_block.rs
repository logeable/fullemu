//! 使用 VirtIO MMIO split virtqueue 轮询读取 QEMU 提供的块设备。
//!
//! 当前驱动只支持单 hart 启动阶段的一次串行读取，不启用设备中断，也不管理并发请求。

use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicUsize, Ordering};

use super::MmioRegion;

pub(crate) const SECTOR_SIZE: usize = 512;
pub(crate) const TEST_SECTOR_SIGNATURE: &[u8] = b"FULLEMU-BLOCK-TEST\n";

const VIRTIO_MAGIC: u32 = 0x7472_6976;
const VIRTIO_MMIO_VERSION_MODERN: u32 = 2;
const VIRTIO_DEVICE_BLOCK: u32 = 2;
const VIRTIO_F_VERSION_1: u32 = 1;

const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

const BLOCK_REQUEST_READ: u32 = 0;
const DESCRIPTOR_NEXT: u16 = 1;
const DESCRIPTOR_DEVICE_WRITES: u16 = 2;
const QUEUE_SIZE: usize = 8;
const READ_TIMEOUT_ITERATIONS: usize = 100_000_000;

const MAGIC_VALUE: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const DEVICE_FEATURES: usize = 0x010;
const DEVICE_FEATURES_SELECT: usize = 0x014;
const DRIVER_FEATURES: usize = 0x020;
const DRIVER_FEATURES_SELECT: usize = 0x024;
const QUEUE_SELECT: usize = 0x030;
const QUEUE_SIZE_MAX: usize = 0x034;
const QUEUE_SIZE_REGISTER: usize = 0x038;
const QUEUE_READY: usize = 0x044;
const QUEUE_NOTIFY: usize = 0x050;
const DEVICE_STATUS: usize = 0x070;
const QUEUE_DESCRIPTOR_LOW: usize = 0x080;
const QUEUE_DRIVER_LOW: usize = 0x090;
const QUEUE_DEVICE_LOW: usize = 0x0a0;
const CONFIG_GENERATION: usize = 0x0fc;
const CONFIG_CAPACITY: usize = 0x100;

static DEVICE_BASE: AtomicUsize = AtomicUsize::new(0);
static DEVICE_CAPACITY_SECTORS: AtomicUsize = AtomicUsize::new(0);
static REQUEST_PHYSICAL_ADDRESS: AtomicUsize = AtomicUsize::new(0);

/// VirtIO 块设备初始化或请求处理失败。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VirtioBlockError {
    /// MMIO 区域未覆盖本驱动访问的寄存器。
    InvalidRegisterRegion,
    /// VirtIO 设备状态寄存器拒绝初始化。
    DeviceRejectedInitialization,
    /// 本设备没有提供所需的现代 VirtIO 接口特性。
    MissingVersionOneFeature,
    /// 设备没有提供足够大的请求队列。
    QueueTooSmall,
    /// 队列地址不能表示为设备使用的物理地址。
    InvalidDmaAddress,
    /// 块设备没有可读取的扇区。
    EmptyDevice,
    /// 块设备容量无法表示为当前平台的 `usize`。
    CapacityTooLarge,
    /// 扇区编号超出设备容量。
    SectorOutOfRange,
    /// 设备没有在轮询期限内完成请求。
    RequestTimedOut,
    /// 设备在请求完成项中返回了错误状态。
    DeviceReportedIoError,
    /// 使用项编号或长度与单扇区读请求不匹配。
    InvalidUsedEntry,
}

impl VirtioBlockError {
    /// 返回适合启动诊断的错误说明。
    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::InvalidRegisterRegion => "VirtIO MMIO 寄存器范围无效",
            Self::DeviceRejectedInitialization => "VirtIO 设备拒绝初始化状态",
            Self::MissingVersionOneFeature => "VirtIO 设备不支持现代版本特性",
            Self::QueueTooSmall => "VirtIO 请求队列容量不足",
            Self::InvalidDmaAddress => "VirtIO 队列 DMA 地址无效",
            Self::EmptyDevice => "VirtIO 块设备容量为空",
            Self::CapacityTooLarge => "VirtIO 块设备容量超出当前平台可表示范围",
            Self::SectorOutOfRange => "扇区编号超出块设备容量",
            Self::RequestTimedOut => "VirtIO 设备请求超时",
            Self::DeviceReportedIoError => "VirtIO 块设备报告读请求失败",
            Self::InvalidUsedEntry => "VirtIO used ring 返回项无效",
        }
    }
}

/// VirtIO split virtqueue 中的一个设备描述符。
#[derive(Clone, Copy)]
#[repr(C)]
struct Descriptor {
    address: u64,
    length: u32,
    flags: u16,
    next: u16,
}

impl Descriptor {
    const EMPTY: Self = Self {
        address: 0,
        length: 0,
        flags: 0,
        next: 0,
    };
}

#[repr(C)]
struct AvailableRing {
    flags: u16,
    index: u16,
    ring: [u16; QUEUE_SIZE],
}

#[derive(Clone, Copy)]
#[repr(C)]
struct UsedElement {
    id: u32,
    length: u32,
}

#[repr(C)]
struct UsedRing {
    flags: u16,
    index: u16,
    ring: [UsedElement; QUEUE_SIZE],
}

/// 队列各区域按 split virtqueue 要求分别自然对齐。
#[repr(C, align(4096))]
struct QueueMemory {
    descriptors: [Descriptor; QUEUE_SIZE],
    available: AvailableRing,
    used: UsedRing,
}

impl QueueMemory {
    const EMPTY: Self = Self {
        descriptors: [Descriptor::EMPTY; QUEUE_SIZE],
        available: AvailableRing {
            flags: 1,
            index: 0,
            ring: [0; QUEUE_SIZE],
        },
        used: UsedRing {
            flags: 0,
            index: 0,
            ring: [UsedElement { id: 0, length: 0 }; QUEUE_SIZE],
        },
    };
}

#[derive(Clone, Copy)]
#[repr(C)]
struct BlockRequestHeader {
    request_type: u32,
    reserved: u32,
    sector: u64,
}

#[repr(C, align(4096))]
struct RequestMemory {
    header: BlockRequestHeader,
    data: [u8; SECTOR_SIZE],
    status: u8,
}

impl RequestMemory {
    const EMPTY: Self = Self {
        header: BlockRequestHeader {
            request_type: BLOCK_REQUEST_READ,
            reserved: 0,
            sector: 0,
        },
        data: [0; SECTOR_SIZE],
        status: u8::MAX,
    };
}

static mut QUEUE: QueueMemory = QueueMemory::EMPTY;
static mut REQUEST: RequestMemory = RequestMemory::EMPTY;

/// 若该 MMIO 区域是块设备则初始化它；其他 VirtIO 设备返回 `Ok(false)`。
pub(crate) fn initialize(
    region: MmioRegion,
    queue_physical_address: usize,
    request_physical_address: usize,
) -> Result<bool, VirtioBlockError> {
    if region.start % core::mem::align_of::<u32>() != 0
        || region.size < CONFIG_CAPACITY + core::mem::size_of::<u64>()
    {
        return Err(VirtioBlockError::InvalidRegisterRegion);
    }
    if read_register(region.start, MAGIC_VALUE) != VIRTIO_MAGIC
        || read_register(region.start, VERSION) != VIRTIO_MMIO_VERSION_MODERN
        || read_register(region.start, DEVICE_ID) != VIRTIO_DEVICE_BLOCK
    {
        return Ok(false);
    }

    initialize_device(
        region.start,
        queue_physical_address,
        request_physical_address,
    )?;
    DEVICE_BASE.store(region.start, Ordering::Release);
    Ok(true)
}

fn initialize_device(
    base: usize,
    queue_physical_address: usize,
    request_physical_address: usize,
) -> Result<(), VirtioBlockError> {
    write_register(base, DEVICE_STATUS, 0);
    if read_register(base, DEVICE_STATUS) != 0 {
        return Err(VirtioBlockError::DeviceRejectedInitialization);
    }
    set_device_status(base, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

    write_register(base, DEVICE_FEATURES_SELECT, 1);
    let high_features = read_register(base, DEVICE_FEATURES);
    if high_features & VIRTIO_F_VERSION_1 == 0 {
        return Err(VirtioBlockError::MissingVersionOneFeature);
    }

    write_register(base, DRIVER_FEATURES_SELECT, 0);
    write_register(base, DRIVER_FEATURES, 0);
    write_register(base, DRIVER_FEATURES_SELECT, 1);
    write_register(base, DRIVER_FEATURES, VIRTIO_F_VERSION_1);
    set_device_status(base, STATUS_FEATURES_OK);
    if read_register(base, DEVICE_STATUS) & STATUS_FEATURES_OK == 0 {
        return Err(VirtioBlockError::DeviceRejectedInitialization);
    }

    write_register(base, QUEUE_SELECT, 0);
    let maximum_queue_size = read_register(base, QUEUE_SIZE_MAX);
    if maximum_queue_size < QUEUE_SIZE as u32 {
        return Err(VirtioBlockError::QueueTooSmall);
    }
    write_register(base, QUEUE_SIZE_REGISTER, QUEUE_SIZE as u32);

    // 静态队列在链接时按页对齐；这里仅获取原始地址并计算字段位置。
    let queue_pointer = addr_of_mut!(QUEUE);
    let queue_virtual_address = queue_pointer as usize;
    // 安全性：静态队列对象有效且字段布局由 repr(C) 固定；这里只计算字段地址，不读取内容。
    let (available_virtual_address, used_virtual_address, descriptor_virtual_address) = unsafe {
        (
            addr_of_mut!((*queue_pointer).available) as usize,
            addr_of_mut!((*queue_pointer).used) as usize,
            addr_of_mut!((*queue_pointer).descriptors) as usize,
        )
    };
    if !is_aligned(descriptor_virtual_address, 16)
        || !is_aligned(available_virtual_address, 2)
        || !is_aligned(used_virtual_address, 4)
        || !is_aligned(queue_virtual_address, 4096)
        || !is_aligned(queue_physical_address, 4096)
        || !is_aligned(request_virtual_address(), 4096)
        || !is_aligned(request_physical_address, 4096)
    {
        return Err(VirtioBlockError::InvalidDmaAddress);
    }

    let descriptor_address = queue_physical_address
        .checked_add(core::mem::offset_of!(QueueMemory, descriptors))
        .ok_or(VirtioBlockError::InvalidDmaAddress)?;
    let available_address = queue_physical_address
        .checked_add(core::mem::offset_of!(QueueMemory, available))
        .ok_or(VirtioBlockError::InvalidDmaAddress)?;
    let used_address = queue_physical_address
        .checked_add(core::mem::offset_of!(QueueMemory, used))
        .ok_or(VirtioBlockError::InvalidDmaAddress)?;
    write_address(base, QUEUE_DESCRIPTOR_LOW, descriptor_address as u64);
    write_address(base, QUEUE_DRIVER_LOW, available_address as u64);
    write_address(base, QUEUE_DEVICE_LOW, used_address as u64);
    write_register(base, QUEUE_READY, 1);

    let generation_before = read_register(base, CONFIG_GENERATION);
    let capacity_low = read_register(base, CONFIG_CAPACITY) as u64;
    let capacity_high = read_register(base, CONFIG_CAPACITY + 4) as u64;
    let generation_after = read_register(base, CONFIG_GENERATION);
    if generation_before != generation_after {
        return Err(VirtioBlockError::InvalidRegisterRegion);
    }
    let capacity = capacity_low | (capacity_high << 32);
    if capacity == 0 {
        return Err(VirtioBlockError::EmptyDevice);
    }
    let capacity = usize::try_from(capacity).map_err(|_| VirtioBlockError::CapacityTooLarge)?;
    DEVICE_CAPACITY_SECTORS.store(capacity, Ordering::Release);
    REQUEST_PHYSICAL_ADDRESS.store(request_physical_address, Ordering::Release);

    set_device_status(base, STATUS_DRIVER_OK);
    if read_register(base, DEVICE_STATUS) & STATUS_DRIVER_OK == 0 {
        return Err(VirtioBlockError::DeviceRejectedInitialization);
    }
    Ok(())
}

/// 读取一个 512 字节逻辑扇区；本实验只由启动阶段串行调用。
pub(crate) fn read_sector(
    sector: u64,
    destination: &mut [u8; SECTOR_SIZE],
) -> Result<(), VirtioBlockError> {
    let base = DEVICE_BASE.load(Ordering::Acquire);
    let request_physical_address = REQUEST_PHYSICAL_ADDRESS.load(Ordering::Acquire);
    if base == 0 {
        return Err(VirtioBlockError::InvalidRegisterRegion);
    }
    if sector >= DEVICE_CAPACITY_SECTORS.load(Ordering::Acquire) as u64 {
        return Err(VirtioBlockError::SectorOutOfRange);
    }

    // 安全性：本实验在单 hart 启动阶段串行发起一次请求；设备完成前不复用这些独占静态缓冲区。
    unsafe {
        let request = addr_of_mut!(REQUEST);
        (*request).header = BlockRequestHeader {
            request_type: BLOCK_REQUEST_READ,
            reserved: 0,
            sector,
        };
        (*request).status = u8::MAX;
        (*request).data.fill(0);

        let queue = addr_of_mut!(QUEUE);
        let descriptors = addr_of_mut!((*queue).descriptors).cast::<Descriptor>();
        descriptors.add(0).write(Descriptor {
            address: checked_dma_offset(
                request_physical_address,
                core::mem::offset_of!(RequestMemory, header),
            )?,
            length: core::mem::size_of::<BlockRequestHeader>() as u32,
            flags: DESCRIPTOR_NEXT,
            next: 1,
        });
        descriptors.add(1).write(Descriptor {
            address: checked_dma_offset(
                request_physical_address,
                core::mem::offset_of!(RequestMemory, data),
            )?,
            length: SECTOR_SIZE as u32,
            flags: DESCRIPTOR_NEXT | DESCRIPTOR_DEVICE_WRITES,
            next: 2,
        });
        descriptors.add(2).write(Descriptor {
            address: checked_dma_offset(
                request_physical_address,
                core::mem::offset_of!(RequestMemory, status),
            )?,
            length: 1,
            flags: DESCRIPTOR_DEVICE_WRITES,
            next: 0,
        });

        let available_ring_entry = addr_of_mut!((*queue).available.ring[0]);
        available_ring_entry.write_volatile(0);
        addr_of_mut!((*queue).available.index).write_volatile(1);
    }

    // 安全性：描述符链和可用环已完整写入 RAM；该 fence 保证 MMIO doorbell 前的 DMA 数据可见。
    unsafe { core::arch::asm!("fence w, o", options(nostack)) };
    write_register(base, QUEUE_NOTIFY, 0);

    let queue = addr_of_mut!(QUEUE);
    // 安全性：队列指针指向有效的静态对象；这里只计算 used index 字段地址供 volatile 读取。
    let used_index = unsafe { addr_of_mut!((*queue).used.index) };
    let mut completed = false;
    for _ in 0..READ_TIMEOUT_ITERATIONS {
        // 安全性：队列页由驱动和设备共享；volatile 读取用于观察设备异步更新的 used index。
        if unsafe { read_volatile(used_index) } != 0 {
            completed = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !completed {
        return Err(VirtioBlockError::RequestTimedOut);
    }
    // 安全性：设备已发布 used index；该 fence 保证后续读取设备写入的数据和状态不会越过完成检查。
    unsafe { core::arch::asm!("fence r, rw", options(nostack)) };

    // 安全性：used ring、请求状态和扇区缓冲区均由本请求独占，且设备已报告请求完成。
    unsafe {
        let used_entry = read_volatile(addr_of_mut!((*queue).used.ring[0]));
        if used_entry.id != 0 || used_entry.length != (SECTOR_SIZE + 1) as u32 {
            return Err(VirtioBlockError::InvalidUsedEntry);
        }
        let request = addr_of_mut!(REQUEST);
        if read_volatile(addr_of_mut!((*request).status)) != 0 {
            return Err(VirtioBlockError::DeviceReportedIoError);
        }
        core::ptr::copy_nonoverlapping(
            (*request).data.as_ptr(),
            destination.as_mut_ptr(),
            SECTOR_SIZE,
        );
    }

    Ok(())
}

/// 返回设备声明的逻辑扇区数。
pub(crate) fn capacity_sectors() -> usize {
    DEVICE_CAPACITY_SECTORS.load(Ordering::Acquire)
}

fn set_device_status(base: usize, status_bits: u32) {
    let status = read_register(base, DEVICE_STATUS);
    write_register(base, DEVICE_STATUS, status | status_bits);
}

fn read_register(base: usize, offset: usize) -> u32 {
    // 安全性：调用方传入经 FDT 校验的 MMIO 范围，且 offset 位于 VirtIO MMIO 寄存器布局内。
    unsafe { read_volatile((base + offset) as *const u32) }
}

fn write_register(base: usize, offset: usize, value: u32) {
    // 安全性：调用方传入经 FDT 校验的 MMIO 范围，且 offset 位于 VirtIO MMIO 寄存器布局内。
    unsafe { write_volatile((base + offset) as *mut u32, value) };
}

fn write_address(base: usize, low_offset: usize, address: u64) {
    write_register(base, low_offset, address as u32);
    write_register(base, low_offset + 4, (address >> 32) as u32);
}

fn encode_dma_address(physical_address: usize) -> Result<u64, VirtioBlockError> {
    u64::try_from(physical_address).map_err(|_| VirtioBlockError::InvalidDmaAddress)
}

fn checked_dma_offset(base: usize, offset: usize) -> Result<u64, VirtioBlockError> {
    let address = base
        .checked_add(offset)
        .ok_or(VirtioBlockError::InvalidDmaAddress)?;
    encode_dma_address(address)
}

/// 返回驱动静态 DMA 区域的内核虚拟地址，供启动层换算为物理地址。
pub(crate) fn dma_virtual_addresses() -> (usize, usize) {
    (addr_of_mut!(QUEUE) as usize, addr_of_mut!(REQUEST) as usize)
}

fn request_virtual_address() -> usize {
    addr_of_mut!(REQUEST) as usize
}

fn is_aligned(address: usize, alignment: usize) -> bool {
    address & (alignment - 1) == 0
}
