//! 管理进程用户堆的 program break、页帧所有权和地址空间映射。

use alloc::vec::Vec;

use crate::frame_allocator::{self, PhysicalFrame};
use crate::memory::page::PAGE_SIZE;

/// 一个用户进程从镜像末尾向高地址增长的堆。
pub(super) struct UserHeap {
    start: usize,
    program_break: usize,
    frames: Vec<PhysicalFrame>,
}

impl UserHeap {
    /// 以页对齐的用户镜像末尾作为初始 program break。
    pub(super) fn new(program_end: usize) -> Self {
        if program_end % PAGE_SIZE != 0 {
            panic!("用户堆起点未按页对齐：{program_end:#x}");
        }
        Self {
            start: program_end,
            program_break: program_end,
            frames: Vec::new(),
        }
    }

    /// 返回堆目前实际映射的用户虚拟范围。
    pub(super) fn mapped_range(&self) -> (usize, usize) {
        let end = self.start + self.frames.len() * PAGE_SIZE;
        (self.start, end)
    }

    pub(super) fn frames(&self) -> &[PhysicalFrame] {
        &self.frames
    }

    /// 为子进程复制当前堆内容，保持 program break 与页布局一致。
    pub(super) fn duplicate(&self) -> Result<Self, ()> {
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(self.frames.len())
            .map_err(|_| ())?;
        for source in &self.frames {
            let destination = match frame_allocator::allocate_zeroed_frame() {
                Ok(frame) => frame,
                Err(_) => {
                    release_frames(frames);
                    return Err(());
                }
            };
            crate::memory::copy_frame(source, &destination);
            frames.push(destination);
        }
        Ok(Self {
            start: self.start,
            program_break: self.program_break,
            frames,
        })
    }

    /// 设置 program break；无效请求或资源不足时保持并返回旧值。
    pub(super) fn set_break(
        &mut self,
        requested_break: usize,
        limit: usize,
        address_space: &crate::memory::AddressSpace,
    ) -> usize {
        let old_break = self.program_break;
        if requested_break == 0 {
            return old_break;
        }
        if requested_break < self.start || requested_break > limit {
            return old_break;
        }

        let Some(required_bytes) = requested_break.checked_sub(self.start) else {
            return old_break;
        };
        let Some(required_rounded_bytes) = required_bytes.checked_add(PAGE_SIZE - 1) else {
            return old_break;
        };
        let required_frame_count = required_rounded_bytes / PAGE_SIZE;

        if required_frame_count > self.frames.len() {
            let additional_count = required_frame_count - self.frames.len();
            let mut additional_frames = Vec::new();
            if additional_frames
                .try_reserve_exact(additional_count)
                .is_err()
                || self.frames.try_reserve(additional_count).is_err()
            {
                return old_break;
            }

            for _ in 0..additional_count {
                match frame_allocator::allocate_zeroed_frame() {
                    Ok(frame) => additional_frames.push(frame),
                    Err(_) => {
                        release_frames(additional_frames);
                        return old_break;
                    }
                }
            }

            let mapping_start = self.start + self.frames.len() * PAGE_SIZE;
            if crate::memory::address_space::map_user_heap_frames(
                address_space,
                mapping_start,
                &additional_frames,
            )
            .is_err()
            {
                release_frames(additional_frames);
                return old_break;
            }
            self.frames.extend(additional_frames);
        } else if required_frame_count < self.frames.len() {
            let released_count = self.frames.len() - required_frame_count;
            let unmap_start = self.start + required_frame_count * PAGE_SIZE;
            if crate::memory::address_space::unmap_user_heap_pages(
                address_space,
                unmap_start,
                released_count,
            )
            .is_err()
            {
                panic!("用户堆页表映射与进程资源记录不一致");
            }
            while self.frames.len() > required_frame_count {
                let Some(frame) = self.frames.pop() else {
                    panic!("用户堆页帧记录意外为空");
                };
                if let Err(error) = frame_allocator::deallocate_frame(frame) {
                    panic!("释放用户堆页帧失败：{}", error.description());
                }
            }
        }

        self.program_break = requested_break;
        requested_break
    }
}

impl Drop for UserHeap {
    fn drop(&mut self) {
        release_frames(core::mem::take(&mut self.frames));
    }
}

fn release_frames(mut frames: Vec<PhysicalFrame>) {
    while let Some(frame) = frames.pop() {
        if let Err(error) = frame_allocator::deallocate_frame(frame) {
            panic!("释放用户堆页帧失败：{}", error.description());
        }
    }
}
