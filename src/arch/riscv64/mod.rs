//! 首个 QEMU 目标所需的 RISC-V 64 位启动代码和平台支持。

pub mod console;

// 将入口代码单独放在汇编文件中，便于学习者查看调用约定、寄存器用法和初始化顺序。
core::arch::global_asm!(include_str!("boot.S"));
// 异常入口独立存放，便于单独阅读陷入时的寄存器交接过程。
core::arch::global_asm!(include_str!("trap.S"));
