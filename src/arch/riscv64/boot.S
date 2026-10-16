/*
 * QEMU virt / OpenSBI 的 S-mode 入口。
 *
 * 交接约定：a0 为 hart ID，a1 为扁平设备树（FDT）指针。
 * 运行脚本只启动一个 hart，因此将非零 hart 停在等待循环中。
 * 调用 Rust 前，此入口建立临时 Sv39 映射，并切换到高半区内核主体。
 */

    .section .text.entry
    .globl _start
    .type _start, @function
_start:
    /* 后续实现 SMP 时会替换当前的单 hart 临时策略。 */
    bnez    a0, .Lpark

    /* 当前实验使用显式让出；关闭 S-mode 中断，避免调度状态被异步打断。 */
    csrci   sstatus, 2

    /* S-mode 运行期间 sscratch 为零，陷入入口据此识别来自 S-mode 的陷入。 */
    csrw    sscratch, zero

    /* 分页切换前使用低地址紧急入口，任何意外陷入都会停机。 */
    la      t0, .Learly_trap
    csrw    stvec, t0

    /* 清零低地址启动区，为临时根页表和启动栈准备有效存储。 */
    la      t0, __boot_bss_start
    la      t1, __boot_bss_end
.Lbss_clear:
    bgeu    t0, t1, .Lbss_done
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       .Lbss_clear

.Lbss_done:
    /* Rust 主体的高地址数据区也必须在进入 Rust 前按物理装载地址清零。 */
    la      t0, __bss_load_start
    la      t1, __bss_load_end
.Lkernel_bss_clear:
    bgeu    t0, t1, .Lkernel_bss_done
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       .Lkernel_bss_clear

.Lkernel_bss_done:
    /* 临时页表用 1 GiB 叶子项映射低端设备区、QEMU RAM 和内核高地址别名。 */
    la      t1, boot_page_table
    li      t0, 0
    li      t2, 512
.Lclear_root_table:
    sd      zero, 0(t1)
    addi    t1, t1, 8
    addi    t2, t2, -1
    bnez    t2, .Lclear_root_table

    la      t1, boot_page_table
    li      t0, 0xcf
    sd      t0, 0(t1)

    li      t0, 0x200000cf
    li      t2, 2
    slli    t2, t2, 3
    add     t2, t1, t2
    sd      t0, 0(t2)

    /* Sv39 高地址别名对应根页表索引 258，与低地址 RAM 映射同指向 0x80000000。 */
    li      t2, 258
    slli    t2, t2, 3
    add     t2, t1, t2
    sd      t0, 0(t2)

    /* 打开 Sv39；当前低地址指令、FDT、UART 和 RAM 均仍有恒等映射。 */
    srli    t1, t1, 12
    li      t0, 0x8000000000000000
    or      t1, t1, t0
    csrw    satp, t1
    sfence.vma zero, zero

    /* 将陷入入口切至高地址；切换期间低地址紧急入口仍保持有效。 */
    la      t0, __supervisor_trap_entry_load
    li      t1, 0xffffffc000000000
    add     t0, t0, t1
    csrw    stvec, t0

    /* 高地址栈和 gp 均指向与低地址启动区相同的物理存储。 */
    la      sp, boot_stack_top
    add     sp, sp, t1
    .option push
    .option norelax
    la      gp, __global_pointer_load
    .option pop
    add     gp, gp, t1

    /* 保留固件传入的 a0/a1，在高地址继续执行 Rust 初始化。 */
    la      t0, __kernel_main_load
    add     t0, t0, t1
    jr      t0

.Learly_trap:
    /* 早期异常没有可恢复的内核状态，停在低地址启动代码中。 */
    wfi
    j       .Learly_trap

.Lpark:
    wfi
    j       .Lpark

    .size _start, . - _start

    .section .bss.boot_page_table,"aw",@nobits
    .balign 4096
    .globl boot_page_table
boot_page_table:
    .space 4096

    .section .bss.boot_stack,"aw",@nobits
    .balign 16
    .space 16384
