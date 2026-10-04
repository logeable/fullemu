//! 保存和恢复协作式任务切换所需的 RISC-V 整数寄存器现场。

/// 与 `context.S` 中寄存器偏移保持一致的任务上下文。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TaskContext {
    return_address: usize,
    stack_pointer: usize,
    saved_registers: [usize; 12],
}

impl TaskContext {
    /// 创建尚未初始化的上下文存储。
    pub const fn empty() -> Self {
        Self {
            return_address: 0,
            stack_pointer: 0,
            saved_registers: [0; 12],
        }
    }

    /// 创建一个首次恢复时从指定入口开始运行的上下文。
    pub fn for_entry(entry: extern "C" fn() -> !, stack_pointer: usize) -> Self {
        Self {
            return_address: entry as usize,
            stack_pointer,
            saved_registers: [0; 12],
        }
    }
}

extern "C" {
    fn riscv64_switch_context(previous: *mut TaskContext, next: *const TaskContext);
}

/// 从当前上下文切换到另一个上下文。
///
/// 当前实验只保存整数调用现场，所有任务必须运行在同一 hart、同一地址空间中，
/// 并且不得在 `yield` 调用间保持浮点寄存器状态。
///
/// # Safety
///
/// `previous` 必须指向独占且可写的上下文；`next` 必须指向已初始化的上下文，
/// 其栈在任务整个生命周期内有效且按 ABI 对齐。两个指针不能指向同一上下文。
/// 调用方还必须保证切换期间没有中断处理程序或其他 hart 并发访问调度器状态。
pub unsafe fn switch_context(previous: *mut TaskContext, next: *const TaskContext) {
    // 安全性：调用方保证两个上下文有效且不同；汇编按 `TaskContext` 的固定布局读写寄存器。
    unsafe { riscv64_switch_context(previous, next) };
}

core::arch::global_asm!(include_str!("context.S"));
