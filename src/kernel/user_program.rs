//! 将独立构建的用户程序原始镜像复制到固定加载区。

const USER_PROGRAM_IMAGE: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/fullemu_user.bin");

extern "C" {
    static __user_program_load_start: u8;
    static __user_program_load_end: u8;
}

/// 已复制到内存中的用户程序入口和镜像长度。
pub struct LoadedUserProgram {
    /// 用户程序入口地址。
    pub entry: usize,
    /// 从独立构建产物复制的字节数。
    pub image_size: usize,
}

/// 固定加载区容量不足时返回该错误。
pub struct UserProgramLoadError {
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

/// 将独立用户程序镜像复制到链接脚本指定的物理内存区。
pub fn load() -> Result<LoadedUserProgram, UserProgramLoadError> {
    // 链接器将这两个符号定义为加载区首尾地址，而非存放数据的变量。
    let load_start = core::ptr::addr_of!(__user_program_load_start) as usize;
    let load_end = core::ptr::addr_of!(__user_program_load_end) as usize;
    let capacity = load_end - load_start;

    if USER_PROGRAM_IMAGE.len() > capacity {
        return Err(UserProgramLoadError {
            image_size: USER_PROGRAM_IMAGE.len(),
            capacity,
        });
    }

    // 安全性：链接脚本保证加载区位于内核映像之后且落在 QEMU RAM 内；
    // 编译期嵌入的源镜像与目标区不重叠，镜像长度已在复制前和区域容量比较。
    unsafe {
        core::ptr::write_bytes(load_start as *mut u8, 0, capacity);
        core::ptr::copy_nonoverlapping(
            USER_PROGRAM_IMAGE.as_ptr(),
            load_start as *mut u8,
            USER_PROGRAM_IMAGE.len(),
        );
        core::arch::asm!("fence.i", options(nostack));
    }

    Ok(LoadedUserProgram {
        entry: load_start,
        image_size: USER_PROGRAM_IMAGE.len(),
    })
}

/// 判断缓冲区是否完全落在当前固定加载的用户程序镜像中。
pub fn contains_buffer_range(address: usize, length: usize) -> bool {
    if length == 0 {
        return true;
    }

    let image_start = core::ptr::addr_of!(__user_program_load_start) as usize;
    let Some(image_end) = image_start.checked_add(USER_PROGRAM_IMAGE.len()) else {
        return false;
    };
    let Some(buffer_end) = address.checked_add(length) else {
        return false;
    };

    address >= image_start && buffer_end <= image_end
}
