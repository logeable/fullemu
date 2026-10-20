//! 集中定义 Linux RISC-V 系统调用编号、标志和错误码。

pub(super) const LINUX_WRITE_SYSCALL: usize = 64;
pub(super) const LINUX_READ_SYSCALL: usize = 63;
pub(super) const LINUX_CLOCK_GETTIME_SYSCALL: usize = 113;
pub(super) const LINUX_EXIT_SYSCALL: usize = 93;
pub(super) const LINUX_SCHED_YIELD_SYSCALL: usize = 124;
pub(super) const LINUX_CLONE_SYSCALL: usize = 220;
pub(super) const LINUX_EXECVE_SYSCALL: usize = 221;
pub(super) const LINUX_WAIT4_SYSCALL: usize = 260;
pub(super) const LINUX_BRK_SYSCALL: usize = 214;
pub(super) const LINUX_MKDIRAT_SYSCALL: usize = 34;
pub(super) const LINUX_UNLINKAT_SYSCALL: usize = 35;
pub(super) const LINUX_OPENAT_SYSCALL: usize = 56;
pub(super) const LINUX_CLOSE_SYSCALL: usize = 57;
pub(super) const LINUX_GETDENTS64_SYSCALL: usize = 61;
pub(super) const LINUX_SIGCHLD: usize = 17;
pub(super) const LINUX_WNOHANG: usize = 1;
pub(super) const LINUX_AT_FDCWD: usize = (-100isize) as usize;
pub(super) const LINUX_O_ACCMODE: usize = 3;
pub(super) const LINUX_O_RDONLY: usize = 0;
pub(super) const LINUX_O_WRONLY: usize = 1;
pub(super) const LINUX_O_RDWR: usize = 2;
pub(super) const LINUX_O_CREAT: usize = 0x40;
pub(super) const LINUX_O_EXCL: usize = 0x80;
pub(super) const LINUX_O_TRUNC: usize = 0x200;
pub(super) const LINUX_O_APPEND: usize = 0x400;
pub(super) const LINUX_O_DIRECTORY: usize = 0x10000;
pub(super) const LINUX_AT_REMOVEDIR: usize = 0x200;
pub(super) const STDIN_FILE_DESCRIPTOR: usize = 0;
pub(super) const STDOUT_FILE_DESCRIPTOR: usize = 1;
pub(super) const STDERR_FILE_DESCRIPTOR: usize = 2;
pub(super) const CLOCK_MONOTONIC: usize = 1;
pub(super) const MAX_EXEC_PROGRAM_NAME_LENGTH: usize = 128;
pub(super) const MAX_EXEC_ARGUMENT_COUNT: usize = 32;
pub(super) const MAX_EXEC_ARGUMENT_BYTES: usize = 4096;
pub(super) const MAX_PATH_LENGTH: usize = 256;
pub(super) const MAX_FILE_IO_CHUNK: usize = 128;

#[derive(Clone, Copy)]
#[repr(isize)]
pub(super) enum LinuxErrno {
    TooBig = 7,
    Again = 11,
    BadFileDescriptor = 9,
    Child = 10,
    NoEntry = 2,
    NoMemory = 12,
    InvalidArgument = 22,
    Fault = 14,
    IsDirectory = 21,
    NotDirectory = 20,
    Exists = 17,
    Busy = 16,
    NoSpace = 28,
    FileTooLarge = 27,
    NameTooLong = 36,
    Io = 5,
    ExecFormat = 8,
    NoSystemCall = 38,
}

impl LinuxErrno {
    pub(super) const fn return_value(self) -> isize {
        -(self as isize)
    }
}
