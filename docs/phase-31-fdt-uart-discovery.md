# 第 31 阶段：从 FDT 发现 UART 控制台

## 目标关联

日志、shell 输入和用户态 `read`/`write` 都依赖 UART，但驱动此前把 QEMU `virt` UART0 地址 `0x10000000` 写死。启动参数已经提供 FDT，继续硬编码会让平台数据绕过设备树，也使后续替换设备变得困难。本阶段让现有控制台从 FDT 获得寄存器物理范围。

## 实验问题

**内核在没有堆、且尚不能输出启动日志时，怎样从固件设备树找到当前平台的串口地址，并把平台信息交给设备驱动？**

## 实现范围

- 新增无堆的 `device_tree::console_info` 解析器，查找 `/soc` 下第一个状态为启用（或未声明状态）的 `ns16550a` / `ns16550` 兼容 UART。
- 按 `/soc` 的 `#address-cells`、`#size-cells` 解码 UART `reg` 的首个范围；拒绝不支持的 cell 数、缺失 `reg`、零地址、长度不足和地址溢出。
- 仅接受空 `/soc/ranges`，即 UART 子总线地址可直接作为物理地址。当前目标 QEMU `virt` 使用此布局。
- 启动路径在输出第一条日志前解析 FDT 并初始化 `console`。FDT 解析器只产出地址与长度；它不依赖驱动。`console` 把范围交给 `driver::uart16550`，驱动只访问 NS16550 寄存器，不解析 FDT。
- 页表仍通过控制台设备接口取得 UART MMIO 范围，并为各任务地址空间映射对应页面。

## 暂不实现

- 不解析 `/chosen/stdout-path` 或 `/aliases`；多个可用 UART 时选择 `/soc` 下第一个受支持且启用的设备。
- 不实现非空 `ranges` 的总线地址转换，也不扫描任意层级总线或其他串口控制器。
- 不实现 UART 中断、驱动注册表、自动匹配多种 UART 类型或运行时设备热插拔。
- UART 尚未发现时没有可用的串口输出；因此 FDT 头部或控制台发现失败只能停机，不能通过该 UART 报告错误。

## 依赖与不变量

- `device_tree::console_info` 依赖 FDT 结构遍历器，只输出无堆、拥有的 `FdtConsoleInfo` 值。
- `startup` 是设备树数据与控制台服务之间的组合点：先解析 FDT 控制台信息，再调用 `console::initialize`，之后才使用内核日志。
- `driver::uart16550` 不含平台物理地址常量；MMIO 范围来自 `console::initialize`，且在首次日志输出前固定。
- 页表映射的 MMIO 范围与驱动访问范围来自同一个 UART 配置，避免设备能输出但任务地址空间未映射该设备的分歧。

## 构建与验收

```sh
make fmt-check
make build
make run
```

QEMU `virt` 启动日志应报告 FDT UART 范围 `0x10000000..0x10000100`；后续启动日志、shell 提示符和用户程序输出应正常，shell 仍可从串口接收输入。源码中不应再有 UART 物理基址常量。

## 后续连接

日志与 shell 不再绑定到固定 UART 地址，为后续添加设备树选择的其他平台设备建立了“平台解析产生数据、启动代码组合、驱动消费数据”的边界。下一步若支持其他设备或平台，再根据实际 FDT 布局扩展 `ranges` 转换或 `stdout-path` 解析，不提前建立通用设备框架。
