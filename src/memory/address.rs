//! 定义 Sv39 地址布局和内核直接映射中的地址换算。

/// Sv39 低半部的结束地址，也是用户地址空间的上界（不包含）。
pub(crate) const SV39_USER_ADDRESS_END_EXCLUSIVE: usize = 1usize << 38;

/// 内核直接映射的虚拟基址；物理地址 `p` 对应 `KERNEL_DIRECT_MAP_BASE + p`。
pub(crate) const KERNEL_DIRECT_MAP_BASE: usize = 0xffff_ffc0_0000_0000;

/// 将 Sv39 页内索引编码还原为规范的 64 位虚拟地址。
pub(crate) const fn canonicalize_sv39(address: usize) -> usize {
    const VIRTUAL_ADDRESS_MASK: usize = (1usize << 39) - 1;

    if address & (1usize << 38) == 0 {
        address & VIRTUAL_ADDRESS_MASK
    } else {
        address | !VIRTUAL_ADDRESS_MASK
    }
}

/// 判断地址是否符合 Sv39 的符号扩展规则。
pub(crate) const fn is_sv39_canonical(address: usize) -> bool {
    let sign_bit = (address >> 38) & 1;
    let upper_bits = address >> 39;
    if sign_bit == 0 {
        upper_bits == 0
    } else {
        upper_bits == (1usize << 25) - 1
    }
}

/// 将物理 RAM 地址换算为内核高半区直接映射地址。
///
/// 调用方仍须保证该物理地址位于已映射的 RAM 范围内。
pub(crate) const fn physical_to_kernel_virtual(physical_address: usize) -> Option<usize> {
    let Some(virtual_address) = KERNEL_DIRECT_MAP_BASE.checked_add(physical_address) else {
        return None;
    };

    if is_sv39_canonical(virtual_address) {
        Some(virtual_address)
    } else {
        None
    }
}

/// 将内核映像高半区地址换算为链接器安排的物理装载地址。
pub(crate) const fn kernel_virtual_to_physical(virtual_address: usize) -> usize {
    virtual_address - KERNEL_DIRECT_MAP_BASE
}
