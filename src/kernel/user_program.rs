//! 管理批量执行所需的独立用户程序镜像，并逐个复制到固定加载区。

struct UserProgramImage {
    name: &'static str,
    bytes: &'static [u8],
}

const USER_PROGRAMS: &[UserProgramImage] = &[
    UserProgramImage {
        name: "fullemu_user",
        bytes: include_bytes!(
            "../../user/target/riscv64gc-unknown-none-elf/release/fullemu_user.bin"
        ),
    },
    UserProgramImage {
        name: "fullemu_user_stderr",
        bytes: include_bytes!(
            "../../user/target/riscv64gc-unknown-none-elf/release/fullemu_user_stderr.bin"
        ),
    },
    UserProgramImage {
        name: "fullemu_user_syscall_error",
        bytes: include_bytes!(
            "../../user/target/riscv64gc-unknown-none-elf/release/fullemu_user_syscall_error.bin"
        ),
    },
];

static mut CURRENT_IMAGE_SIZE: usize = 0;

extern "C" {
    static __user_program_load_start: u8;
    static __user_program_load_end: u8;
}

/// 已复制到内存中的用户程序入口和镜像信息。
pub struct LoadedUserProgram {
    /// 用户程序在批次清单中的名称。
    pub name: &'static str,
    /// 用户程序入口地址。
    pub entry: usize,
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
}

impl UserProgramLoadError {
    /// 返回用于启动诊断的错误说明。
    pub fn description(&self) -> &'static str {
        "用户程序镜像超过固定加载区容量"
    }
}

/// 按批次索引加载用户程序；索引超出清单时表示批次结束。
pub fn load(index: usize) -> Result<Option<LoadedUserProgram>, UserProgramLoadError> {
    let Some(program) = USER_PROGRAMS.get(index) else {
        return Ok(None);
    };

    // 链接器将这两个符号定义为加载区首尾地址，而非存放数据的变量。
    let load_start = core::ptr::addr_of!(__user_program_load_start) as usize;
    let load_end = core::ptr::addr_of!(__user_program_load_end) as usize;
    let capacity = load_end - load_start;

    if program.bytes.len() > capacity {
        return Err(UserProgramLoadError {
            program_name: program.name,
            image_size: program.bytes.len(),
            capacity,
        });
    }

    // 安全性：链接脚本保证加载区位于内核映像之后且落在 QEMU RAM 内；
    // 编译期嵌入的源镜像与目标区不重叠，镜像长度已在复制前和区域容量比较。
    unsafe {
        core::ptr::write_bytes(load_start as *mut u8, 0, capacity);
        core::ptr::copy_nonoverlapping(
            program.bytes.as_ptr(),
            load_start as *mut u8,
            program.bytes.len(),
        );
        core::arch::asm!("fence.i", options(nostack));
        // 安全性：加载仅在任务启动或切换前执行，当前单 hart 没有其他读取该字段的执行流。
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(CURRENT_IMAGE_SIZE),
            program.bytes.len(),
        );
    }

    Ok(Some(LoadedUserProgram {
        name: program.name,
        entry: load_start,
        image_size: program.bytes.len(),
    }))
}

/// 判断缓冲区是否完全落在当前已加载的用户程序镜像中。
pub fn contains_buffer_range(address: usize, length: usize) -> bool {
    if length == 0 {
        return true;
    }

    let image_start = core::ptr::addr_of!(__user_program_load_start) as usize;
    // 安全性：加载器在运行用户程序前写入当前镜像长度；系统调用时用户程序已暂停。
    let image_size = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(CURRENT_IMAGE_SIZE)) };
    let Some(image_end) = image_start.checked_add(image_size) else {
        return false;
    };
    let Some(buffer_end) = address.checked_add(length) else {
        return false;
    };

    address >= image_start && buffer_end <= image_end
}
