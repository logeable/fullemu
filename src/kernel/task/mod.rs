//! 管理任务启动、陷入处理和单 hart 调度。

use crate::arch::riscv64::trap::{ExceptionCause, InterruptCause, TrapCause, TrapFrame};
use alloc::vec::Vec;

mod creation;
mod stack;

use stack::{KernelStack, UserStack};

const MAX_USER_TASKS: usize = 8;
const BOOT_PROGRAM_NAME: &str = match option_env!("FULLEMU_BOOT_PROGRAM") {
    Some(name) => name,
    None => "shell",
};
const SSTATUS_SPP: usize = 1 << 8;
// 按 QEMU virt 的时间基准将时间片固定为 10 ms。
const TIME_SLICE_TICKS: u64 = crate::arch::riscv64::sbi::QEMU_VIRT_TIMEBASE_FREQUENCY_HZ / 100;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaskState {
    Ready,
    Running,
    Exited,
}

/// 任务表中按创建顺序追加的稳定标识；本阶段不复用已退出任务的 ID。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
struct TaskId(usize);

impl TaskId {
    const fn from_index(index: usize) -> Self {
        Self(index)
    }

    const fn index(self) -> usize {
        self.0
    }
}

struct Task {
    state: TaskState,
    saved_frame_address: usize,
    id: TaskId,
    program_range: (usize, usize),
    program_image: Option<super::user_program::LoadedUserProgram>,
    user_stack: Option<UserStack>,
    kernel_stack: Option<KernelStack>,
}

static mut TASKS: Vec<Task> = Vec::new();
static mut CURRENT_TASK_INDEX: usize = 0;
static mut PENDING_KERNEL_STACK_RELEASE: Option<usize> = None;

/// 装入构建配置指定的入口用户程序并开始运行。
pub(super) fn run_boot_program() -> ! {
    let Some(program_index) = super::user_program::find_index(BOOT_PROGRAM_NAME) else {
        crate::klog_error!("用户程序清单中没有入口程序：{BOOT_PROGRAM_NAME}");
        stop_forever();
    };

    start_programs(&[program_index]);
}

/// 同时装入并调度构建清单中的全部程序。
pub(super) fn run_batch_programs() -> ! {
    let program_count = super::user_program::count();
    if program_count == 0 || program_count > MAX_USER_TASKS {
        crate::klog_error!("批处理程序数量无效：{program_count}");
        stop_forever();
    }

    let mut program_indices = [0; MAX_USER_TASKS];
    for (program_index, slot) in program_indices.iter_mut().take(program_count).enumerate() {
        *slot = program_index;
    }

    start_programs(&program_indices[..program_count]);
}

fn start_programs(program_indices: &[usize]) -> ! {
    if program_indices.is_empty() || program_indices.len() > MAX_USER_TASKS {
        crate::klog_error!("用户任务数量无效：{}", program_indices.len());
        stop_forever();
    }

    // 安全性：单 hart 尚未进入用户任务；新建的页表都共同映射内核 RAM，因此启动代码可在 Bare 模式下逐个构造它们。
    unsafe {
        core::arch::asm!(
            "csrw sie, zero",
            "csrw satp, zero",
            "sfence.vma zero, zero",
            options(nostack)
        );
        core::ptr::write_volatile(core::ptr::addr_of_mut!(CURRENT_TASK_INDEX), 0);
    }

    {
        // 安全性：任务容器只在启动路径初始化；此时 S-mode 中断尚未开启。
        let tasks = unsafe { &mut *core::ptr::addr_of_mut!(TASKS) };
        if !tasks.is_empty() {
            crate::klog_error!("用户任务容器已经初始化");
            stop_forever();
        }
    }

    for program_index in program_indices.iter().copied() {
        if let Err(error) = creation::create_task(program_index) {
            error.report(program_index);
            stop_forever();
        }
    }

    set_task_state(0, TaskState::Running);
    if let Err(error) = super::memory::activate_address_space(0) {
        crate::klog_error!("无法激活首个任务地址空间：{}", error.description());
        stop_forever();
    }

    super::memory::log_address_spaces();
    crate::klog_info!("已启用 Sv39 用户/内核页权限边界");
    crate::klog_info!(
        "启动 {} 个用户任务；定时器中断保持调度响应",
        program_indices.len()
    );

    arm_next_timer_or_stop();
    crate::arch::riscv64::interrupt::enable_supervisor_timer();

    let first_frame = saved_frame_pointer(0);
    // 安全性：首个陷入帧、用户栈及内核栈页均由首个任务持有并保持有效。
    unsafe { crate::arch::riscv64::trap::set_user_kernel_stack(task_kernel_stack_top(0)) };
    // 安全性：首个陷入帧在对应任务整个生命周期内有效，入口不返回。
    unsafe { crate::arch::riscv64::trap::start_first_task(first_frame) }
}

/// 处理系统调用并返回下一项任务应恢复的陷入帧。
#[no_mangle]
pub extern "C" fn supervisor_trap_handler(frame: *mut TrapFrame) -> *mut TrapFrame {
    // 安全性：汇编入口已切换到当前任务的专用内核栈，并在那里构造完整陷入帧。
    let frame = unsafe { &mut *frame };
    let current_index = current_task_index();
    release_deferred_kernel_stack(current_index);
    let cause = TrapCause::decode(frame.cause);
    crate::klog_trace!("任务 {} 进入 trap handler：原因={cause:?}", current_index);

    if cause == TrapCause::Interrupt(InterruptCause::SupervisorTimer) {
        arm_next_timer_or_stop();

        // S-mode 中断目前不会在内核临界路径中嵌套；若未来启用嵌套，保留原帧直接返回。
        if frame.status & SSTATUS_SPP != 0 {
            return frame;
        }

        save_current_frame(current_index, frame);
        set_task_state(current_index, TaskState::Ready);
        return schedule_next(current_index);
    }

    if cause == TrapCause::Exception(ExceptionCause::UserEnvironmentCall)
        && frame.status & SSTATUS_SPP == 0
    {
        let syscall_number = frame.registers[17];
        let arguments = [
            frame.registers[10],
            frame.registers[11],
            frame.registers[12],
        ];
        let task = task_record_or_stop(current_index);
        let Some(stack_range) = user_stack_range(current_index) else {
            crate::klog_error!("任务 {} 的用户栈不存在", current_index);
            stop_forever();
        };
        let syscall_context =
            super::syscall::SyscallContext::new(current_index, task.program_range, stack_range);
        match super::syscall::dispatch(syscall_context, syscall_number, arguments) {
            super::syscall::SyscallOutcome::Return(result) => {
                frame.registers[10] = result as usize;
                frame.exception_pc += 4;
                return restore_user_frame(frame);
            }
            super::syscall::SyscallOutcome::Yield => {
                frame.registers[10] = 0;
                frame.exception_pc += 4;
                save_current_frame(current_index, frame);
                set_task_state(current_index, TaskState::Ready);
                return schedule_next(current_index);
            }
            super::syscall::SyscallOutcome::Exit(status) => {
                crate::klog_info!("任务 {} 调用 exit 结束，状态码：{status}", current_index);
                save_current_frame(current_index, frame);
                set_task_state(current_index, TaskState::Exited);
                return schedule_next(current_index);
            }
        }
    }

    crate::klog_error!(
        "用户任务 {} 陷入：原因={cause:?}，scause={:#018x}，sepc={:#018x}，stval={:#018x}",
        current_index,
        frame.cause,
        frame.exception_pc,
        frame.trap_value
    );

    if frame.status & SSTATUS_SPP == 0 {
        save_current_frame(current_index, frame);
        set_task_state(current_index, TaskState::Exited);
        crate::klog_warn!("用户任务 {} 因异常终止", current_index);
        return schedule_next(current_index);
    }

    stop_forever()
}

fn schedule_next(previous_index: usize) -> *mut TrapFrame {
    let Some(next_index) = find_next_ready_task(previous_index) else {
        crate::klog_info!("所有用户任务均已退出，共运行 {} 个任务", task_count());
        stop_forever();
    };

    set_task_state(next_index, TaskState::Running);
    // 安全性：只有单 hart 的陷入处理器修改当前任务索引。
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(CURRENT_TASK_INDEX), next_index);
    }

    if let Err(error) = super::memory::activate_address_space(next_index) {
        crate::klog_error!(
            "切换到任务 {} 的地址空间失败：{}",
            next_index,
            error.description()
        );
        stop_forever();
    }

    if previous_index != next_index && task_state(previous_index) == TaskState::Exited {
        match super::memory::destroy_address_space(previous_index) {
            Ok(released_page_count) => {
                release_user_stack(previous_index);
                discard_loaded_program_reference(previous_index);
                defer_kernel_stack_release(previous_index);
                crate::klog_info!(
                    "已回收任务 {} 的地址空间与用户栈：释放 {} 个页表页",
                    previous_index,
                    released_page_count
                );
            }
            Err(error) => {
                crate::klog_error!(
                    "回收任务 {} 的页表失败：{}",
                    previous_index,
                    error.description()
                );
                stop_forever();
            }
        }
    }

    if next_index != previous_index {
        crate::klog_debug!("任务切换：{} -> {}", previous_index, next_index);
    }

    let next_frame = saved_frame_pointer(next_index);
    // 安全性：调度器只返回初始化或先前 trap 保存的专属任务陷入帧。
    let next_frame = unsafe { &mut *next_frame };
    restore_user_frame(next_frame)
}

fn arm_next_timer_or_stop() {
    let Some(deadline) = crate::arch::riscv64::sbi::read_time().checked_add(TIME_SLICE_TICKS)
    else {
        crate::klog_error!("计算下一次定时器截止时间时溢出");
        stop_forever();
    };

    if let Err(error) = crate::arch::riscv64::sbi::set_timer(deadline) {
        crate::klog_error!("SBI 设置 supervisor 定时器失败：错误码 {}", error.0);
        stop_forever();
    }
}

fn find_next_ready_task(previous_index: usize) -> Option<usize> {
    let count = task_count();
    for offset in 1..=count {
        let candidate = (previous_index + offset) % count;
        if task_state(candidate) == TaskState::Ready {
            return Some(candidate);
        }
    }
    None
}

fn task_kernel_stack_top(index: usize) -> usize {
    // 安全性：只有已创建且仍可调度的任务会请求栈顶；持有的内核栈页帧保持分配。
    let Some(stack) = (unsafe { &*core::ptr::addr_of!(TASKS) })
        .get(index)
        .and_then(|task| task.kernel_stack.as_ref())
    else {
        crate::klog_error!("任务 {} 的内核栈不存在", index);
        stop_forever();
    };
    stack.top()
}

fn restore_user_frame(frame: &mut TrapFrame) -> *mut TrapFrame {
    // 安全性：陷入帧位于专用内核栈顶部下方；sscratch 指回该任务的栈顶以处理下一次陷入。
    let kernel_stack_top = frame as *mut TrapFrame as usize + core::mem::size_of::<TrapFrame>();
    unsafe { crate::arch::riscv64::trap::set_user_kernel_stack(kernel_stack_top) };
    frame as *mut TrapFrame
}

fn saved_frame_pointer(index: usize) -> *mut TrapFrame {
    task_record_or_stop(index).saved_frame_address as *mut TrapFrame
}

fn save_current_frame(index: usize, frame: &mut TrapFrame) {
    let Some(task) = (unsafe { &mut *core::ptr::addr_of_mut!(TASKS) }).get_mut(index) else {
        crate::klog_error!("无法保存不存在的任务 {} 的陷入帧", index);
        stop_forever();
    };
    // 安全性：当前陷入帧由本 hart 独占，索引是正在运行的有效任务，且陷入处理不会嵌套调度。
    task.saved_frame_address = frame as *mut TrapFrame as usize;
}

fn current_task_index() -> usize {
    // 安全性：当前任务索引只由单 hart 的启动路径和陷入处理器访问。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(CURRENT_TASK_INDEX)) }
}

fn task_count() -> usize {
    // 安全性：任务容器在首次进入 U-mode 前完成初始化，此后长度保持不变。
    unsafe { (&*core::ptr::addr_of!(TASKS)).len() }
}

fn task_state(index: usize) -> TaskState {
    task_record_or_stop(index).state
}

fn set_task_state(index: usize, state: TaskState) {
    let Some(task) = (unsafe { &mut *core::ptr::addr_of_mut!(TASKS) }).get_mut(index) else {
        crate::klog_error!("无法更新不存在的任务 {} 的状态", index);
        stop_forever();
    };
    // 安全性：状态只在单 hart 启动或陷入路径更新；S-mode 陷入期间不会发生定时器重入。
    task.state = state;
}

#[derive(Clone, Copy)]
struct TaskSnapshot {
    state: TaskState,
    saved_frame_address: usize,
    program_range: (usize, usize),
}

fn task_record_or_stop(index: usize) -> TaskSnapshot {
    // 安全性：任务容器进入 U-mode 前构造完成，此后不增删记录；快照不暴露对容器元素的引用。
    let Some(task) = (unsafe { &*core::ptr::addr_of!(TASKS) }).get(index) else {
        crate::klog_error!("任务索引越界：{}，任务数量={}", index, task_count());
        stop_forever();
    };
    if task.id.index() != index {
        crate::klog_error!("任务记录 ID 不匹配：索引={}，ID={}", index, task.id.index());
        stop_forever();
    }
    TaskSnapshot {
        state: task.state,
        saved_frame_address: task.saved_frame_address,
        program_range: task.program_range,
    }
}

fn user_stack_range(index: usize) -> Option<(usize, usize)> {
    // 安全性：任务表长度在启动后固定；共享引用仅读取该任务仍拥有的用户栈描述。
    unsafe { &*core::ptr::addr_of!(TASKS) }
        .get(index)?
        .user_stack
        .as_ref()
        .map(UserStack::range)
}

fn release_user_stack(index: usize) {
    // 安全性：调用方已切换到其他地址空间并销毁旧页表，旧 U-mode 映射不再引用这些页帧。
    let Some(task) = (unsafe { &mut *core::ptr::addr_of_mut!(TASKS) }).get_mut(index) else {
        crate::klog_error!("无法释放不存在任务 {} 的用户栈", index);
        stop_forever();
    };
    if task.user_stack.take().is_none() {
        crate::klog_error!("任务 {} 的用户栈已经释放", index);
        stop_forever();
    }
}

fn discard_loaded_program_reference(index: usize) {
    // 安全性：调用方已切换到其他地址空间并销毁旧页表；这里只丢弃任务记录中的镜像描述，固定槽位仍由链接器预留。
    let Some(task) = (unsafe { &mut *core::ptr::addr_of_mut!(TASKS) }).get_mut(index) else {
        crate::klog_error!("无法释放不存在任务 {} 的程序镜像", index);
        stop_forever();
    };
    if task.program_image.take().is_none() {
        crate::klog_error!("任务 {} 的程序镜像已经释放", index);
        stop_forever();
    }
}

fn defer_kernel_stack_release(index: usize) {
    // 安全性：当前仍在被退出任务的内核栈上运行，只记录待释放任务，不立即释放活动栈。
    unsafe {
        let pending = core::ptr::addr_of_mut!(PENDING_KERNEL_STACK_RELEASE);
        if core::ptr::read_volatile(pending).is_some() {
            crate::klog_error!("已有尚未释放的任务内核栈");
            stop_forever();
        }
        core::ptr::write_volatile(pending, Some(index));
    }
}

fn release_deferred_kernel_stack(current_index: usize) {
    // 安全性：此函数在新任务自己的陷入栈上运行；待释放栈属于已经退出的其他任务。
    let pending =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!(PENDING_KERNEL_STACK_RELEASE)) };
    let Some(pending_index) = pending else {
        return;
    };
    if pending_index == current_index {
        crate::klog_error!("拒绝释放当前任务正在使用的内核栈");
        stop_forever();
    }

    // 安全性：pending_index 来自退出任务；当前陷入帧已在另一任务的栈上，故该栈不再活动。
    let Some(task) = (unsafe { &mut *core::ptr::addr_of_mut!(TASKS) }).get_mut(pending_index)
    else {
        crate::klog_error!("待释放的内核栈所属任务 {} 不存在", pending_index);
        stop_forever();
    };
    if task.kernel_stack.take().is_none() {
        crate::klog_error!("任务 {} 的内核栈已经释放", pending_index);
        stop_forever();
    }
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(PENDING_KERNEL_STACK_RELEASE), None);
    }
}

fn stop_forever() -> ! {
    loop {
        // 安全性：当前没有可运行任务；WFI 让 hart 等待，不再恢复已退出或异常的任务。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __data_end: u8;
    static __bss_start: u8;
    static __bss_end: u8;
}
