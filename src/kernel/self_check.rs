//! 在启动阶段检查内核分配器的基本不变量。
//!
//! 这些检查运行在 QEMU 内核中，验证分配器自身行为；它们不是 shell 功能，也不是宿主机单元测试。

use super::frame_allocator::FrameError;
use super::heap::HeapError;

/// 描述启动自检失败的子系统。
pub(crate) enum StartupCheckError {
    HeapAllocator(HeapError),
    GlobalAllocator(HeapError),
    PhysicalFrameAllocator(FrameError),
}

impl StartupCheckError {
    /// 返回用于启动诊断的错误说明。
    pub(crate) fn description(&self) -> &'static str {
        match self {
            Self::HeapAllocator(error) => error.description(),
            Self::GlobalAllocator(error) => error.description(),
            Self::PhysicalFrameAllocator(error) => error.description(),
        }
    }
}

/// 运行各分配器的启动自检；调用前必须初始化内核堆和物理页帧分配器。
pub(crate) fn run_startup_checks() -> Result<(), StartupCheckError> {
    super::heap::check_allocator().map_err(StartupCheckError::HeapAllocator)?;
    super::heap::check_global_allocator().map_err(StartupCheckError::GlobalAllocator)?;
    super::frame_allocator::check_allocation()
        .map_err(StartupCheckError::PhysicalFrameAllocator)?;
    Ok(())
}
