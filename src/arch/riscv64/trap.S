/*
 * S-mode 陷入入口。
 *
 * 无论是中断还是异常，都先把完整整数寄存器现场和关键 CSR 保存到当前任务栈。
 * Rust 处理函数返回要恢复的陷入帧地址；切换栈后使用 sret 恢复对应任务。
 */
    .section .text.trap
    .align 2
    .globl supervisor_trap_entry
    .type supervisor_trap_entry, @function
supervisor_trap_entry:
    addi    sp, sp, -288

    /* 保存通用寄存器；x0 恒为零，槽位保留以保持 Rust 结构体偏移直观。 */
    sd      zero, 0(sp)
    sd      ra, 8(sp)
    sd      t0, 40(sp)
    addi    t0, sp, 288
    sd      t0, 16(sp)
    sd      gp, 24(sp)
    sd      tp, 32(sp)
    sd      t1, 48(sp)
    sd      t2, 56(sp)
    sd      s0, 64(sp)
    sd      s1, 72(sp)
    sd      a0, 80(sp)
    sd      a1, 88(sp)
    sd      a2, 96(sp)
    sd      a3, 104(sp)
    sd      a4, 112(sp)
    sd      a5, 120(sp)
    sd      a6, 128(sp)
    sd      a7, 136(sp)
    sd      s2, 144(sp)
    sd      s3, 152(sp)
    sd      s4, 160(sp)
    sd      s5, 168(sp)
    sd      s6, 176(sp)
    sd      s7, 184(sp)
    sd      s8, 192(sp)
    sd      s9, 200(sp)
    sd      s10, 208(sp)
    sd      s11, 216(sp)
    sd      t3, 224(sp)
    sd      t4, 232(sp)
    sd      t5, 240(sp)
    sd      t6, 248(sp)

    csrr    t0, sepc
    sd      t0, 256(sp)
    csrr    t0, sstatus
    sd      t0, 264(sp)
    csrr    t0, scause
    sd      t0, 272(sp)
    csrr    t0, stval
    sd      t0, 280(sp)

    mv      a0, sp
    call    supervisor_trap_handler

    /* 返回值是下一任务的陷入帧；从该任务自己的栈恢复寄存器。 */
    mv      sp, a0
.Lrestore_trap_frame:
    ld      t0, 256(sp)
    csrw    sepc, t0
    ld      t0, 264(sp)
    csrw    sstatus, t0

    ld      ra, 8(sp)
    ld      gp, 24(sp)
    ld      tp, 32(sp)
    ld      t0, 40(sp)
    ld      t1, 48(sp)
    ld      t2, 56(sp)
    ld      s0, 64(sp)
    ld      s1, 72(sp)
    ld      a0, 80(sp)
    ld      a1, 88(sp)
    ld      a2, 96(sp)
    ld      a3, 104(sp)
    ld      a4, 112(sp)
    ld      a5, 120(sp)
    ld      a6, 128(sp)
    ld      a7, 136(sp)
    ld      s2, 144(sp)
    ld      s3, 152(sp)
    ld      s4, 160(sp)
    ld      s5, 168(sp)
    ld      s6, 176(sp)
    ld      s7, 184(sp)
    ld      s8, 192(sp)
    ld      s9, 200(sp)
    ld      s10, 208(sp)
    ld      s11, 216(sp)
    ld      t3, 224(sp)
    ld      t4, 232(sp)
    ld      t5, 240(sp)
    ld      t6, 248(sp)
    ld      sp, 16(sp)
    sret

    .size supervisor_trap_entry, . - supervisor_trap_entry

    .globl riscv64_start_first_task
    .type riscv64_start_first_task, @function
riscv64_start_first_task:
    /* a0 指向预先构造的首个任务陷入帧，恢复后由 sret 进入任务入口。 */
    mv      sp, a0
    j       .Lrestore_trap_frame

    .size riscv64_start_first_task, . - riscv64_start_first_task
