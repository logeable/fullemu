//! 组织 VirtIO transport、通用队列机制和具体设备驱动。

pub(crate) mod block;
mod mmio;
mod virtqueue;
