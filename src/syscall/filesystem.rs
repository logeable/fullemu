//! 适配目录与文件描述符相关的 Linux RISC-V 系统调用。

use super::abi::*;
use super::context::SyscallContext;

pub(super) fn openat(
    context: SyscallContext,
    directory_descriptor: usize,
    path_address: usize,
    flags: usize,
    _mode: usize,
) -> isize {
    if directory_descriptor != LINUX_AT_FDCWD {
        return LinuxErrno::BadFileDescriptor.return_value();
    }
    let allowed_flags = LINUX_O_ACCMODE
        | LINUX_O_CREAT
        | LINUX_O_EXCL
        | LINUX_O_TRUNC
        | LINUX_O_APPEND
        | LINUX_O_DIRECTORY;
    if flags & !allowed_flags != 0
        || flags & LINUX_O_ACCMODE == 3
        || (flags & LINUX_O_EXCL != 0 && flags & LINUX_O_CREAT == 0)
        || (flags & LINUX_O_CREAT != 0 && flags & LINUX_O_DIRECTORY != 0)
        || (flags & LINUX_O_TRUNC != 0 && flags & LINUX_O_ACCMODE == LINUX_O_RDONLY)
    {
        return LinuxErrno::InvalidArgument.return_value();
    }

    let (path_bytes, path_length) = match copy_user_path(context, path_address) {
        Ok(path) => path,
        Err(error) => return error.return_value(),
    };
    let Ok(path) = core::str::from_utf8(&path_bytes[..path_length]) else {
        return LinuxErrno::InvalidArgument.return_value();
    };
    let access_mode = flags & LINUX_O_ACCMODE;
    let options = crate::filesystem::OpenOptions {
        readable: access_mode == LINUX_O_RDONLY || access_mode == LINUX_O_RDWR,
        writable: access_mode == LINUX_O_WRONLY || access_mode == LINUX_O_RDWR,
        create: flags & LINUX_O_CREAT != 0,
        exclusive: flags & LINUX_O_EXCL != 0,
        truncate: flags & LINUX_O_TRUNC != 0,
        append: flags & LINUX_O_APPEND != 0,
        directory_only: flags & LINUX_O_DIRECTORY != 0,
    };
    let file =
        match crate::filesystem::with_mounted(|file_system| file_system.open_path(path, options)) {
            Ok(Ok(file)) => file,
            Ok(Err(error)) | Err(error) => return file_system_error(error),
        };
    match crate::process::install_open_file(file) {
        Ok(descriptor) => descriptor as isize,
        Err(()) => LinuxErrno::NoMemory.return_value(),
    }
}

pub(super) fn close(descriptor: usize) -> isize {
    if crate::process::close_open_file(descriptor).is_err() {
        return LinuxErrno::BadFileDescriptor.return_value();
    }
    0
}

pub(super) fn mkdirat(
    context: SyscallContext,
    directory_descriptor: usize,
    path_address: usize,
    _mode: usize,
) -> isize {
    if directory_descriptor != LINUX_AT_FDCWD {
        return LinuxErrno::BadFileDescriptor.return_value();
    }
    let (path_bytes, path_length) = match copy_user_path(context, path_address) {
        Ok(path) => path,
        Err(error) => return error.return_value(),
    };
    let Ok(path) = core::str::from_utf8(&path_bytes[..path_length]) else {
        return LinuxErrno::InvalidArgument.return_value();
    };
    match crate::filesystem::with_mounted(|file_system| file_system.create_directory_path(path)) {
        Ok(Ok(_)) => 0,
        Ok(Err(error)) | Err(error) => file_system_error(error),
    }
}

pub(super) fn unlinkat(
    context: SyscallContext,
    directory_descriptor: usize,
    path_address: usize,
    flags: usize,
) -> isize {
    if directory_descriptor != LINUX_AT_FDCWD {
        return LinuxErrno::BadFileDescriptor.return_value();
    }
    if flags != 0 && flags != LINUX_AT_REMOVEDIR {
        return LinuxErrno::InvalidArgument.return_value();
    }
    if flags == LINUX_AT_REMOVEDIR {
        return LinuxErrno::InvalidArgument.return_value();
    }
    let (path_bytes, path_length) = match copy_user_path(context, path_address) {
        Ok(path) => path,
        Err(error) => return error.return_value(),
    };
    let Ok(path) = core::str::from_utf8(&path_bytes[..path_length]) else {
        return LinuxErrno::InvalidArgument.return_value();
    };
    let inode_id =
        match crate::filesystem::with_mounted(|file_system| file_system.lookup_path(path)) {
            Ok(Ok(inode_id)) => inode_id,
            Ok(Err(error)) | Err(error) => return file_system_error(error),
        };
    if crate::process::is_inode_open(inode_id) {
        return LinuxErrno::Busy.return_value();
    }
    match crate::filesystem::with_mounted(|file_system| file_system.unlink_file(path)) {
        Ok(Ok(())) => 0,
        Ok(Err(error)) | Err(error) => file_system_error(error),
    }
}

pub(super) fn getdents64(
    context: SyscallContext,
    descriptor: usize,
    buffer_address: usize,
    count: usize,
) -> isize {
    let Some(mut file) = crate::process::open_file(descriptor) else {
        return LinuxErrno::BadFileDescriptor.return_value();
    };
    if !file.readable {
        return LinuxErrno::BadFileDescriptor.return_value();
    }
    if file.kind != crate::filesystem::InodeKind::Directory {
        return LinuxErrno::NotDirectory.return_value();
    }
    if count == 0 {
        return 0;
    }
    if !context.contains_writable_buffer(buffer_address, count) {
        return LinuxErrno::Fault.return_value();
    }

    let result = match crate::filesystem::with_mounted(|file_system| {
        let slot_count = file_system.directory_slot_count(file.inode_id)?;
        let mut written = 0usize;
        while file.position < slot_count {
            let slot_index = file.position;
            file.position += 1;
            let Some(entry) = file_system.directory_entry(file.inode_id, slot_index)? else {
                continue;
            };
            let record_length = align_up(19 + entry.name_length + 1, 8);
            if record_length > count - written {
                if written == 0 {
                    return Err(crate::filesystem::FileSystemError::InvalidOffset);
                }
                file.position = slot_index;
                break;
            }
            let mut record = [0; 48];
            record[0..8].copy_from_slice(&u64::from(entry.inode_id.index()).to_le_bytes());
            record[8..16].copy_from_slice(&((slot_index + 1) as i64).to_le_bytes());
            record[16..18].copy_from_slice(&(record_length as u16).to_le_bytes());
            record[18] = match entry.kind {
                crate::filesystem::InodeKind::File => 8,
                crate::filesystem::InodeKind::Directory => 4,
            };
            record[19..19 + entry.name_length].copy_from_slice(&entry.name[..entry.name_length]);
            for (offset, byte) in record[..record_length].iter().copied().enumerate() {
                // 安全性：系统调用入口已验证完整的输出缓冲区位于当前进程可写内存。
                unsafe {
                    crate::memory::user_access::write_user_value(
                        buffer_address + written + offset,
                        byte,
                    );
                }
            }
            written += record_length;
        }
        Ok(written)
    }) {
        Ok(Ok(written)) => written,
        Ok(Err(crate::filesystem::FileSystemError::InvalidOffset)) => {
            return LinuxErrno::InvalidArgument.return_value();
        }
        Ok(Err(error)) | Err(error) => return file_system_error(error),
    };
    if crate::process::update_open_file(descriptor, file).is_err() {
        return LinuxErrno::BadFileDescriptor.return_value();
    }
    result as isize
}

fn copy_user_path(
    context: SyscallContext,
    address: usize,
) -> Result<([u8; MAX_PATH_LENGTH], usize), LinuxErrno> {
    let mut path = [0; MAX_PATH_LENGTH];
    for (offset, byte) in path.iter_mut().enumerate() {
        let Some(user_address) = address.checked_add(offset) else {
            return Err(LinuxErrno::Fault);
        };
        if !context.contains_readable_buffer(user_address, 1) {
            return Err(LinuxErrno::Fault);
        }
        // 安全性：逐字节确认路径字符串在当前进程可读内存中后再复制。
        *byte = unsafe { crate::memory::user_access::read_user_byte(user_address) };
        if *byte == 0 {
            if offset == 0 {
                return Err(LinuxErrno::InvalidArgument);
            }
            return Ok((path, offset));
        }
    }
    Err(LinuxErrno::NameTooLong)
}

pub(super) fn file_system_error(error: crate::filesystem::FileSystemError) -> isize {
    match error {
        crate::filesystem::FileSystemError::NoEntry => LinuxErrno::NoEntry.return_value(),
        crate::filesystem::FileSystemError::AlreadyExists => LinuxErrno::Exists.return_value(),
        crate::filesystem::FileSystemError::NotDirectory
        | crate::filesystem::FileSystemError::PathNotDirectory => {
            LinuxErrno::NotDirectory.return_value()
        }
        crate::filesystem::FileSystemError::IsDirectory => LinuxErrno::IsDirectory.return_value(),
        crate::filesystem::FileSystemError::NameTooLong => LinuxErrno::NameTooLong.return_value(),
        crate::filesystem::FileSystemError::InvalidName
        | crate::filesystem::FileSystemError::InvalidOffset => {
            LinuxErrno::InvalidArgument.return_value()
        }
        crate::filesystem::FileSystemError::NoSpace => LinuxErrno::NoSpace.return_value(),
        crate::filesystem::FileSystemError::FileTooLarge => LinuxErrno::FileTooLarge.return_value(),
        _ => LinuxErrno::Io.return_value(),
    }
}

const fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}
