/* 独立用户程序入口。 */
    .section .text.entry
    .align 2
    .globl _start
    .type _start, @function
_start:
    /* 用户程序有独立的全局指针布局；在进入 Rust 代码或访问全局数据前初始化 gp。 */
    .option push
    .option norelax
    la      gp, __global_pointer$
    .option pop

    /* 用户主体通过系统调用输出文本并结束当前任务，不应返回到此处。 */
    call    user_main

    /* 若用户入口意外返回，则停留在用户态，避免执行未定义的后续逻辑。 */
.Lstop:
    j       .Lstop

    .size _start, . - _start
