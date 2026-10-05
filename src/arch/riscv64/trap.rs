//! 保存异步陷入现场，并从选定的任务陷入帧恢复执行。

/// 陷入时保存的完整整数寄存器和关键 S-mode CSR。
///
/// 字段顺序必须与 `trap.S` 中的偏移一致；结构体总大小为 288 字节，满足栈对齐。
#[repr(C)]
pub struct TrapFrame {
    /// 按 RISC-V 整数寄存器编号保存 x0 至 x31。
    pub registers: [usize; 32],
    /// 陷入后要恢复的指令地址。
    pub exception_pc: usize,
    /// `sret` 使用的 supervisor 状态。
    pub status: usize,
    /// 陷入原因。
    pub cause: usize,
    /// 陷入附加值。
    pub trap_value: usize,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 288);

impl TrapFrame {
    /// 为首次运行的内核任务构造一个可由 `sret` 恢复的陷入帧。
    pub fn for_entry(entry: extern "C" fn() -> !, stack_pointer: usize) -> Self {
        let mut registers = [0; 32];
        registers[2] = stack_pointer;

        Self {
            registers,
            exception_pc: entry as usize,
            // SPP=1 使 sret 返回 S-mode；SPIE=1 使返回任务后开放全局中断。
            status: (1 << 8) | (1 << 5),
            cause: 0,
            trap_value: 0,
        }
    }
}

extern "C" {
    fn riscv64_start_first_task(frame: *const TrapFrame) -> !;
}

/// 从预先构造的陷入帧启动首个任务。
///
/// # Safety
///
/// `frame` 必须指向按 16 字节对齐且在任务生命周期内有效的完整 `TrapFrame`；
/// 其中 `registers[2]` 必须是对应任务可写栈的有效栈顶，入口必须不返回。
pub unsafe fn start_first_task(frame: *const TrapFrame) -> ! {
    // 安全性：调用方保证陷入帧与任务栈有效；汇编按 `TrapFrame` 的固定布局恢复它。
    unsafe { riscv64_start_first_task(frame) }
}

core::arch::global_asm!(include_str!("trap.S"));
