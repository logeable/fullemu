//! 定义内核使用的基础页大小和对齐操作。

/// RISC-V Sv39 基础页大小，单位为字节。
pub(crate) const PAGE_SIZE: usize = 4096;

/// 用于取得页内偏移的掩码。
pub(crate) const PAGE_MASK: usize = PAGE_SIZE - 1;

/// 判断地址是否按基础页对齐。
pub(crate) const fn is_page_aligned(address: usize) -> bool {
    address & PAGE_MASK == 0
}

/// 将地址向下对齐到基础页边界。
pub(crate) const fn align_down_to_page(address: usize) -> usize {
    address & !PAGE_MASK
}

/// 将地址向上对齐到基础页边界；结果溢出时返回 `None`。
pub(crate) const fn align_up_to_page(address: usize) -> Option<usize> {
    match address.checked_add(PAGE_MASK) {
        Some(value) => Some(value & !PAGE_MASK),
        None => None,
    }
}
