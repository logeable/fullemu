//! 管理 VirtIO split virtqueue 的描述符、环形区和串行请求完成流程。
//!
//! 本模块只负责队列协议机制；请求中各缓冲区的含义由具体设备驱动决定。

use core::ptr::{addr_of_mut, read_volatile};

const QUEUE_SIZE: usize = 8;
const REQUEST_TIMEOUT_ITERATIONS: usize = 100_000_000;

const DESCRIPTOR_NEXT: u16 = 1;
const DESCRIPTOR_DEVICE_WRITES: u16 = 2;

/// VirtIO split virtqueue 初始化或请求处理失败。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum VirtqueueError {
    /// 队列的虚拟或物理地址未满足协议对齐要求。
    InvalidDmaAddress,
    /// 请求没有在轮询期限内完成。
    RequestTimedOut,
    /// 设备返回的使用项编号或长度不符合当前请求。
    InvalidUsedEntry,
    /// 队列状态损坏后不能安全地继续复用。
    Unavailable,
    /// 缓冲区链为空、过长或包含零长度缓冲区。
    InvalidBufferChain,
}

/// 一个交给设备读取或写入的请求缓冲区。
#[derive(Clone, Copy)]
pub(super) struct QueueBuffer {
    /// 设备可访问的物理地址。
    pub(super) address: u64,
    /// 缓冲区长度，单位为字节。
    pub(super) length: u32,
    /// 为真时表示设备向该缓冲区写入数据。
    pub(super) device_writes: bool,
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

/// 描述符表、available ring 和 used ring 组成的 DMA 区域。
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

static mut QUEUE_MEMORY: QueueMemory = QueueMemory::EMPTY;

/// 一个固定大小、单请求串行的 split virtqueue。
///
/// 当前实现只允许一个设备实例和一个未完成请求；超时或 used ring 异常后队列会失效。
pub(super) struct VirtioSplitQueue {
    available_index: u16,
    used_index: u16,
    available: bool,
    addresses: QueueAddresses,
}

/// split virtqueue 三个区域各自的设备物理地址。
#[derive(Clone, Copy)]
pub(super) struct QueueAddresses {
    pub(super) descriptors: u64,
    pub(super) available: u64,
    pub(super) used: u64,
}

impl VirtioSplitQueue {
    /// 初始化固定大小队列内存，并计算设备需要的三个物理区域地址。
    pub(super) fn new(queue_physical_address: usize) -> Result<Self, VirtqueueError> {
        // 安全性：调用方在初始化前独占单实例占用令牌，设备尚未使用这段静态 DMA 区域。
        unsafe { addr_of_mut!(QUEUE_MEMORY).write(QueueMemory::EMPTY) };

        let queue_pointer = addr_of_mut!(QUEUE_MEMORY);
        let queue_virtual_address = queue_pointer as usize;
        // 安全性：静态队列对象有效且字段布局由 repr(C) 固定；这里只计算字段地址。
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
        {
            return Err(VirtqueueError::InvalidDmaAddress);
        }

        let addresses = QueueAddresses {
            descriptors: checked_dma_offset(
                queue_physical_address,
                core::mem::offset_of!(QueueMemory, descriptors),
            )?,
            available: checked_dma_offset(
                queue_physical_address,
                core::mem::offset_of!(QueueMemory, available),
            )?,
            used: checked_dma_offset(
                queue_physical_address,
                core::mem::offset_of!(QueueMemory, used),
            )?,
        };

        Ok(Self {
            available_index: 0,
            used_index: 0,
            available: true,
            addresses,
        })
    }

    /// 返回设备配置队列时需要的区域物理地址。
    pub(super) const fn addresses(&self) -> QueueAddresses {
        self.addresses
    }

    /// 返回该队列描述符表和环形区的容量。
    pub(super) const fn size() -> u32 {
        QUEUE_SIZE as u32
    }

    /// 返回队列 DMA 区域的内核虚拟地址，供块驱动换算为设备地址。
    pub(super) fn dma_virtual_address() -> usize {
        addr_of_mut!(QUEUE_MEMORY) as usize
    }

    /// 提交一条缓冲区链，通知设备并轮询等待其完成。
    pub(super) fn submit(&mut self, buffers: &[QueueBuffer]) -> Result<(), VirtqueueError> {
        if !self.available {
            return Err(VirtqueueError::Unavailable);
        }
        if buffers.is_empty()
            || buffers.len() > QUEUE_SIZE
            || buffers.iter().any(|buffer| buffer.length == 0)
        {
            return Err(VirtqueueError::InvalidBufferChain);
        }

        // 安全性：当前队列只允许一个未完成请求；设备可访问的缓冲区已由调用方保持有效。
        unsafe {
            let queue = addr_of_mut!(QUEUE_MEMORY);
            let descriptors = addr_of_mut!((*queue).descriptors).cast::<Descriptor>();
            for (index, buffer) in buffers.iter().enumerate() {
                let has_next = index + 1 < buffers.len();
                descriptors.add(index).write(Descriptor {
                    address: buffer.address,
                    length: buffer.length,
                    flags: (if has_next { DESCRIPTOR_NEXT } else { 0 })
                        | if buffer.device_writes {
                            DESCRIPTOR_DEVICE_WRITES
                        } else {
                            0
                        },
                    next: if has_next { (index + 1) as u16 } else { 0 },
                });
            }

            let ring_slot = self.available_index as usize % QUEUE_SIZE;
            addr_of_mut!((*queue).available.ring[ring_slot]).write_volatile(0);
            self.available_index = self.available_index.wrapping_add(1);
            addr_of_mut!((*queue).available.index).write_volatile(self.available_index);
        }

        // 安全性：描述符链和可用环已完整写入 RAM；该 fence 保证 doorbell 前 DMA 数据可见。
        unsafe { core::arch::asm!("fence w, o", options(nostack)) };
        Ok(())
    }

    /// 等待设备完成已提交请求，并校验使用项长度。
    pub(super) fn wait_for_completion(
        &mut self,
        expected_used_length: u32,
    ) -> Result<(), VirtqueueError> {
        if !self.available {
            return Err(VirtqueueError::Unavailable);
        }
        let queue = addr_of_mut!(QUEUE_MEMORY);
        // 安全性：队列指针指向有效静态对象；这里只计算 used index 字段地址供 volatile 读取。
        let used_index_pointer = unsafe { addr_of_mut!((*queue).used.index) };
        let expected_used_index = self.used_index.wrapping_add(1);
        let mut completed = false;
        for _ in 0..REQUEST_TIMEOUT_ITERATIONS {
            // 安全性：设备异步更新共享的 used index，必须使用 volatile 读取观察完成状态。
            let current_index = unsafe { read_volatile(used_index_pointer) };
            if current_index == expected_used_index {
                completed = true;
                break;
            }
            if current_index != self.used_index {
                self.available = false;
                return Err(VirtqueueError::InvalidUsedEntry);
            }
            core::hint::spin_loop();
        }
        if !completed {
            self.available = false;
            return Err(VirtqueueError::RequestTimedOut);
        }

        // 安全性：设备已发布 used index；该 fence 保证后续读取设备写入的数据不会越过完成检查。
        unsafe { core::arch::asm!("fence r, rw", options(nostack)) };

        let used_ring_slot = self.used_index as usize % QUEUE_SIZE;
        self.used_index = expected_used_index;
        // 安全性：used ring 已由设备发布完成；读取项在本次请求处理期间保持稳定。
        let used_entry = unsafe { read_volatile(addr_of_mut!((*queue).used.ring[used_ring_slot])) };
        if used_entry.id != 0 || used_entry.length != expected_used_length {
            self.available = false;
            return Err(VirtqueueError::InvalidUsedEntry);
        }
        Ok(())
    }

    /// 禁止复用已提交但未能可靠通知或完成的队列。
    pub(super) fn mark_unavailable(&mut self) {
        self.available = false;
    }

    /// 返回队列是否仍可安全提交请求。
    pub(super) const fn is_available(&self) -> bool {
        self.available
    }
}

fn checked_dma_offset(base: usize, offset: usize) -> Result<u64, VirtqueueError> {
    let address = base
        .checked_add(offset)
        .ok_or(VirtqueueError::InvalidDmaAddress)?;
    u64::try_from(address).map_err(|_| VirtqueueError::InvalidDmaAddress)
}

fn is_aligned(address: usize, alignment: usize) -> bool {
    address & (alignment - 1) == 0
}
