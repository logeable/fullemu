//! 协调标准控制台和普通文件的 `read`、`write` 系统调用。

use super::abi::*;
use super::context::SyscallContext;

pub(super) fn read(
    context: SyscallContext,
    file_descriptor: usize,
    buffer_address: usize,
    count: usize,
) -> isize {
    if count == 0 {
        return 0;
    }
    if !context.contains_writable_buffer(buffer_address, count) {
        return LinuxErrno::Fault.return_value();
    }

    if file_descriptor != STDIN_FILE_DESCRIPTOR {
        let Some(mut file) = crate::process::open_file(file_descriptor) else {
            return LinuxErrno::BadFileDescriptor.return_value();
        };
        if !file.readable {
            return LinuxErrno::BadFileDescriptor.return_value();
        }
        if file.kind == crate::filesystem::InodeKind::Directory {
            return LinuxErrno::IsDirectory.return_value();
        }
        let mut copied = 0;
        let mut buffer = [0; MAX_FILE_IO_CHUNK];
        while copied < count {
            let amount = core::cmp::min(buffer.len(), count - copied);
            let read_count = match crate::filesystem::with_mounted(|file_system| {
                file_system.read_at(file.inode_id, file.position, &mut buffer[..amount])
            }) {
                Ok(Ok(read_count)) => read_count,
                Ok(Err(error)) | Err(error) => {
                    if copied == 0 {
                        return super::filesystem::file_system_error(error);
                    }
                    break;
                }
            };
            if read_count == 0 {
                break;
            }
            for (offset, byte) in buffer[..read_count].iter().copied().enumerate() {
                // 安全性：入口处已检查整个用户输出缓冲区可写，当前地址空间在同步 I/O 期间不变。
                unsafe {
                    crate::memory::user_access::write_user_value(
                        buffer_address + copied + offset,
                        byte,
                    );
                }
            }
            copied += read_count;
            file.position += read_count;
            if read_count < amount {
                break;
            }
        }
        if crate::process::update_open_file(file_descriptor, file).is_err() {
            return LinuxErrno::BadFileDescriptor.return_value();
        }
        return copied as isize;
    }

    // 第一个字节阻塞等待；之后只读取当前已经到达的字节，允许 read 返回短读。
    let mut bytes_read = 0;
    while bytes_read < count {
        let byte = if bytes_read == 0 {
            crate::console::read_byte()
        } else {
            let Some(byte) = crate::console::try_read_byte() else {
                break;
            };
            byte
        };

        // 安全性：前面的范围检查保证该字节位于当前进程已映射的可写用户栈内。
        unsafe {
            crate::memory::user_access::write_user_value(buffer_address + bytes_read, byte);
        }
        bytes_read += 1;
    }

    bytes_read as isize
}

pub(super) fn write(
    context: SyscallContext,
    file_descriptor: usize,
    buffer_address: usize,
    count: usize,
) -> isize {
    if count == 0 {
        return 0;
    }

    if !context.contains_readable_buffer(buffer_address, count) {
        crate::klog_trace!(
            "拒绝进程 {} 的 write 缓冲区：地址={:#018x}，长度={}",
            context.process_id,
            buffer_address,
            count
        );
        return LinuxErrno::Fault.return_value();
    }

    if file_descriptor == STDOUT_FILE_DESCRIPTOR || file_descriptor == STDERR_FILE_DESCRIPTOR {
        // 安全性：范围检查保证待读字节位于当前进程已映射的镜像或栈内。
        for offset in 0..count {
            // 安全性：完整缓冲区已验证，单字节读取期间由 memory 模块短暂开启 SUM。
            let byte =
                unsafe { crate::memory::user_access::read_user_byte(buffer_address + offset) };
            crate::console::write_byte(byte);
        }
        return count as isize;
    }

    let Some(mut file) = crate::process::open_file(file_descriptor) else {
        return LinuxErrno::BadFileDescriptor.return_value();
    };
    if !file.writable {
        return LinuxErrno::BadFileDescriptor.return_value();
    }
    if file.kind == crate::filesystem::InodeKind::Directory {
        return LinuxErrno::IsDirectory.return_value();
    }

    let mut copied = 0;
    let mut buffer = [0; MAX_FILE_IO_CHUNK];
    while copied < count {
        let amount = core::cmp::min(buffer.len(), count - copied);
        for (offset, byte) in buffer[..amount].iter_mut().enumerate() {
            // 安全性：入口处已验证整个用户输入缓冲区可读，当前地址空间在同步 I/O 期间不变。
            *byte = unsafe {
                crate::memory::user_access::read_user_byte(buffer_address + copied + offset)
            };
        }
        let result = crate::filesystem::with_mounted(|file_system| {
            if file.append {
                file.position = file_system.metadata(file.inode_id)?.size;
            }
            file_system.write_at(file.inode_id, file.position, &buffer[..amount])
        });
        let written = match result {
            Ok(Ok(written)) => written,
            Ok(Err(error)) | Err(error) => {
                if copied == 0 {
                    return super::filesystem::file_system_error(error);
                }
                break;
            }
        };
        if written == 0 {
            break;
        }
        copied += written;
        file.position += written;
        if written < amount {
            break;
        }
    }
    if crate::process::update_open_file(file_descriptor, file).is_err() {
        return LinuxErrno::BadFileDescriptor.return_value();
    }
    copied as isize
}
