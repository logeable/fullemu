/* 所有用户程序链接到同一虚拟入口，内核页表将其映射到各自物理镜像槽位。 */
OUTPUT_ARCH(riscv)
ENTRY(_start)

PROVIDE(__user_image_base = 0x80400000);

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
    ASSERT(__user_program_end <= __user_image_base + 0x10000, "用户程序超过 64 KiB 加载槽位")

    /DISCARD/ : {
        *(.eh_frame .eh_frame.*)
    }
}
