# 第 5 阶段：FDT 启动内存信息

## 目标

把通用 FDT 字节格式转换成内核可消费的早期启动信息：解析根节点直接子节点中的内存设备 `reg` 属性，并提供固件内存保留表。结果暂时只用于诊断，不启动物理页分配器。

`reg` 中的地址和长度由父节点的 `#address-cells` 与 `#size-cells` 决定；缺失时按规范建议采用 2 和 1。参见 [Devicetree 规范：标准属性](https://devicetree-specification.readthedocs.io/en/v0.2/devicetree-basics.html) 与 [设备节点要求](https://devicetree-specification.readthedocs.io/en/v0.2/devicenodes.html)。

## 接口与实现范围

- `BootInfo::parse` 消费 FDT 结构事件，提取内存范围并完整遍历结构块，因此同时触发结构格式校验。
- 内存节点通过 `device_type = "memory"` 或节点名 `memory` / `memory@...` 识别；当前只解释根节点的直接子节点。
- `reg` 支持 1 或 2 个地址 cell 与 1 或 2 个长度 cell，解码为 64 位物理地址和长度；检查项长度、零长度、加法溢出和范围重叠。
- `BootInfo::memory_regions` 返回固定容量的 32 个范围以内的切片。超过容量会返回错误，不分配堆内存。
- `BootInfo::reservations` 借用并迭代 DTB memory reservation block 中的固件保留范围。暂不解析 `/reserved-memory` 节点，也不在此阶段从 RAM 范围中扣除保留区域。
- 平台信息提取独立于通用 FDT 二进制格式解析；不解释 `/chosen`、CPU、UART 或其他设备 binding。

## 验证

```sh
make test-fdt
make build
make run
```

宿主机测试覆盖双 cell 多范围解码、规范默认 cell 数、缺失 `reg`、不完整项、重叠范围和不支持的 cell 数。QEMU `virt` 启动应报告从 DTB 读取的 RAM 起始地址 `0x80000000` 和大小 `0x08000000`，并完成结构块遍历。

## 当前限制

- 只识别根节点下的 RAM 节点；更复杂的总线地址转换和多级 `ranges` 映射不在本阶段处理。
- 地址和长度最多支持两个 32 位 cell；更宽的设备树整数明确拒绝。
- 内存区域容量为 32 项，便于早期内核在无堆环境中保存结果。
- 固件保留表与 `/reserved-memory` 是不同来源；目前仅保留前者，后者将在内存管理设计时单独解释。
- `BootInfo` 仅报告范围，尚未定义可用内存策略、内核映像占用区间或分配器元数据布局。
