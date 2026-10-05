/* 独立用户程序被链接到内核预留的固定物理地址，当前 satp=BARE 时可直接执行。 */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0x80400000;

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

    __global_pointer$ = __data_start + 0x800;

    .bss (NOLOAD) : ALIGN(8) {
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
    }

    __user_program_end = .;

    ASSERT(__user_program_end <= 0x80410000, "用户程序超过 64 KiB 加载区")

    /DISCARD/ : {
        *(.eh_frame .eh_frame.*)
    }
}
