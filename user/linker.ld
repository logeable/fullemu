/* 所有用户程序链接到同一虚拟入口，内核页表将其映射到各自拥有的物理页帧。 */
OUTPUT_ARCH(riscv)
ENTRY(_start)

PROVIDE(__user_image_base = 0x40000000);

SECTIONS
{
    . = __user_image_base;

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

    ASSERT(_start == __user_image_base, "用户程序入口必须位于镜像起始地址")
    /DISCARD/ : {
        *(.eh_frame .eh_frame.*)
    }
}
