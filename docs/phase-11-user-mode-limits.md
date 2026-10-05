# 第 11 阶段：U-mode 的限制与内存访问边界

## 目标关联

第 10 阶段证明 S-mode 任务可以直接改写内核调度状态。现在先引入 U-mode，观察降低 CPU 特权级本身能阻止什么、不能阻止什么，为随后建立内存保护提供可比较的基线。

## 实验问题

**进入 U-mode 后，程序是否就不能再访问内核？** 本阶段故意不启用 Sv39 页表，保持 `satp=BARE`，分别尝试普通内存读写和 S-mode CSR 访问。

## 实现范围

- 内核设置 `satp=BARE`，清除 supervisor 中断源使能，然后通过 `sret` 进入一个静态 U-mode 测试任务。
- U-mode 任务从内核映像读取一个专用内核数据值，将原值记录到另一个内核变量，再把原变量改写为新值。
- 完成内存读写后，U-mode 任务尝试读取 S-mode 的 `sstatus` CSR。该操作应触发非法指令异常并进入 S-mode 陷入处理程序。
- 陷入入口识别 U-mode 来源，通过 `sscratch` 切换到静态 S-mode 陷入栈，再保存现场。即使本阶段没有页级保护，也不把陷入帧放在 U-mode 栈上。
- 架构层把原始 `scause` 解码为有类型的中断或异常原因；陷入处理程序报告解码结果、原始 `scause`、`sepc`、`stval`，以及 U-mode 读写内核数据的结果。

## U-mode 限制了什么

U-mode 不能访问更高特权级的 S-mode CSR，也不能执行 S-mode 专用指令。尝试读取 `sstatus` 会产生非法指令异常，陷入到 S-mode。RISC-V 的 `sret` 根据陷入帧中的 SPP 决定返回 S-mode 还是 U-mode；S-mode 陷入 CSR 按特权级限制访问。参见 [RISC-V Supervisor-level ISA](https://docs.riscv.org/reference/isa/priv/supervisor.html)。

## U-mode 此时还没有限制什么

`satp=BARE` 时不做虚拟地址转换，也没有 PTE 的 `U`、`R`、`W`、`X` 页权限检查。访问仍受 PMP 等物理内存保护约束；当前 QEMU `virt` / OpenSBI 启动配置允许 S/U 访问内核所在的 RAM，因此测试程序能读取并改写内核数据。规范也明确说明 `satp.MODE=BARE` 时，地址直接对应物理地址，除 PMP 外没有额外内存保护。参见 [RISC-V Supervisor-level ISA](https://docs.riscv.org/reference/isa/priv/supervisor.html) 和 [Machine-level ISA：PMP 与分页](https://docs.riscv.org/reference/isa/priv/machine.html)。

因此，U-mode 限制了特权指令和 CSR 操作，但在当前配置中没有阻止 U-mode 读写内核数据、执行有执行权限的映射，或访问 PMP 允许的设备地址。降低执行特权级不等于建立了进程地址空间。

## 运行与验收

```sh
make build
make run
```

串口应报告：

1. U-mode 读到初始内核值 `0x4b45524e`。
2. U-mode 成功把内核值改为 `0x554d4f44`。
3. 后续读取 `sstatus` 产生非法指令陷入，`scause` 的异常码为 2，陷入来源为 U-mode。

QEMU 运行结果同时证明 U-mode 的特权限制有效，而内核数据访问仍未隔离。

## 当前边界

- 没有 Sv39 页表或按页访问权限；内核陷入栈本身也处于 U-mode 可访问的物理内存范围。
- 只有一个静态测试任务；没有用户任务调度、系统调用、用户映像加载或进程隔离。
- 本阶段故意使用可访问的专用内核数据，不修改调度器、陷入帧或其他关键内核状态。
- 当前实现依赖 QEMU `virt` / OpenSBI 提供的 PMP 配置；不能据此推断其他固件或平台也允许相同访问。

## 后续连接

下一阶段启用 Sv39：保持内核页面的 PTE `U=0`，仅把用户代码、数据和栈页映射为 `U=1`，然后重跑同一个内核写入尝试。预期写入触发 store page fault，内核变量保持原值；合法的用户页读写仍然成功。这个前后对照将第一次建立可验证的用户态内存保护。
