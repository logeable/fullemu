//! 管理父子关系、阻塞等待和 Linux wait 状态回收。

use super::{ProcessId, ProcessState, PROCESSES};

#[derive(Clone, Copy)]
pub(super) struct ProcessWaitRequest {
    pid: i32,
    status_address: Option<usize>,
    no_hang: bool,
}

pub(in crate::kernel) enum WaitDecision {
    Return(isize),
    Block,
    NoChild,
}

/// 处理当前进程的 wait 请求，并返回立即结果或阻塞决定。
pub(in crate::kernel) fn wait_for_current_child(
    pid: i32,
    status_address: Option<usize>,
    no_hang: bool,
) -> WaitDecision {
    wait_for_child(
        super::current_process_index(),
        ProcessWaitRequest {
            pid,
            status_address,
            no_hang,
        },
    )
}

/// 等待指定子进程或任意子进程；只保留退出状态，不重新取得已回收的运行资源。
fn wait_for_child(parent_index: usize, request: ProcessWaitRequest) -> WaitDecision {
    let (has_matching_child, exited_child) = {
        let processes = unsafe { &*core::ptr::addr_of!(PROCESSES) };
        let parent = processes
            .get(parent_index)
            .unwrap_or_else(|| panic!("wait4 的父进程索引无效：{parent_index}"));
        let parent_id = parent.id;
        let mut has_matching_child = false;
        let mut exited_child = None;

        for (child_index, child) in processes.iter().enumerate() {
            if child.parent_id != Some(parent_id) || !matches_pid(child.id, request.pid) {
                continue;
            }
            if child.state == ProcessState::Reaped {
                continue;
            };
            has_matching_child = true;
            if child.state == ProcessState::Exited {
                let Some(status) = child.exit_status else {
                    panic!("已退出子进程 {} 缺少退出状态", child.id.linux_pid());
                };
                exited_child = Some((child_index, child.id, status));
                break;
            }
        }
        (has_matching_child, exited_child)
    };

    if let Some((child_index, child_id, status)) = exited_child {
        write_wait_status(request.status_address, status.linux_wait_status());
        let child = unsafe { &mut *core::ptr::addr_of_mut!(PROCESSES) }
            .get_mut(child_index)
            .unwrap_or_else(|| panic!("wait4 子进程记录消失：{}", child_id.linux_pid()));
        child.wait_collected = true;
        if child.resources.is_none() {
            child.state = ProcessState::Reaped;
        } else {
            // 安全性：wait4 当前运行在父进程独占的内核栈和地址空间中，子进程栈不再活动。
            super::reap_process_resources(child_index, false);
        }
        return WaitDecision::Return(child_id.linux_pid() as isize);
    }

    if !has_matching_child {
        return WaitDecision::NoChild;
    }
    if request.no_hang {
        return WaitDecision::Return(0);
    }

    let parent = unsafe { &mut *core::ptr::addr_of_mut!(PROCESSES) }
        .get_mut(parent_index)
        .unwrap_or_else(|| panic!("wait4 的父进程记录消失：{parent_index}"));
    if parent.state != ProcessState::Running {
        panic!(
            "只有正在运行的进程可以进入 wait4：{}",
            parent.id.linux_pid()
        );
    }
    parent.wait_request = Some(request);
    parent.state = ProcessState::Waiting;
    WaitDecision::Block
}

/// 子进程退出时完成与之匹配的父进程等待，并使父进程重新可调度。
pub(super) fn notify_child_exit(child_index: usize) {
    let (child_id, parent_id, exit_status) = {
        let processes = unsafe { &*core::ptr::addr_of!(PROCESSES) };
        let child = processes
            .get(child_index)
            .unwrap_or_else(|| panic!("退出的子进程索引无效：{child_index}"));
        (child.id, child.parent_id, child.exit_status)
    };
    let Some(parent_id) = parent_id else {
        return;
    };
    let Some(exit_status) = exit_status else {
        panic!("退出子进程 {} 缺少退出状态", child_id.linux_pid());
    };
    let parent_index = parent_id.index();
    let wait_request = {
        let processes = unsafe { &*core::ptr::addr_of!(PROCESSES) };
        let parent = processes
            .get(parent_index)
            .unwrap_or_else(|| panic!("子进程 {} 的父进程记录不存在", child_id.linux_pid()));
        if parent.id != parent_id || parent.state != ProcessState::Waiting {
            return;
        }
        let Some(request) = parent.wait_request else {
            panic!("等待中的父进程 {} 缺少 wait4 请求", parent_id.linux_pid());
        };
        if !matches_pid(child_id, request.pid) {
            return;
        }
        if parent.resources.is_none() {
            panic!("等待中的父进程运行资源已释放");
        }
        request
    };

    // 安全性：父进程在 wait4 时已验证 status 指针；该进程页表仍拥有其用户栈映射。
    if let Some(status_address) = wait_request.status_address {
        if let Err(error) = super::activate_process_address_space(parent_index) {
            panic!(
                "切换到父进程地址空间写入 wait 状态失败：{}",
                error.description()
            );
        }
        // 安全性：wait4 入队前已检查对齐和可写范围，父进程页表在写入前刚完成激活。
        unsafe {
            super::super::memory::user_access::write_user_value(
                status_address,
                exit_status.linux_wait_status(),
            );
        }
    }

    // 安全性：父进程陷入帧仍位于其独占内核栈，所有进程页表都映射 FDT RAM 的内核高半区别名。
    unsafe {
        let parent_frame = super::process_trap_frame_pointer(parent_index);
        (*parent_frame).registers[10] = child_id.linux_pid() as usize;
    }

    let processes = unsafe { &mut *core::ptr::addr_of_mut!(PROCESSES) };
    let parent = processes
        .get_mut(parent_index)
        .unwrap_or_else(|| panic!("wait4 的父进程记录消失：{parent_index}"));
    parent.wait_request = None;
    parent.state = ProcessState::Ready;
    let child = processes
        .get_mut(child_index)
        .unwrap_or_else(|| panic!("wait4 子进程记录消失：{}", child_id.linux_pid()));
    child.wait_collected = true;
}

/// 父进程退出后解除子进程的父子关联；孤儿退出时不再保留可等待状态。
pub(super) fn orphan_children(parent_id: ProcessId) {
    let processes = unsafe { &mut *core::ptr::addr_of_mut!(PROCESSES) };
    for child in processes.iter_mut() {
        if child.parent_id != Some(parent_id) {
            continue;
        }
        child.parent_id = None;
        if child.state == ProcessState::Exited && child.resources.is_none() {
            child.state = ProcessState::Reaped;
        }
    }
}

fn matches_pid(process_id: ProcessId, requested_pid: i32) -> bool {
    requested_pid == -1 || process_id.linux_pid() == requested_pid
}

fn write_wait_status(status_address: Option<usize>, status: i32) {
    let Some(status_address) = status_address else {
        return;
    };
    // 安全性：wait4 仅在当前父进程地址空间中调用，syscall 层已验证此地址可写且正确对齐。
    unsafe { super::super::memory::user_access::write_user_value(status_address, status) };
}
