# 第 51 阶段：VirtIO 块设备单扇区读取

## 目标关联

当前用户程序 ELF 在构建时嵌入内核，shell 只能从内嵌清单启动程序。后续要从持久化设备读取文件并执行 ELF，首先需要内核能访问块设备。本阶段建立从 QEMU 虚拟磁盘到内核内存的一条最小读取路径。

## 学习重点

- QEMU `virt` 通过 FDT 描述 VirtIO MMIO transport；内核据此找到候选寄存器范围，并读取设备 ID 识别块设备。
- VirtIO 驱动通过 MMIO 完成功能协商和队列配置，再用 split virtqueue 描述块读请求。
- 设备通过 DMA 直接访问内核提供的物理内存，因此队列与数据缓冲区的虚拟地址、物理地址必须明确区分。
- 当前使用轮询检查请求完成，便于先看清请求提交和完成协议；它占用 CPU，不适合作为最终的通用 I/O 路径。

## 实现范围

- 从 `/soc` 下提取启用的 `virtio,mmio` 节点 `reg` 范围，不硬编码 transport 地址。
- 探测现代 VirtIO MMIO 版本，并选择设备 ID 为 block 的 transport；只协商 `VIRTIO_F_VERSION_1`。
- 使用静态、页对齐的 DMA 队列和单扇区请求缓冲区，完成一个 512 字节扇区的读请求。
- QEMU 启动脚本生成并挂载一个 512 字节原始测试镜像；内核读取扇区 0 并核对固定签名。
- QEMU `virt` 默认将 VirtIO MMIO transport 设为 legacy；运行脚本显式关闭 `force-legacy`，让设备使用本阶段实现的现代 MMIO v2 接口。
- 驱动只在启动阶段、启动进程地址空间之前运行；请求以轮询方式串行处理。

## 暂不实现

- 写请求、多个未完成请求、队列动态分配、磁盘缓存、设备中断和多 hart 同步。
- 在进程页表中映射 VirtIO MMIO 范围，或从用户 syscall 路径访问块设备。
- 分区、文件系统、目录、文件描述符和从磁盘装载 ELF。
- 旧版 VirtIO MMIO transport、PCI transport、IOMMU 和非 QEMU `virt` 平台。

## 模块边界

`device_tree::virtio_info` 只解析 FDT 并返回 transport 物理范围；`startup` 把范围和静态 DMA 区域的物理地址交给块设备驱动。`driver::virtio::mmio` 负责通用 transport 寄存器访问，块设备驱动编排设备队列寄存器配置和通知；`driver::virtio::virtqueue` 负责 split virtqueue 描述符、环形区和请求完成轮询，`driver::virtio::block` 负责块请求格式和扇区数据处理。启动页表当前提供本实验使用的低地址 MMIO 恒等映射，驱动不直接依赖 FDT、页帧分配器或内核地址转换模块。

## 验收方式

```sh
make build
make run
```

QEMU 启动后应报告发现并初始化 VirtIO 块设备，读取扇区 0，并显示与生成的测试镜像一致的 `FULLEMU-BLOCK-TEST` 签名。移除 QEMU 块设备或让读取请求失败时，启动检查应明确报错。此验证只证明单次设备读取链路，不代表已支持块设备上的文件系统或 Linux 文件 I/O。

## 后续连接

第 52 阶段在该单扇区读取原型上增加块设备对象和扇区读写接口，为后续文件系统消费提供边界。只有文件系统能从设备读取数据后，才把现有 ELF 装载器的镜像来源从内嵌清单切换到文件路径。
