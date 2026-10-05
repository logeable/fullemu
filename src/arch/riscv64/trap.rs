//! 保存异步陷入现场，并从选定的任务陷入帧恢复执行。

/// 解码后的 RISC-V 陷入原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrapCause {
    /// 异步中断。
    Interrupt(InterruptCause),
    /// 同步异常。
    Exception(ExceptionCause),
}

/// S-mode 可观察到的中断原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptCause {
    /// Supervisor 软件中断。
    SupervisorSoftware,
    /// Supervisor 定时器中断。
    SupervisorTimer,
    /// Supervisor 外部中断。
    SupervisorExternal,
    /// 当前内核尚未专门处理的中断编号。
    Other(usize),
}

/// RISC-V 异常原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExceptionCause {
    /// 指令地址未对齐。
    InstructionAddressMisaligned,
    /// 取指访问错误。
    InstructionAccessFault,
    /// 非法指令。
    IllegalInstruction,
    /// 断点。
    Breakpoint,
    /// 加载地址未对齐。
    LoadAddressMisaligned,
    /// 加载访问错误。
    LoadAccessFault,
    /// 存储或原子操作地址未对齐。
    StoreAmoAddressMisaligned,
    /// 存储或原子操作访问错误。
    StoreAmoAccessFault,
    /// U-mode 环境调用。
    UserEnvironmentCall,
    /// S-mode 环境调用。
    SupervisorEnvironmentCall,
    /// M-mode 环境调用。
    MachineEnvironmentCall,
    /// 取指页错误。
    InstructionPageFault,
    /// 加载页错误。
    LoadPageFault,
    /// 存储或原子操作页错误。
    StoreAmoPageFault,
    /// 当前内核尚未专门处理的异常编号。
    Other(usize),
}

impl TrapCause {
    /// 将硬件 `scause` 编码拆分为中断标志和原因编号。
    pub fn decode(raw: usize) -> Self {
        let interrupt_bit = 1usize << (usize::BITS - 1);
        let code = raw & !interrupt_bit;

        if raw & interrupt_bit != 0 {
            Self::Interrupt(match code {
                1 => InterruptCause::SupervisorSoftware,
                5 => InterruptCause::SupervisorTimer,
                9 => InterruptCause::SupervisorExternal,
                other => InterruptCause::Other(other),
            })
        } else {
            Self::Exception(match code {
                0 => ExceptionCause::InstructionAddressMisaligned,
                1 => ExceptionCause::InstructionAccessFault,
                2 => ExceptionCause::IllegalInstruction,
                3 => ExceptionCause::Breakpoint,
                4 => ExceptionCause::LoadAddressMisaligned,
                5 => ExceptionCause::LoadAccessFault,
                6 => ExceptionCause::StoreAmoAddressMisaligned,
                7 => ExceptionCause::StoreAmoAccessFault,
                8 => ExceptionCause::UserEnvironmentCall,
                9 => ExceptionCause::SupervisorEnvironmentCall,
                11 => ExceptionCause::MachineEnvironmentCall,
                12 => ExceptionCause::InstructionPageFault,
                13 => ExceptionCause::LoadPageFault,
                15 => ExceptionCause::StoreAmoPageFault,
                other => ExceptionCause::Other(other),
            })
        }
    }
}

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
    #[allow(dead_code)]
    pub fn for_entry(entry: extern "C" fn() -> !, stack_pointer: usize) -> Self {
        Self::for_privilege(entry, stack_pointer, true)
    }

    /// 为首次运行的 U-mode 任务构造一个可由 `sret` 恢复的陷入帧。
    pub fn for_user_entry(entry: extern "C" fn() -> !, stack_pointer: usize) -> Self {
        Self::for_privilege(entry, stack_pointer, false)
    }

    fn for_privilege(
        entry: extern "C" fn() -> !,
        stack_pointer: usize,
        supervisor_mode: bool,
    ) -> Self {
        let mut registers = [0; 32];
        registers[2] = stack_pointer;

        Self {
            registers,
            exception_pc: entry as usize,
            // SPIE=1 使 sret 后允许 supervisor 中断；SPP 决定返回 S-mode 还是 U-mode。
            status: (usize::from(supervisor_mode) << 8) | (1 << 5),
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

/// 设置 U-mode 陷入时要切换到的 S-mode 栈顶。
///
/// # Safety
///
/// `stack_pointer` 必须是 16 字节对齐、有效且足以容纳陷入帧和 Rust 陷入处理函数的 S-mode 栈顶。
/// 调用方必须在执行 U-mode 代码前调用此函数；陷入入口会使用 `sscratch` 完成栈切换。
pub unsafe fn set_user_kernel_stack(stack_pointer: usize) {
    // 安全性：调用方保证栈顶有效；S-mode 陷入入口在 U-mode 陷入时从 `sscratch` 取用它。
    unsafe {
        core::arch::asm!(
            "csrw sscratch, {stack_pointer}",
            stack_pointer = in(reg) stack_pointer,
            options(nomem, nostack)
        );
    }
}

core::arch::global_asm!(include_str!("trap.S"));
