//! 首个 QEMU 目标所需的 RISC-V 64 位启动代码和平台支持。

pub mod console;
// 此上下文切换实现保留第 8 阶段的调用点切换示例，当前默认实验改用完整陷入帧。
#[allow(dead_code)]
pub mod context;
pub mod interrupt;
pub mod sbi;
pub mod trap;

// 将入口代码单独放在汇编文件中，便于学习者查看调用约定、寄存器用法和初始化顺序。
core::arch::global_asm!(include_str!("boot.S"));
