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

    /* 探针偏移 0 是内核值，偏移 8 记录读值，偏移 16 记录用户写入值。 */
    ld      t0, 0(a0)
    sd      t0, 8(a0)
    li      a1, 0x55534552
    sd      a1, 0(a0)
    sd      a1, 16(a0)

    /* 故意访问 S-mode CSR；预期产生非法指令陷入，陷入帧应保留 a1 标记。 */
    csrr    zero, sstatus

.Lstop:
    j       .Lstop

    .size _start, . - _start
