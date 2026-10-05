/*
 * S-mode 陷入入口。
 *
 * S-mode 陷入沿用被打断的 S-mode 栈；U-mode 陷入通过 sscratch 切换到内核栈。
 * 两种路径都保存完整整数寄存器现场和关键 CSR，再把陷入帧交给 Rust 处理函数。
 */
    .section .text.trap
    .align 2
    .globl supervisor_trap_entry
    .type supervisor_trap_entry, @function
supervisor_trap_entry:
    /*
     * 入口约定：陷入来自 S-mode 时 sscratch=0；来自 U-mode 时 sscratch=内核栈顶。
     * csrrw 原子交换 sp 与 sscratch，因此交换后可用 sp 是否为零区分陷入来源。
     */
    csrrw   sp, sscratch, sp
    bnez    sp, .Ltrap_from_user

    /*
     * S-mode 路径：第一次交换把原 sp 暂存在 sscratch、让 sp 变为零；
     * 这里交换回来，恢复被打断的 S-mode 栈，并让 sscratch 继续保持零。
     */
    csrrw   sp, sscratch, sp
    /* 在原 S-mode 栈上为 32 个寄存器槽和 4 个 CSR 槽分配 288 字节陷入帧。 */
    addi    sp, sp, -288

    /*
     * 先保存会被入口代码改写的寄存器：ra 原样保存；t0 随后用于计算原 sp。
     * x0 恒为零但仍保留槽位，使寄存器编号直接对应 TrapFrame 偏移。
     */
    sd      zero, 0(sp)
    sd      ra, 8(sp)
    sd      t0, 40(sp)
    /* 原 sp 等于陷入帧底部加 288；写入 x2 槽位供 Rust 保存或切换上下文。 */
    addi    t0, sp, 288
    sd      t0, 16(sp)
    j       .Lsave_registers

.Ltrap_from_user:
    /*
     * U-mode 路径：交换后 sp 是预先设置的内核栈顶，sscratch 暂存用户 sp。
     * 陷入帧只能写入 S-mode 栈，不能依赖当前用户栈可写或可信。
     */
    addi    sp, sp, -288
    /* 保存入口会改写的寄存器；此时尚未借用 t0，因此先保存其陷入现场。 */
    sd      zero, 0(sp)
    sd      ra, 8(sp)
    sd      t0, 40(sp)
    /* 从 sscratch 取回交换出去的用户 sp，填入与 S-mode 路径相同的 x2 槽位。 */
    csrr    t0, sscratch
    sd      t0, 16(sp)
    /*
     * 陷入处理期间用零表示当前已在 S-mode 栈上，便于识别后续 S-mode 陷入。
     * 若处理程序将来返回 U-mode，必须在 sret 前重新设置 sscratch 为内核栈顶。
     */
    csrw    sscratch, zero

.Lsave_registers:
    /*
     * 保存其余整数寄存器。前两条路径已保存 x0、ra、sp、t0；
     * 以下偏移按 TrapFrame 中 x0 至 x31 的顺序排列，每个槽占 8 字节。
     */
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

    /* 保存陷入返回地址和状态；Rust 处理器可以据此判定原因或修改返回现场。 */
    csrr    t0, sepc
    sd      t0, 256(sp)
    csrr    t0, sstatus
    sd      t0, 264(sp)
    csrr    t0, scause
    sd      t0, 272(sp)
    csrr    t0, stval
    sd      t0, 280(sp)

    /* 用户寄存器可能含有任意 gp；进入 Rust 前恢复内核全局指针。 */
    .option push
    .option norelax
    la      gp, __global_pointer$
    .option pop

    /* C ABI 第一个参数 a0 传入陷入帧指针；处理器返回所选任务的陷入帧指针。 */
    mv      a0, sp
    call    supervisor_trap_handler

    /* 返回值是下一任务的陷入帧；从该任务自己的栈恢复寄存器，允许切换任务。 */
    mv      sp, a0
.Lrestore_trap_frame:
    /*
     * sp 指向待恢复任务的 TrapFrame。先恢复 sepc 和 sstatus，供最后的 sret 使用；
     * sret 才会依据 SPP 选择返回特权级，并依据 SPIE 恢复全局中断状态。
     */
    ld      t0, 256(sp)
    csrw    sepc, t0
    ld      t0, 264(sp)
    csrw    sstatus, t0

    /* 按保存时相同的偏移恢复整数寄存器；x0 无需恢复，因为硬件固定为零。 */
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
    /* 最后恢复 sp：从此不能再用当前陷入帧寻址；随后 sret 返回 sepc 指定的位置。 */
    ld      sp, 16(sp)
    sret

    .size supervisor_trap_entry, . - supervisor_trap_entry

    .globl riscv64_start_first_task
    .type riscv64_start_first_task, @function
riscv64_start_first_task:
    /* a0 指向预先构造的首个任务 TrapFrame；复用同一恢复路径并由 sret 首次进入任务。 */
    mv      sp, a0
    j       .Lrestore_trap_frame

    .size riscv64_start_first_task, . - riscv64_start_first_task
