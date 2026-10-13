//! 遍历 Sv39 页表并输出连续虚拟/物理映射范围，供启动调试使用。

use super::{
    address_space_count, address_space_root, canonicalize_sv39, PageTable, PAGE_MASK,
    PAGE_TABLE_ENTRIES, PTE_EXECUTE, PTE_READ, PTE_USER, PTE_VALID, PTE_WRITE,
};

/// 以调试日志输出每个任务页表中的有效映射。
///
/// 连续虚拟页映射到连续物理页且权限相同的条目会合并为一个范围。
pub(super) fn log_address_spaces() {
    if !crate::kernel::logging::is_enabled(crate::kernel::logging::Level::Debug) {
        return;
    }

    for address_space_index in 0..address_space_count() {
        let Some(root_address) = address_space_root(address_space_index) else {
            continue;
        };
        if !crate::kernel::frame_allocator::is_allocated_frame_address(root_address) {
            crate::klog_error!(
                "Sv39 页表无效：任务={}，根页表地址 {root_address:#018x} 不是已分配页帧",
                address_space_index
            );
            continue;
        }
        crate::klog_debug!(
            "Sv39 页表：任务={}，根页表物理地址={:#018x}",
            address_space_index,
            root_address
        );

        let mut mapping_run = None;
        walk_page_table(
            root_address as *const PageTable,
            2,
            0,
            address_space_index,
            &mut mapping_run,
        );
        flush_mapping_run(address_space_index, mapping_run);
    }
}

#[derive(Clone, Copy)]
struct MappingRun {
    virtual_start: usize,
    physical_start: usize,
    size: usize,
    permissions: usize,
}

fn walk_page_table(
    table: *const PageTable,
    level: usize,
    virtual_page_number: usize,
    address_space_index: usize,
    mapping_run: &mut Option<MappingRun>,
) {
    for index in 0..PAGE_TABLE_ENTRIES {
        // 安全性：当前表是根页帧或已验证的已分配子页帧，索引由 512 项表长度限制。
        let entry = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*table).0[index])) };
        if entry & PTE_VALID == 0 {
            continue;
        }

        let next_virtual_page_number = (virtual_page_number << 9) | index;
        if entry & (PTE_READ | PTE_WRITE | PTE_EXECUTE) != 0 {
            let page_shift = 12 + level * 9;
            let mapping_size = 1usize << page_shift;
            let virtual_start = canonicalize_sv39(next_virtual_page_number << page_shift);
            let physical_start = ((entry >> 10) << 12) & !(mapping_size - 1);
            record_mapping(
                address_space_index,
                MappingRun {
                    virtual_start,
                    physical_start,
                    size: mapping_size,
                    permissions: entry & (PTE_USER | PTE_READ | PTE_WRITE | PTE_EXECUTE),
                },
                mapping_run,
            );
            continue;
        }

        if level == 0 {
            crate::klog_error!(
                "Sv39 页表无效：任务={}，最低级页表项不是叶子映射，虚拟页号={:#x}",
                address_space_index,
                next_virtual_page_number
            );
            continue;
        }

        let child_table_address = ((entry >> 10) << 12) as usize;
        if child_table_address & PAGE_MASK != 0
            || !crate::kernel::frame_allocator::is_allocated_frame_address(child_table_address)
        {
            crate::klog_error!(
                "Sv39 页表无效：任务={}，子页表地址 {child_table_address:#018x} 不是已分配页帧",
                address_space_index
            );
            continue;
        }

        walk_page_table(
            child_table_address as *const PageTable,
            level - 1,
            next_virtual_page_number,
            address_space_index,
            mapping_run,
        );
    }
}

fn record_mapping(
    address_space_index: usize,
    next_mapping: MappingRun,
    mapping_run: &mut Option<MappingRun>,
) {
    if let Some(current) = mapping_run.as_mut() {
        let virtual_end = current.virtual_start + current.size;
        let physical_end = current.physical_start + current.size;
        if virtual_end == next_mapping.virtual_start
            && physical_end == next_mapping.physical_start
            && current.permissions == next_mapping.permissions
        {
            current.size += next_mapping.size;
            return;
        }
    }

    flush_mapping_run(address_space_index, *mapping_run);
    *mapping_run = Some(next_mapping);
}

fn flush_mapping_run(address_space_index: usize, mapping_run: Option<MappingRun>) {
    let Some(mapping) = mapping_run else {
        return;
    };

    crate::klog_debug!(
        "页表映射：任务={}，虚拟={:#018x}..{:#018x}，物理={:#018x}..{:#018x}，权限={}{}{}{}",
        address_space_index,
        mapping.virtual_start,
        mapping.virtual_start + mapping.size,
        mapping.physical_start,
        mapping.physical_start + mapping.size,
        if mapping.permissions & PTE_USER != 0 {
            "U"
        } else {
            "S"
        },
        if mapping.permissions & PTE_READ != 0 {
            "R"
        } else {
            "-"
        },
        if mapping.permissions & PTE_WRITE != 0 {
            "W"
        } else {
            "-"
        },
        if mapping.permissions & PTE_EXECUTE != 0 {
            "X"
        } else {
            "-"
        }
    );
}
