//! 管理一次 syscall 可访问的用户地址范围。

/// 记录当前进程的身份及 syscall 可访问的用户内存范围。
#[derive(Clone, Copy)]
pub(super) struct SyscallContext {
    pub(super) process_id: usize,
    user_memory: crate::memory::UserMemoryRanges,
}

impl SyscallContext {
    pub(super) const fn new(
        process_id: usize,
        user_memory: crate::memory::UserMemoryRanges,
    ) -> Self {
        Self {
            process_id,
            user_memory,
        }
    }

    pub(super) fn contains_writable_buffer(self, address: usize, length: usize) -> bool {
        self.user_memory
            .writable_program
            .is_some_and(|range| contains_range(range, address, length))
            || contains_range(self.user_memory.heap, address, length)
            || contains_range(self.user_memory.stack, address, length)
    }

    pub(super) fn contains_readable_buffer(self, address: usize, length: usize) -> bool {
        contains_range(self.user_memory.program, address, length)
            || contains_range(self.user_memory.heap, address, length)
            || contains_range(self.user_memory.stack, address, length)
    }
}

fn contains_range(range: (usize, usize), address: usize, length: usize) -> bool {
    if length == 0 {
        return true;
    }
    let Some(end) = address.checked_add(length) else {
        return false;
    };
    address >= range.0 && end <= range.1
}
