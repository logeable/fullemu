/*
 * QEMU RISC-V virt 内核布局：低地址启动入口，高半区链接 Rust 内核主体。
 * QEMU 默认 OpenSBI 固件从 0x80200000 加载并跳转到低地址入口。
 */
OUTPUT_ARCH(riscv)
ENTRY(_start)

KERNEL_VIRTUAL_OFFSET = 0xffffffc000000000;

PHDRS
{
    boot_text PT_LOAD FLAGS(5);
    boot_data PT_LOAD FLAGS(6);
    kernel_text PT_LOAD FLAGS(5);
    kernel_rodata PT_LOAD FLAGS(4);
    kernel_data PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = 0x80200000;
    __boot_start = .;
    .boot : {
        KEEP(*(.text.entry))
    } :boot_text
    __boot_end = .;
    __boot_virtual_start = __boot_start + KERNEL_VIRTUAL_OFFSET;
    __boot_virtual_end = __boot_end + KERNEL_VIRTUAL_OFFSET;

    . = ALIGN(4096);
    __boot_bss_start = .;
    .boot.bss (NOLOAD) : {
        KEEP(*(.bss.boot_page_table))
        . = ALIGN(16);
        __boot_stack_start = .;
        KEEP(*(.bss.boot_stack))
        __boot_stack_end = .;
        . = ALIGN(8);
    } :boot_data
    __boot_bss_end = .;
    __boot_bss_virtual_start = __boot_bss_start + KERNEL_VIRTUAL_OFFSET;
    __boot_bss_virtual_end = __boot_bss_end + KERNEL_VIRTUAL_OFFSET;
    boot_stack_top = __boot_stack_end;
    __boot_stack_virtual_start = __boot_stack_start + KERNEL_VIRTUAL_OFFSET;
    __boot_stack_virtual_end = __boot_stack_end + KERNEL_VIRTUAL_OFFSET;

    /* 高半区 VMA 与低地址 LMA 相差固定偏移，保持段内偏移一致。 */
    __kernel_load_start = ALIGN(LOADADDR(.boot.bss) + SIZEOF(.boot.bss), 4096);
    . = __kernel_load_start + KERNEL_VIRTUAL_OFFSET;
    __kernel_start = .;

    .text : AT(__kernel_load_start) {
        . = ALIGN(4096);
        __text_start = .;
        *(.text .text.*)
        . = ALIGN(4096);
        __text_end = .;
    } :kernel_text
    __text_load_start = LOADADDR(.text);

    . = ALIGN(4096);
    __rodata_load_start = ALIGN(LOADADDR(.text) + SIZEOF(.text), 4096);
    .rodata : AT(__rodata_load_start) {
        __rodata_start = .;
        *(.srodata .srodata.*)
        *(.rodata .rodata.*)
        . = ALIGN(4096);
        __rodata_end = .;
    } :kernel_rodata

    . = ALIGN(4096);
    __data_load_start = ALIGN(LOADADDR(.rodata) + SIZEOF(.rodata), 4096);
    .data : AT(__data_load_start) {
        __data_start = .;
        *(.sdata .sdata.*)
        *(.data .data.*)
        . = ALIGN(4096);
        __data_end = .;
    } :kernel_data

    /* Rust 使用高地址 gp；启动汇编通过低地址副本先构造它。 */
    __global_pointer$ = __data_start + 0x800;
    __global_pointer_load = __global_pointer$ - KERNEL_VIRTUAL_OFFSET;

    . = ALIGN(4096);
    __bss_load_start = ALIGN(LOADADDR(.data) + SIZEOF(.data), 4096);
    .bss (NOLOAD) : AT(__bss_load_start) {
        __bss_start = .;
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4096);
        __bss_data_end = .;
        __bss_end = .;
    } :kernel_data
    __bss_load_end = LOADADDR(.bss) + SIZEOF(.bss);
    __kernel_load_end = __bss_load_end;
    __kernel_load_end_virtual = __kernel_load_end + KERNEL_VIRTUAL_OFFSET;

    /* 低入口需要经低地址符号定位 Rust 续接点和高半区陷入入口。 */
    __kernel_main_load = LOADADDR(.text) + (kernel_main - ADDR(.text));
    __supervisor_trap_entry_load = LOADADDR(.text) + (supervisor_trap_entry - ADDR(.text));

    /* 为最多 8 个用户程序各预留一个 64 KiB 固定物理镜像槽位。 */
    __user_program_load_start = 0x80400000;
    __user_program_load_end = 0x80480000;
    __user_program_load_virtual_start = __user_program_load_start + KERNEL_VIRTUAL_OFFSET;
    __user_program_load_virtual_end = __user_program_load_end + KERNEL_VIRTUAL_OFFSET;

    ASSERT(__kernel_load_end <= __user_program_load_start, "内核物理映像与用户程序加载区重叠")
    ASSERT(__kernel_start < 0xffffffe000000000, "内核高地址超出 Sv39 高半区范围")
    ASSERT(__boot_stack_end > __boot_stack_start, "启动栈必须非空")

    /DISCARD/ : {
        *(.eh_frame .eh_frame.*)
    }
}
