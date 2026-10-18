/* 所有用户程序从同一虚拟地址开始；段按页分开，避免不同权限内容共用一页。 */
OUTPUT_ARCH(riscv)
ENTRY(_start)

PROVIDE(__user_image_base = 0x40000000);

PHDRS
{
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = __user_image_base;

    .text : ALIGN(4096) {
        KEEP(*(.text.entry))
        *(.text .text.*)
        . = ALIGN(4096);
    } :text

    .rodata : ALIGN(4096) {
        *(.rodata .rodata.*)
        . = ALIGN(4096);
    } :rodata

    .data : ALIGN(4096) {
        __data_start = .;
        *(.sdata .sdata.*)
        *(.data .data.*)
    } :data

    __global_pointer$ = __data_start + 0x800;

    .bss (NOLOAD) : ALIGN(8) {
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
    } :data

    __user_program_end = .;

    ASSERT(_start == __user_image_base, "用户程序入口必须位于镜像起始地址")
    /DISCARD/ : {
        *(.eh_frame .eh_frame.*)
    }
}
