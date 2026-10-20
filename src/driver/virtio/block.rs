//! 实现 VirtIO 块设备协议，并通过共享的 split virtqueue 机制提交请求。
//!
//! 当前驱动只支持一个活动设备和串行读写，不启用设备中断，也不管理并发请求。

use core::ptr::{addr_of_mut, read_volatile};
use core::sync::atomic::{AtomicBool, Ordering};

use super::super::MmioRegion;
use super::mmio::{VirtioMmioError, VirtioMmioTransport, VIRTIO_F_VERSION_1};
use super::virtqueue::{QueueBuffer, VirtioSplitQueue, VirtqueueError};
use crate::block::{BlockDevice, BlockDeviceError, SECTOR_SIZE};

const VIRTIO_DEVICE_BLOCK: u32 = 2;

const BLOCK_REQUEST_READ: u32 = 0;
const BLOCK_REQUEST_WRITE: u32 = 1;

const CONFIG_GENERATION: usize = 0x0fc;
const CONFIG_CAPACITY: usize = 0x100;

/// VirtIO 块设备初始化或请求处理失败。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VirtioBlockError {
    /// MMIO 区域未覆盖本驱动访问的寄存器。
    InvalidRegisterRegion,
    /// VirtIO 设备状态寄存器拒绝初始化。
    DeviceRejectedInitialization,
    /// 本设备没有提供所需的现代 VirtIO 接口特性。
    MissingVersionOneFeature,
    /// 设备提供的队列容量不足。
    QueueTooSmall,
    /// 队列地址不能表示为设备使用的物理地址。
    InvalidDmaAddress,
    /// 块设备没有可读写的扇区。
    EmptyDevice,
    /// 扇区编号超出设备容量。
    SectorOutOfRange,
    /// 队列请求未能在轮询期限内完成。
    RequestTimedOut,
    /// 设备在请求完成项中返回了错误状态。
    DeviceReportedIoError,
    /// 队列返回的使用项与请求不匹配。
    InvalidUsedEntry,
    /// 队列发生错误后不能安全地继续提交请求。
    DeviceUnavailable,
    /// 设备驱动提供的缓冲区描述符链无效。
    InvalidQueueBufferChain,
    /// 当前单设备驱动已有活动设备实例。
    DeviceAlreadyInitialized,
}

impl VirtioBlockError {
    /// 返回适合启动诊断的错误说明。
    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::InvalidRegisterRegion => "VirtIO MMIO 寄存器范围无效",
            Self::DeviceRejectedInitialization => "VirtIO 设备拒绝初始化状态",
            Self::MissingVersionOneFeature => "VirtIO 设备不支持现代版本特性",
            Self::QueueTooSmall => "VirtIO 请求队列容量不足",
            Self::InvalidDmaAddress => "VirtIO 块请求 DMA 地址无效",
            Self::EmptyDevice => "VirtIO 块设备容量为空",
            Self::SectorOutOfRange => "扇区编号超出块设备容量",
            Self::RequestTimedOut => "VirtIO 设备请求超时",
            Self::DeviceReportedIoError => "VirtIO 块设备报告读请求失败",
            Self::InvalidUsedEntry => "VirtIO used ring 返回项无效",
            Self::DeviceUnavailable => "VirtIO 块设备队列不可继续使用",
            Self::InvalidQueueBufferChain => "VirtIO 块请求描述符链无效",
            Self::DeviceAlreadyInitialized => "当前 VirtIO 块驱动已有活动设备实例",
        }
    }
}

impl From<VirtqueueError> for VirtioBlockError {
    fn from(error: VirtqueueError) -> Self {
        match error {
            VirtqueueError::InvalidDmaAddress => Self::InvalidDmaAddress,
            VirtqueueError::RequestTimedOut => Self::RequestTimedOut,
            VirtqueueError::InvalidUsedEntry => Self::InvalidUsedEntry,
            VirtqueueError::Unavailable => Self::DeviceUnavailable,
            VirtqueueError::InvalidBufferChain => Self::InvalidQueueBufferChain,
        }
    }
}

impl From<VirtioMmioError> for VirtioBlockError {
    fn from(error: VirtioMmioError) -> Self {
        match error {
            VirtioMmioError::InvalidRegisterRegion | VirtioMmioError::RegisterOutsideRegion => {
                Self::InvalidRegisterRegion
            }
            VirtioMmioError::DeviceRejectedStatus => Self::DeviceRejectedInitialization,
        }
    }
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

static mut REQUEST: RequestMemory = RequestMemory::EMPTY;
static DEVICE_CLAIMED: AtomicBool = AtomicBool::new(false);

/// 已初始化的 VirtIO 块设备及其串行请求状态。
///
/// 当前驱动为一个设备保留一组静态 DMA 队列和请求缓冲区；调用者通过可变借用串行提交请求。
pub(crate) struct VirtioBlockDevice {
    transport: VirtioMmioTransport,
    queue: VirtioSplitQueue,
    sector_count: u64,
    request_physical_address: usize,
}

impl VirtioBlockDevice {
    /// 若该 MMIO 区域是块设备则初始化并返回设备对象；其他 VirtIO 设备返回 `Ok(None)`。
    pub(crate) fn probe(region: MmioRegion) -> Result<Option<Self>, VirtioBlockError> {
        let Some(transport) = VirtioMmioTransport::probe(region)? else {
            return Ok(None);
        };
        if region.size < CONFIG_CAPACITY + core::mem::size_of::<u64>() {
            return Err(VirtioBlockError::InvalidRegisterRegion);
        }
        if transport.device_id()? != VIRTIO_DEVICE_BLOCK {
            return Ok(None);
        }

        if DEVICE_CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(VirtioBlockError::DeviceAlreadyInitialized);
        }

        // 当前 QEMU 平台没有 IOMMU；静态 DMA 区域位于内核高半区直接映射中，设备地址等于对应物理地址。
        let queue_physical_address = crate::memory::address::kernel_virtual_to_physical(
            VirtioSplitQueue::dma_virtual_address(),
        );
        let request_physical_address =
            crate::memory::address::kernel_virtual_to_physical(request_virtual_address());
        let (sector_count, queue) =
            match initialize_device(transport, queue_physical_address, request_physical_address) {
                Ok(initialized) => initialized,
                Err(error) => {
                    DEVICE_CLAIMED.store(false, Ordering::Release);
                    return Err(error);
                }
            };

        Ok(Some(Self {
            transport,
            queue,
            sector_count,
            request_physical_address,
        }))
    }

    fn transfer_sector(
        &mut self,
        sector: u64,
        request_type: u32,
        data: &mut [u8; SECTOR_SIZE],
    ) -> Result<(), VirtioBlockError> {
        if sector >= self.sector_count {
            return Err(VirtioBlockError::SectorOutOfRange);
        }

        // 安全性：本设备对象独占静态请求缓冲区；可变借用保证请求完成前不会复用该缓冲区。
        unsafe {
            let request = addr_of_mut!(REQUEST);
            (*request).header = BlockRequestHeader {
                request_type,
                reserved: 0,
                sector,
            };
            (*request).status = u8::MAX;
            if request_type == BLOCK_REQUEST_WRITE {
                (*request).data.copy_from_slice(data);
            } else {
                (*request).data.fill(0);
            }
        }

        let buffers = [
            QueueBuffer {
                address: checked_dma_offset(
                    self.request_physical_address,
                    core::mem::offset_of!(RequestMemory, header),
                )?,
                length: core::mem::size_of::<BlockRequestHeader>() as u32,
                device_writes: false,
            },
            QueueBuffer {
                address: checked_dma_offset(
                    self.request_physical_address,
                    core::mem::offset_of!(RequestMemory, data),
                )?,
                length: SECTOR_SIZE as u32,
                device_writes: request_type == BLOCK_REQUEST_READ,
            },
            QueueBuffer {
                address: checked_dma_offset(
                    self.request_physical_address,
                    core::mem::offset_of!(RequestMemory, status),
                )?,
                length: 1,
                device_writes: true,
            },
        ];
        let expected_used_length = if request_type == BLOCK_REQUEST_READ {
            (SECTOR_SIZE + 1) as u32
        } else {
            1
        };
        self.queue.submit(&buffers)?;
        if let Err(error) = self.transport.notify_queue(0) {
            self.queue.mark_unavailable();
            return Err(error.into());
        }
        self.queue.wait_for_completion(expected_used_length)?;

        // 安全性：used ring、请求状态和扇区缓冲区均由本请求独占，且设备已报告请求完成。
        unsafe {
            let request = addr_of_mut!(REQUEST);
            if read_volatile(addr_of_mut!((*request).status)) != 0 {
                return Err(VirtioBlockError::DeviceReportedIoError);
            }
            if request_type == BLOCK_REQUEST_READ {
                data.copy_from_slice(&(*request).data);
            }
        }

        Ok(())
    }
}

impl Drop for VirtioBlockDevice {
    fn drop(&mut self) {
        if self.queue.is_available() {
            // 已完成的请求不再访问 DMA 区；复位设备后，静态队列可以安全地交给下一实例。
            let _ = self.transport.reset();
            DEVICE_CLAIMED.store(false, Ordering::Release);
        }
    }
}

impl BlockDevice for VirtioBlockDevice {
    fn sector_count(&self) -> u64 {
        self.sector_count
    }

    fn read_sector(
        &mut self,
        sector: u64,
        destination: &mut [u8; SECTOR_SIZE],
    ) -> Result<(), BlockDeviceError> {
        self.transfer_sector(sector, BLOCK_REQUEST_READ, destination)
            .map_err(map_block_error)
    }

    fn write_sector(
        &mut self,
        sector: u64,
        source: &[u8; SECTOR_SIZE],
    ) -> Result<(), BlockDeviceError> {
        let mut data = *source;
        self.transfer_sector(sector, BLOCK_REQUEST_WRITE, &mut data)
            .map_err(map_block_error)
    }
}

fn map_block_error(error: VirtioBlockError) -> BlockDeviceError {
    match error {
        VirtioBlockError::SectorOutOfRange => BlockDeviceError::SectorOutOfRange,
        _ => BlockDeviceError::Io,
    }
}

fn initialize_device(
    transport: VirtioMmioTransport,
    queue_physical_address: usize,
    request_physical_address: usize,
) -> Result<(u64, VirtioSplitQueue), VirtioBlockError> {
    transport.reset()?;
    transport.acknowledge()?;

    let high_features = transport.device_features(1)?;
    if high_features & VIRTIO_F_VERSION_1 == 0 {
        return Err(VirtioBlockError::MissingVersionOneFeature);
    }

    transport.set_driver_features(0, 0)?;
    transport.set_driver_features(1, VIRTIO_F_VERSION_1)?;
    if !transport.confirm_features()? {
        return Err(VirtioBlockError::DeviceRejectedInitialization);
    }

    if !is_aligned(request_virtual_address(), 4096) || !is_aligned(request_physical_address, 4096) {
        return Err(VirtioBlockError::InvalidDmaAddress);
    }

    // 安全性：单实例占用令牌保证设备尚未使用这组静态请求缓冲区。
    unsafe { addr_of_mut!(REQUEST).write(RequestMemory::EMPTY) };
    let queue = VirtioSplitQueue::new(queue_physical_address)?;
    transport.select_queue(0)?;
    if transport.queue_size_max()? < VirtioSplitQueue::size() {
        return Err(VirtioBlockError::QueueTooSmall);
    }
    transport.set_queue_size(VirtioSplitQueue::size())?;
    let queue_addresses = queue.addresses();
    transport.set_queue_addresses(
        queue_addresses.descriptors,
        queue_addresses.available,
        queue_addresses.used,
    )?;
    transport.set_queue_ready()?;

    let generation_before = transport.read_config(CONFIG_GENERATION)?;
    let capacity_low = transport.read_config(CONFIG_CAPACITY)? as u64;
    let capacity_high = transport.read_config(CONFIG_CAPACITY + 4)? as u64;
    let generation_after = transport.read_config(CONFIG_GENERATION)?;
    if generation_before != generation_after {
        return Err(VirtioBlockError::InvalidRegisterRegion);
    }
    let capacity = capacity_low | (capacity_high << 32);
    if capacity == 0 {
        return Err(VirtioBlockError::EmptyDevice);
    }
    if !transport.confirm_driver_ready()? {
        return Err(VirtioBlockError::DeviceRejectedInitialization);
    }
    Ok((capacity, queue))
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

fn request_virtual_address() -> usize {
    addr_of_mut!(REQUEST) as usize
}

fn is_aligned(address: usize, alignment: usize) -> bool {
    address & (alignment - 1) == 0
}
