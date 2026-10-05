/*
 * QEMU virt / OpenSBI 的 S-mode 入口。
 *
 * 交接约定：a0 为 hart ID，a1 为扁平设备树（FDT）指针。
 * 运行脚本只启动一个 hart，因此将非零 hart 停在等待循环中。
 * 调用 Rust 前，此入口会设置栈和全局指针，并清零 .bss。
 */

    .section .text.entry
    .globl _start
    .type _start, @function
_start:
    /* 后续实现 SMP 时会替换当前的单 hart 临时策略。 */
    bnez    a0, .Lpark

    /* 当前实验使用显式让出；关闭 S-mode 中断，避免调度状态被异步打断。 */
    csrci   sstatus, 2

    /* 栈和小数据区基址有效后，才能运行 Rust 代码。 */
    la      sp, boot_stack_top
    /* S-mode 运行期间 sscratch 为零，陷入入口据此识别来自 S-mode 的陷入。 */
    csrw    sscratch, zero
    .option push
    .option norelax
    la      gp, __global_pointer$
    .option pop

    /* 在进入 Rust 前安装 S-mode 陷入入口。 */
    la      t0, supervisor_trap_entry
    csrw    stvec, t0

    /* 显式清零 .bss，确保 Rust 入口不依赖加载器对未初始化区域的处理方式。 */
    la      t0, __bss_start
    la      t1, __bss_end
.Lbss_clear:
    bgeu    t0, t1, .Lbss_done
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       .Lbss_clear

.Lbss_done:
    /* 保留 a0/a1，让 Rust 收到固件交接时提供的原始参数。 */
    tail    kernel_main

.Lpark:
    wfi
    j       .Lpark

    .size _start, . - _start
