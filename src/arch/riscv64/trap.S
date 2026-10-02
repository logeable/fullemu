/*
 * S-mode 陷入入口。
 *
 * 当前阶段只处理致命陷入，不恢复被打断的寄存器现场：入口将 scause、
 * sepc 和 stval 放入 C ABI 参数寄存器，再尾调用永不返回的 Rust 报告函数。
 */
    .section .text.trap
    .align 2
    .globl supervisor_trap_entry
    .type supervisor_trap_entry, @function
supervisor_trap_entry:
    csrr    a0, scause
    csrr    a1, sepc
    csrr    a2, stval
    tail    supervisor_trap_handler

    .size supervisor_trap_entry, . - supervisor_trap_entry
