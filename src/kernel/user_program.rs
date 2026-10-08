//! 管理同时驻留的用户程序镜像，并复制到各自固定的物理加载槽位。

const USER_PROGRAM_SLOT_SIZE: usize = 64 * 1024;
const USER_PROGRAM_VIRTUAL_BASE: usize = 0x8040_0000;

struct UserProgramImage {
    name: &'static str,
    linked_base: usize,
    bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/user_program_catalog.rs"));

extern "C" {
    static __user_program_load_start: u8;
    static __user_program_load_end: u8;
}

/// 已复制到内存中的用户程序入口和镜像信息。
pub struct LoadedUserProgram {
    /// 用户程序在构建清单中的名称。
    pub name: &'static str,
    /// 用户程序入口地址。
    pub entry: usize,
    /// 用户程序链接时使用的固定虚拟起始地址。
    pub virtual_start: usize,
    /// 用户程序固定加载槽位的起始地址。
    pub slot_start: usize,
    /// 用户程序固定加载槽位的结束地址。
    pub slot_end: usize,
    /// 从独立构建产物复制的字节数。
    pub image_size: usize,
}

/// 固定加载区容量不足时返回该错误。
pub struct UserProgramLoadError {
    /// 无法装入的用户程序名称。
    pub program_name: &'static str,
    /// 用户程序镜像实际长度。
    pub image_size: usize,
    /// 链接脚本预留的加载区长度。
    pub capacity: usize,
    description: &'static str,
}

impl UserProgramLoadError {
    /// 返回用于启动诊断的错误说明。
    pub fn description(&self) -> &'static str {
        self.description
    }
}

/// 按构建清单名称查找程序索引。
pub fn find_index(name: &str) -> Option<usize> {
    USER_PROGRAMS
        .iter()
        .position(|program| program.name == name)
}

/// 返回构建清单中的用户程序数量。
pub fn count() -> usize {
    USER_PROGRAMS.len()
}

/// 将指定程序装入独立槽位；索引超出清单时表示没有该程序。
pub fn load(index: usize) -> Result<Option<LoadedUserProgram>, UserProgramLoadError> {
    let Some(program) = USER_PROGRAMS.get(index) else {
        return Ok(None);
    };

    // 链接器将这两个符号定义为加载区首尾地址，而非存放数据的变量。
    let load_start = core::ptr::addr_of!(__user_program_load_start) as usize;
    let load_end = core::ptr::addr_of!(__user_program_load_end) as usize;
    let reserved_capacity = load_end - load_start;
    let capacity = USER_PROGRAM_SLOT_SIZE;
    let Some(slot_offset) = index.checked_mul(USER_PROGRAM_SLOT_SIZE) else {
        return Err(UserProgramLoadError {
            program_name: program.name,
            image_size: program.bytes.len(),
            capacity: 0,
            description: "用户程序槽位地址计算溢出",
        });
    };
    let Some(slot_start) = load_start.checked_add(slot_offset) else {
        return Err(UserProgramLoadError {
            program_name: program.name,
            image_size: program.bytes.len(),
            capacity: 0,
            description: "用户程序槽位地址计算溢出",
        });
    };
    let Some(slot_end) = slot_start.checked_add(capacity) else {
        return Err(UserProgramLoadError {
            program_name: program.name,
            image_size: program.bytes.len(),
            capacity: 0,
            description: "用户程序槽位结束地址计算溢出",
        });
    };

    if program.bytes.len() > capacity || slot_end > load_end || reserved_capacity < capacity {
        return Err(UserProgramLoadError {
            program_name: program.name,
            image_size: program.bytes.len(),
            capacity: core::cmp::min(capacity, reserved_capacity.saturating_sub(slot_offset)),
            description: "用户程序镜像超过固定加载槽位容量",
        });
    }
    if program.linked_base != USER_PROGRAM_VIRTUAL_BASE {
        return Err(UserProgramLoadError {
            program_name: program.name,
            image_size: program.bytes.len(),
            capacity,
            description: "用户程序没有链接到统一的虚拟起始地址",
        });
    }
    // 安全性：链接脚本保证加载区位于内核映像之后且落在 QEMU RAM 内；
    // 编译期嵌入的源镜像与目标物理槽位不重叠，镜像长度和槽位边界已在复制前检查。
    unsafe {
        core::ptr::write_bytes(slot_start as *mut u8, 0, capacity);
        core::ptr::copy_nonoverlapping(
            program.bytes.as_ptr(),
            slot_start as *mut u8,
            program.bytes.len(),
        );
        core::arch::asm!("fence.i", options(nostack));
    }

    Ok(Some(LoadedUserProgram {
        name: program.name,
        entry: program.linked_base,
        virtual_start: program.linked_base,
        slot_start,
        slot_end,
        image_size: program.bytes.len(),
    }))
}

/// 判断缓冲区是否完全落在指定用户程序的镜像槽位中。
pub fn contains_buffer_range(program_index: usize, address: usize, length: usize) -> bool {
    if length == 0 {
        return true;
    }

    let Some(program) = USER_PROGRAMS.get(program_index) else {
        return false;
    };
    let Some(image_end) = program.linked_base.checked_add(program.bytes.len()) else {
        return false;
    };
    let Some(buffer_end) = address.checked_add(length) else {
        return false;
    };

    address >= program.linked_base && buffer_end <= image_end
}
