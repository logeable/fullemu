/*
 * QEMU RISC-V virt 机器的最小链接布局。
 * QEMU 默认 OpenSBI 固件将 S-mode 内核加载到 0x80200000。
 */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0x80200000;
    __kernel_start = .;

    . = ALIGN(4);
    __text_start = .;
    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
    }
    __text_end = .;

    . = ALIGN(8);
    __rodata_start = .;
    .rodata : {
        *(.rodata .rodata.*)
    }
    __rodata_end = .;

    . = ALIGN(8);
    __data_start = .;
    .data : {
        *(.sdata .sdata.*)
        *(.data .data.*)
    }
    __data_end = .;

    /* 进入编译后的 Rust 代码前，必须先初始化 gp。 */
    __global_pointer$ = __data_start + 0x800;

    . = ALIGN(8);
    __bss_start = .;
    .bss (NOLOAD) : {
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
    }
    __bss_data_end = .;

    /* 第一个阶段只运行一个 hart，并为它分配独立的启动栈。 */
    . = ALIGN(16);
    __boot_stack_start = .;
    .bss.stack (NOLOAD) : {
        . += 16K;
        boot_stack_top = .;
    }
    __boot_stack_end = .;

    . = ALIGN(8);
    __bss_end = .;

    /* 为最多 8 个用户程序各预留一个 64 KiB 固定镜像槽位。 */
    __user_program_load_start = 0x80400000;
    __user_program_load_end = 0x80480000;

    ASSERT(__bss_end <= __user_program_load_start, "内核映像与用户程序加载区重叠")

    /DISCARD/ : {
        *(.eh_frame .eh_frame.*)
    }
}
