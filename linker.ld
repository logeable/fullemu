/*
 * QEMU RISC-V virt 机器的最小链接布局。
 * QEMU 默认 OpenSBI 固件将 S-mode 内核加载到 0x80200000。
 */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0x80200000;

    .text : ALIGN(4) {
        KEEP(*(.text.entry))
        *(.text .text.*)
    }

    .rodata : ALIGN(8) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(8) {
        __data_start = .;
        *(.sdata .sdata.*)
        *(.data .data.*)
    }

    /* 进入编译后的 Rust 代码前，必须先初始化 gp。 */
    __global_pointer$ = __data_start + 0x800;

    .bss (NOLOAD) : ALIGN(8) {
        __bss_start = .;
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
    }

    /* 第一个阶段只运行一个 hart，并为它分配独立的启动栈。 */
    .bss.stack (NOLOAD) : ALIGN(16) {
        . += 16K;
        boot_stack_top = .;
    }

    . = ALIGN(8);
    __bss_end = .;

    /DISCARD/ : {
        *(.eh_frame .eh_frame.*)
    }
}
