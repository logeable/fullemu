//! 遍历 Sv39 页表并输出连续虚拟/物理映射范围，供启动调试使用。

use super::address::{canonicalize_sv39, physical_to_kernel_virtual};
use super::address_space::AddressSpace;
use super::page::is_page_aligned;
use super::page_table::{
    PageTable, PAGE_TABLE_ENTRIES, PTE_EXECUTE, PTE_READ, PTE_USER, PTE_VALID, PTE_WRITE,
};

/// 以调试日志输出指定进程页表中的有效映射。
///
/// 连续虚拟页映射到连续物理页且权限相同的条目会合并为一个范围。
pub(crate) fn log_address_space(process_id: usize, address_space: &AddressSpace) {
    if !crate::logging::is_enabled(crate::logging::Level::Debug) {
        return;
    }

    let root_address = address_space.root_address();
    if !crate::frame_allocator::is_allocated_frame_address(root_address) {
        crate::klog_error!(
            "Sv39 页表无效：进程={}，根页表地址 {root_address:#018x} 不是已分配页帧",
            process_id
        );
        return;
    }
    crate::klog_debug!(
        "Sv39 页表：进程={}，根页表物理地址={:#018x}",
        process_id,
        root_address
    );

    let Some(root_virtual_address) = physical_to_kernel_virtual(root_address) else {
        crate::klog_error!(
            "Sv39 页表无效：进程={}，根页表无法映射到内核高半区",
            process_id
        );
        return;
    };

    let mut mapping_run = None;
    walk_page_table(
        root_virtual_address as *const PageTable,
        2,
        0,
        process_id,
        &mut mapping_run,
    );
    flush_mapping_run(process_id, mapping_run);
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
    process_id: usize,
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
                process_id,
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
                "Sv39 页表无效：进程={}，最低级页表项不是叶子映射，虚拟页号={:#x}",
                process_id,
                next_virtual_page_number
            );
            continue;
        }

        let child_table_address = ((entry >> 10) << 12) as usize;
        if !is_page_aligned(child_table_address)
            || !crate::frame_allocator::is_allocated_frame_address(child_table_address)
        {
            crate::klog_error!(
                "Sv39 页表无效：进程={}，子页表地址 {child_table_address:#018x} 不是已分配页帧",
                process_id
            );
            continue;
        }

        let Some(child_virtual_address) = physical_to_kernel_virtual(child_table_address) else {
            crate::klog_error!(
                "Sv39 页表无效：进程={}，子页表无法映射到内核高半区",
                process_id
            );
            continue;
        };
        walk_page_table(
            child_virtual_address as *const PageTable,
            level - 1,
            next_virtual_page_number,
            process_id,
            mapping_run,
        );
    }
}

fn record_mapping(
    process_id: usize,
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

    flush_mapping_run(process_id, *mapping_run);
    *mapping_run = Some(next_mapping);
}

fn flush_mapping_run(process_id: usize, mapping_run: Option<MappingRun>) {
    let Some(mapping) = mapping_run else {
        return;
    };

    crate::klog_debug!(
        "页表映射：进程={}，虚拟={:#018x}..{:#018x}，物理={:#018x}..{:#018x}，权限={}{}{}{}",
        process_id,
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
