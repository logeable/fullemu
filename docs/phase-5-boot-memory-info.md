# 第 5 阶段：FDT 启动内存信息

> 本阶段最初只验证固定堆与固定页池。第 30 阶段已将 `FdtMemoryInfo` 的 RAM、固件保留范围和 DTB 范围用于构造可用页帧区间，并由页帧分配器向堆提供后备页。

## 目标

把通用 FDT 字节格式转换成初始化阶段可消费的早期启动信息：解析根节点直接子节点中的内存设备 `reg` 属性，并收集固件保留范围与 DTB 自身范围。结果用于验证当前静态堆和页帧池后备区；尚未驱动通用物理页分配策略。

`reg` 中的地址和长度由父节点的 `#address-cells` 与 `#size-cells` 决定；缺失时按规范建议采用 2 和 1。参见 [Devicetree 规范：标准属性](https://devicetree-specification.readthedocs.io/en/v0.2/devicetree-basics.html) 与 [设备节点要求](https://devicetree-specification.readthedocs.io/en/v0.2/devicenodes.html)。

## 接口与实现范围

- `FdtMemoryInfo::parse` 在堆初始化前消费 FDT 结构事件，提取内存范围、固定容量的固件保留范围和 DTB 自身物理范围；结果不借用 DTB，也不分配堆内存。
- 内存节点通过 `device_type = "memory"` 或节点名 `memory` / `memory@...` 识别；当前只解释根节点的直接子节点。
- `reg` 支持 1 或 2 个地址 cell 与 1 或 2 个长度 cell，解码为 64 位物理地址和长度；检查项长度、零长度、加法溢出和范围重叠。
- `FdtMemoryInfo::memory_regions` 与 `FdtMemoryInfo::memory_reservations` 返回固定容量数组中的有效切片。超过容量会返回错误，不分配堆内存。
- `FdtMemoryInfo::contains_unreserved_ram_region` 用于确认完整范围处于 FDT 描述的 RAM 内，且不与 DTB 或固件保留区域重叠；页帧分配器按页使用此检查。
- 暂不解析 `/reserved-memory` 节点；页帧分配器只使用本模块已经提取的 RAM、reserve map 和 DTB 范围。
- 平台信息提取独立于通用 FDT 二进制格式解析；不解释 `/chosen`、CPU、UART 或其他设备 binding。

## 验证

```sh
make test-fdt
make build
make run
```

宿主机测试覆盖双 cell 多范围解码、规范默认 cell 数、缺失 `reg`、不完整项、重叠范围和不支持的 cell 数。QEMU `virt` 启动应先从 DTB 读取 RAM 范围，再验证静态堆和页帧池后初始化它们；RAM 起始地址应为 `0x80000000`，大小为 `0x08000000`。

## 当前限制

- 只识别根节点下的 RAM 节点；更复杂的总线地址转换和多级 `ranges` 映射不在本阶段处理。
- 地址和长度最多支持两个 32 位 cell；更宽的设备树整数明确拒绝。
- 内存区域与固件保留范围容量各为 32 项，便于早期内核在无堆环境中保存结果。
- 固件保留表与 `/reserved-memory` 是不同来源；目前仅保留前者，后者将在内存管理设计时单独解释。
- 可用页帧区构造和内核映像、用户程序槽位排除策略见[第 30 阶段](phase-30-fdt-backed-physical-memory.md)；本模块仍不负责通用分配器策略。
