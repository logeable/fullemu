/* 独立用户程序入口；a0 指向三个 RV64 字长字段组成的内核内存探针。 */
    .section .text.entry
    .align 2
    .globl _start
    .type _start, @function
_start:
    /* 用户程序有独立的全局指针布局；在进入 Rust 代码或访问全局数据前初始化 gp。 */
    .option push
    .option norelax
    la      gp, __global_pointer$
    .option pop

    /* a0 是当前实验传入的探针地址；用户主体完成探针访问和 write 系统调用。 */
    call    user_main

    /* 用寄存器标记确认系统调用返回后仍在用户程序中继续执行。 */
    li      a1, 0x55534552

    /* 故意访问 S-mode CSR；预期产生非法指令陷入。 */
    csrr    zero, sstatus

.Lstop:
    j       .Lstop

    .size _start, . - _start
