//! 用户程序使用的最小 Linux RISC-V 系统调用封装。

const LINUX_WRITE_SYSCALL: usize = 64;
const LINUX_READ_SYSCALL: usize = 63;
const LINUX_CLOCK_GETTIME_SYSCALL: usize = 113;
const LINUX_EXIT_SYSCALL: usize = 93;
const LINUX_CLONE_SYSCALL: usize = 220;
const LINUX_EXECVE_SYSCALL: usize = 221;
const LINUX_WAIT4_SYSCALL: usize = 260;
const LINUX_BRK_SYSCALL: usize = 214;
const LINUX_OPENAT_SYSCALL: usize = 56;
const LINUX_CLOSE_SYSCALL: usize = 57;
const LINUX_GETDENTS64_SYSCALL: usize = 61;
const LINUX_MKDIRAT_SYSCALL: usize = 34;
const LINUX_UNLINKAT_SYSCALL: usize = 35;
const LINUX_SIGCHLD: usize = 17;
const CLOCK_MONOTONIC: usize = 1;
const LINUX_AT_FDCWD: isize = -100;
/// 不因 `O_RDONLY` 权限位改变打开方式。
pub const O_RDONLY: usize = 0;
/// 以只写方式打开。
pub const O_WRONLY: usize = 1;
/// 以读写方式打开。
pub const O_RDWR: usize = 2;
/// 文件不存在时创建。
pub const O_CREAT: usize = 0x40;
/// 与 `O_CREAT` 同用时要求文件原先不存在。
pub const O_EXCL: usize = 0x80;
/// 打开时清空现有普通文件。
pub const O_TRUNC: usize = 0x200;
/// 写入时追加到文件末尾。
pub const O_APPEND: usize = 0x400;
/// 要求目标为目录。
pub const O_DIRECTORY: usize = 0x10000;
const EXEC_PROGRAM_NAME_BUFFER_SIZE: usize = 128;
const LINUX_ENAMETOOLONG: isize = 36;
const LINUX_E2BIG: isize = 7;
const LINUX_EINVAL: isize = 22;
const PATH_BUFFER_SIZE: usize = 256;
const MAX_EXEC_ARGUMENT_COUNT: usize = 32;
const MAX_EXEC_ARGUMENT_BYTES: usize = 4096;

/// Linux RISC-V 64 位 ABI 的单调时间结构。
#[repr(C)]
pub struct Timespec {
    /// 完整秒数。
    pub seconds: i64,
    /// 不足一秒的纳秒数。
    pub nanoseconds: i64,
}

/// 从指定文件描述符读取字节，并返回读取字节数或负的 Linux errno。
pub fn read(file_descriptor: usize, bytes: &mut [u8]) -> isize {
    let mut result = file_descriptor as isize;

    // 安全性：调用约定遵循 Linux RISC-V syscall ABI；内核只允许写入当前任务栈缓冲区。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") bytes.as_mut_ptr() as usize,
            in("a2") bytes.len(),
            in("a7") LINUX_READ_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 请求内核把字节写到指定文件描述符，并返回写入字节数或负的 Linux errno。
pub fn write(file_descriptor: usize, bytes: &[u8]) -> isize {
    let mut result = file_descriptor as isize;

    // 安全性：寄存器和调用指令遵循 Linux RISC-V syscall ABI；内核负责检查缓冲区并返回结果。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") bytes.as_ptr() as usize,
            in("a2") bytes.len(),
            in("a7") LINUX_WRITE_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 读取 `CLOCK_MONOTONIC`，结果表示从内核启动计时起点经过的时间。
pub fn clock_gettime(time: &mut Timespec) -> isize {
    let mut result = CLOCK_MONOTONIC as isize;

    // 安全性：时间结构位于当前用户栈；调用约定遵循 Linux RISC-V syscall ABI。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") time as *mut Timespec as usize,
            in("a7") LINUX_CLOCK_GETTIME_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 查询或设置当前进程的 program break；失败时原始 Linux syscall 返回旧边界。
pub fn brk(address: usize) -> usize {
    let mut result = address;

    // 安全性：参数按 Linux RISC-V ABI 放入 a0，系统调用号 214 放入 a7。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a7") LINUX_BRK_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 打开相对根目录解析的文件或目录，返回文件描述符或负的 Linux errno。
pub fn open(path: &str, flags: usize, mode: usize) -> isize {
    let mut path_buffer = [0; PATH_BUFFER_SIZE];
    if let Err(error) = copy_path_to_buffer(path, &mut path_buffer) {
        return error;
    }
    let mut result = LINUX_AT_FDCWD;

    // 安全性：路径缓冲区在当前用户栈中且以 NUL 结尾；寄存器按 Linux RISC-V openat ABI 传递。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") path_buffer.as_ptr() as usize,
            in("a2") flags,
            in("a3") mode,
            in("a7") LINUX_OPENAT_SYSCALL,
            options(nostack)
        );
    }
    result
}

/// 关闭文件描述符；失败时返回负的 Linux errno。
pub fn close(file_descriptor: usize) -> isize {
    let mut result = file_descriptor as isize;

    // 安全性：文件描述符和系统调用号按 Linux RISC-V close ABI 传递。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a7") LINUX_CLOSE_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 从目录描述符读取 Linux `dirent64` 记录；本阶段仅支持传入有效的用户缓冲区。
pub fn getdents64(file_descriptor: usize, buffer: &mut [u8]) -> isize {
    let mut result = file_descriptor as isize;

    // 安全性：输出切片位于当前用户地址空间，内核仍会验证整个缓冲区范围。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") buffer.as_mut_ptr() as usize,
            in("a2") buffer.len(),
            in("a7") LINUX_GETDENTS64_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 创建一个目录；`mode` 按 Linux 接口传入，当前文件系统尚不保存权限位。
pub fn mkdir(path: &str, mode: usize) -> isize {
    let mut path_buffer = [0; PATH_BUFFER_SIZE];
    if let Err(error) = copy_path_to_buffer(path, &mut path_buffer) {
        return error;
    }
    let mut result = LINUX_AT_FDCWD;

    // 安全性：路径缓冲区在当前用户栈中且以 NUL 结尾；参数按 Linux RISC-V mkdirat ABI 传递。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") path_buffer.as_ptr() as usize,
            in("a2") mode,
            in("a7") LINUX_MKDIRAT_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 删除普通文件；本阶段不支持删除目录。
pub fn unlink(path: &str) -> isize {
    let mut path_buffer = [0; PATH_BUFFER_SIZE];
    if let Err(error) = copy_path_to_buffer(path, &mut path_buffer) {
        return error;
    }
    let mut result = LINUX_AT_FDCWD;

    // 安全性：路径缓冲区在当前用户栈中且以 NUL 结尾；标志为零表示 unlinkat 普通文件删除。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") path_buffer.as_ptr() as usize,
            in("a2") 0usize,
            in("a7") LINUX_UNLINKAT_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 把路径编码为以 NUL 结尾的固定缓冲区，拒绝过长或包含内部 NUL 的字符串。
fn copy_path_to_buffer(path: &str, buffer: &mut [u8; PATH_BUFFER_SIZE]) -> Result<(), isize> {
    if path.is_empty() || path.as_bytes().contains(&0) {
        return Err(-LINUX_EINVAL);
    }
    if path.len() >= buffer.len() {
        return Err(-LINUX_ENAMETOOLONG);
    }
    buffer[..path.len()].copy_from_slice(path.as_bytes());
    buffer[path.len()] = 0;
    Ok(())
}

/// 请求内核以给定状态码结束当前用户任务；符合 Linux ABI 时不会返回。
pub fn exit(status: i32) -> ! {
    // 安全性：状态码按 Linux `int` 参数传入 `a0`，系统调用号 93 放入 `a7`。
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a0") status as isize,
            in("a7") LINUX_EXIT_SYSCALL,
            options(nostack)
        );
    }

    // 若内核错误地让 exit 返回，用户代码也不能继续越过不可返回的 ABI 边界。
    loop {
        core::hint::spin_loop();
    }
}

/// 主动让出 CPU，并在再次获得调度时从系统调用之后继续执行。
// 多个独立二进制共享此封装，只有需要协作切换的程序会调用它。
#[allow(dead_code)]
pub fn sched_yield() -> isize {
    let mut result = 0isize;
    let syscall_number = 124;

    // 安全性：寄存器和调用指令遵循 Linux RISC-V syscall ABI；内核负责选择下一个就绪任务。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a7") syscall_number,
            options(nostack)
        );
    }

    result
}

/// 复制当前进程；父进程得到子进程 PID，子进程得到 0，失败时返回负的 Linux errno。
pub fn fork() -> isize {
    let mut result = LINUX_SIGCHLD as isize;

    // 安全性：只请求带 SIGCHLD 退出通知的独立进程；其余 clone 参数按 fork 约定传零。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") 0usize,
            in("a2") 0usize,
            in("a3") 0usize,
            in("a4") 0usize,
            in("a7") LINUX_CLONE_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 按程序名替换当前进程；程序名作为 `argv[0]`，其余切片依次作为参数。
///
/// 参数字符串和总参数数据量受内核栈大小限制；成功后不返回，失败返回负的 Linux errno。
pub fn exec(program_name: &str, arguments: &[&str]) -> isize {
    if program_name.len() >= EXEC_PROGRAM_NAME_BUFFER_SIZE {
        return -LINUX_ENAMETOOLONG;
    }
    if arguments.len() + 1 > MAX_EXEC_ARGUMENT_COUNT {
        return -LINUX_E2BIG;
    }

    let mut name = [0u8; EXEC_PROGRAM_NAME_BUFFER_SIZE];
    name[..program_name.len()].copy_from_slice(program_name.as_bytes());
    let mut argument_bytes = [0u8; MAX_EXEC_ARGUMENT_BYTES];
    let mut argument_pointers = [0usize; MAX_EXEC_ARGUMENT_COUNT + 1];
    argument_pointers[0] = name.as_ptr() as usize;
    let mut bytes_used = 0usize;
    for (argument_index, argument) in arguments.iter().enumerate() {
        if argument.as_bytes().contains(&0) {
            return -LINUX_EINVAL;
        }
        let Some(argument_size) = argument.len().checked_add(1) else {
            return -LINUX_E2BIG;
        };
        let Some(argument_end) = bytes_used.checked_add(argument_size) else {
            return -LINUX_E2BIG;
        };
        if argument_end > argument_bytes.len() {
            return -LINUX_E2BIG;
        }

        argument_pointers[argument_index + 1] = argument_bytes[bytes_used..].as_ptr() as usize;
        argument_bytes[bytes_used..bytes_used + argument.len()]
            .copy_from_slice(argument.as_bytes());
        argument_bytes[bytes_used + argument.len()] = 0;
        bytes_used = argument_end;
    }

    let mut result = name.as_mut_ptr() as isize;
    // 安全性：程序名、参数字符串和以空指针结尾的 argv 都位于当前用户栈；内核会在切换地址空间前复制它们。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") argument_pointers.as_ptr() as usize,
            in("a2") 0usize,
            in("a7") LINUX_EXECVE_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 等待指定子进程或任意子进程，阻塞直到其退出并可选地取得 wait 状态。
pub fn waitpid(pid: i32, status: Option<&mut i32>) -> isize {
    let mut result = pid as isize;
    let status_address = status.map_or(0, |status| status as *mut i32 as usize);

    // 安全性：状态指针在调用期间指向当前用户栈中的可写 `i32`，其他参数符合 Linux wait4 ABI。
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") result,
            in("a1") status_address,
            in("a2") 0usize,
            in("a3") 0usize,
            in("a7") LINUX_WAIT4_SYSCALL,
            options(nostack)
        );
    }

    result
}

/// 等待任意一个子进程；这是 `waitpid(-1, status)` 的便捷封装。
pub fn wait(status: Option<&mut i32>) -> isize {
    waitpid(-1, status)
}
