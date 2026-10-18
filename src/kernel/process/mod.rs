//! 管理进程对象、用户态陷入、单 hart 调度和退出资源回收。

use crate::arch::riscv64::trap::{ExceptionCause, InterruptCause, TrapCause, TrapFrame};
use alloc::vec::Vec;

mod creation;
mod stack;
mod wait;

use stack::{KernelStack, UserStack};

const BOOT_PROGRAM_NAME: &str = match option_env!("FULLEMU_BOOT_PROGRAM") {
    Some(name) => name,
    None => "shell",
};
const SSTATUS_SPP: usize = 1 << 8;
// 按 QEMU virt 的时间基准将时间片固定为 10 ms。
const TIME_SLICE_TICKS: u64 = crate::arch::riscv64::sbi::QEMU_VIRT_TIMEBASE_FREQUENCY_HZ / 100;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessState {
    Ready,
    Running,
    Waiting,
    Exited,
    Reaped,
}

/// 进程表中按创建顺序追加的稳定标识；本阶段不复用已退出进程的 ID。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
struct ProcessId(usize);

impl ProcessId {
    const fn from_index(index: usize) -> Self {
        Self(index)
    }

    const fn index(self) -> usize {
        self.0
    }

    const fn linux_pid(self) -> i32 {
        (self.0 + 1) as i32
    }
}

#[derive(Clone, Copy, Debug)]
enum ProcessExitStatus {
    Code(u8),
    Signal(u8),
}

impl ProcessExitStatus {
    const fn linux_wait_status(self) -> i32 {
        match self {
            Self::Code(code) => (code as i32) << 8,
            Self::Signal(signal) => signal as i32,
        }
    }
}

struct ProcessResources {
    address_space: super::memory::AddressSpace,
    program_range: (usize, usize),
    program_image: super::user_program::LoadedUserProgram,
    user_stack: UserStack,
    kernel_stack: KernelStack,
}

struct Process {
    id: ProcessId,
    parent_id: Option<ProcessId>,
    state: ProcessState,
    exit_status: Option<ProcessExitStatus>,
    wait_request: Option<super::syscall::WaitRequest>,
    wait_collected: bool,
    resources: Option<ProcessResources>,
}

static mut PROCESSES: Vec<Process> = Vec::new();
static mut CURRENT_PROCESS_INDEX: usize = 0;
static mut PENDING_KERNEL_STACK_RELEASE: Option<(ProcessId, KernelStack)> = None;

/// 装入构建配置指定的入口用户程序并开始运行。
pub(super) fn run_boot_program() -> ! {
    let Some(program_index) = super::user_program::find_index(BOOT_PROGRAM_NAME) else {
        panic!("用户程序清单中没有入口程序：{BOOT_PROGRAM_NAME}");
    };

    let mut program_indices = Vec::new();
    if program_indices.try_reserve_exact(1).is_err() {
        panic!("无法为入口程序索引分配内核堆空间");
    }
    program_indices.push(program_index);
    start_programs(program_indices);
}

/// 同时装入并调度构建清单中的全部程序。
pub(super) fn run_batch_programs() -> ! {
    let program_count = super::user_program::count();
    if program_count == 0 {
        panic!("批处理程序清单为空");
    }

    let mut program_indices = Vec::new();
    if program_indices.try_reserve_exact(program_count).is_err() {
        panic!("无法为批处理程序索引分配内核堆空间");
    }
    program_indices.extend(0..program_count);

    start_programs(program_indices);
}

fn start_programs(program_indices: Vec<usize>) -> ! {
    if program_indices.is_empty() {
        panic!("启动进程列表不能为空");
    }

    // 安全性：启动页表仍保持活动；新建进程页表映射内核高半区、启动栈和 RAM 直接映射区。
    unsafe {
        core::arch::asm!("csrw sie, zero", options(nostack));
        core::ptr::write_volatile(core::ptr::addr_of_mut!(CURRENT_PROCESS_INDEX), 0);
    }

    {
        // 安全性：进程容器只在启动路径初始化；此时 S-mode 中断尚未开启。
        let processes = unsafe { &mut *core::ptr::addr_of_mut!(PROCESSES) };
        if !processes.is_empty() {
            panic!("用户进程容器已经初始化");
        }
    }

    let program_count = program_indices.len();
    for program_index in program_indices.iter().copied() {
        if let Err(error) = creation::create_process(program_index) {
            panic!(
                "用户进程 {} 创建失败：{}",
                program_index,
                error.description()
            );
        }
    }
    drop(program_indices);

    let mut kernel_stacks = Vec::new();
    if kernel_stacks.try_reserve_exact(program_count).is_err() {
        panic!("无法为共享内核栈映射分配内核堆空间");
    }
    // 安全性：进程创建已完成且启动中断仍关闭；每个内核栈的页帧所有权由进程资源记录持有。
    let processes = unsafe { &*core::ptr::addr_of!(PROCESSES) };
    for process in processes.iter() {
        let Some(resources) = process.resources.as_ref() else {
            panic!("进程 {} 的资源在启动时不存在", process.id.index());
        };
        let stack = &resources.kernel_stack;
        let range = stack.range();
        kernel_stacks.push(super::memory::KernelStackMapping {
            virtual_start: range.0,
            physical_start: stack.physical_start(),
            size: range.1 - range.0,
        });
    }
    let processes = unsafe { &mut *core::ptr::addr_of_mut!(PROCESSES) };
    for process in processes.iter_mut() {
        let Some(resources) = process.resources.as_mut() else {
            panic!("进程 {} 的资源在启动时不存在", process.id.index());
        };
        if let Err(error) = super::memory::address_space::map_kernel_stacks(
            &mut resources.address_space,
            &kernel_stacks,
        ) {
            panic!(
                "进程 {} 的内核栈映射失败：{}",
                process.id.index(),
                error.description()
            );
        }
    }
    drop(kernel_stacks);

    set_process_state(0, ProcessState::Running);
    if let Err(error) = activate_process_address_space(0) {
        panic!("无法激活首个进程地址空间：{}", error.description());
    }

    let trap_entry = crate::arch::riscv64::trap::supervisor_trap_entry_address();
    // 安全性：陷入入口由链接器直接放在高半区 VMA，且每个进程页表均以 SR-X 映射该段。
    unsafe {
        crate::arch::riscv64::trap::set_supervisor_trap_vector(trap_entry);
    }

    log_process_address_spaces();
    crate::klog_info!("已启用 Sv39 用户/内核页权限边界");
    crate::klog_info!("S-mode 陷入入口位于内核高半区：{trap_entry:#018x}");
    crate::klog_info!("启动 {} 个用户进程；定时器中断保持调度响应", program_count);

    arm_next_timer();
    crate::arch::riscv64::interrupt::enable_supervisor_timer();

    let first_frame = process_trap_frame_pointer(0);
    // 安全性：首个陷入帧、用户栈及内核栈页均由首个进程持有并保持有效。
    unsafe { crate::arch::riscv64::trap::set_user_kernel_stack(process_kernel_stack_top(0)) };
    // 安全性：首个陷入帧在对应进程整个生命周期内有效，入口不返回。
    unsafe { crate::arch::riscv64::trap::start_first_process(first_frame) }
}

/// 处理系统调用并返回下一项进程应恢复的陷入帧。
#[no_mangle]
pub extern "C" fn supervisor_trap_handler(frame: *mut TrapFrame) -> *mut TrapFrame {
    // 安全性：汇编入口已切换到当前进程的专用内核栈，并在那里构造完整陷入帧。
    let frame = unsafe { &mut *frame };
    let current_index = current_process_index();
    release_deferred_kernel_stack(current_index);
    let cause = TrapCause::decode(frame.cause);
    crate::klog_trace!(
        "进程 PID={} 进入 trap handler：原因={cause:?}",
        process_id_at(current_index).linux_pid()
    );

    if cause == TrapCause::Interrupt(InterruptCause::SupervisorTimer) {
        arm_next_timer();

        // S-mode 中断目前不会在内核临界路径中嵌套；若未来启用嵌套，保留原帧直接返回。
        if frame.status & SSTATUS_SPP != 0 {
            return frame;
        }

        set_process_state(current_index, ProcessState::Ready);
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
            frame.registers[13],
            frame.registers[14],
        ];
        let process = process_record_or_panic(current_index);
        let Some(stack_range) = user_stack_range(current_index) else {
            panic!("进程 {} 的用户栈不存在", current_index);
        };
        let syscall_context =
            super::syscall::SyscallContext::new(current_index, process.program_range, stack_range);
        match super::syscall::dispatch(syscall_context, syscall_number, arguments) {
            super::syscall::SyscallOutcome::Return(result) => {
                frame.registers[10] = result as usize;
                frame.exception_pc += 4;
                return restore_user_frame(frame);
            }
            super::syscall::SyscallOutcome::Yield => {
                frame.registers[10] = 0;
                frame.exception_pc += 4;
                set_process_state(current_index, ProcessState::Ready);
                return schedule_next(current_index);
            }
            super::syscall::SyscallOutcome::Fork => {
                frame.registers[10] = match creation::fork_process(current_index, frame) {
                    Ok(child_id) => child_id.linux_pid() as usize,
                    Err(error) => {
                        crate::klog_warn!(
                            "进程 PID={} fork 失败：{}",
                            process_id_at(current_index).linux_pid(),
                            error.description()
                        );
                        let error_number = if error.is_pid_exhausted() {
                            super::syscall::process_id_exhaustion_error()
                        } else {
                            super::syscall::process_creation_error()
                        };
                        error_number as usize
                    }
                };
                frame.exception_pc += 4;
                return restore_user_frame(frame);
            }
            super::syscall::SyscallOutcome::Wait(request) => {
                frame.exception_pc += 4;
                match wait::wait_for_child(current_index, request) {
                    wait::WaitDecision::Return(result) => {
                        frame.registers[10] = result as usize;
                        return restore_user_frame(frame);
                    }
                    wait::WaitDecision::Block => {
                        return schedule_next(current_index);
                    }
                }
            }
            super::syscall::SyscallOutcome::Exit(status) => {
                crate::klog_info!(
                    "进程 PID={} 调用 exit 结束，状态码：{status}",
                    process_id_at(current_index).linux_pid()
                );
                mark_process_exited(current_index, ProcessExitStatus::Code(status));
                wait::notify_child_exit(current_index);
                return schedule_next(current_index);
            }
        }
    }

    if frame.status & SSTATUS_SPP == 0 {
        crate::klog_error!(
            "用户进程 PID={} 陷入：原因={cause:?}，scause={:#018x}，sepc={:#018x}，stval={:#018x}",
            process_id_at(current_index).linux_pid(),
            frame.cause,
            frame.exception_pc,
            frame.trap_value
        );
        mark_process_exited(
            current_index,
            ProcessExitStatus::Signal(signal_for_trap(cause)),
        );
        wait::notify_child_exit(current_index);
        crate::klog_warn!(
            "用户进程 PID={} 因异常终止",
            process_id_at(current_index).linux_pid()
        );
        return schedule_next(current_index);
    }

    panic!(
        "S-mode 陷入：进程 PID={}，原因={cause:?}，scause={:#018x}，sepc={:#018x}，stval={:#018x}",
        process_id_at(current_index).linux_pid(),
        frame.cause,
        frame.exception_pc,
        frame.trap_value
    )
}

fn schedule_next(previous_index: usize) -> *mut TrapFrame {
    let Some(next_index) = find_next_ready_process(previous_index) else {
        crate::klog_info!("所有用户进程均已退出，共运行 {} 个进程", process_count());
        switch_to_reaper(previous_index);
    };

    set_process_state(next_index, ProcessState::Running);
    // 安全性：只有单 hart 的陷入处理器修改当前进程索引。
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(CURRENT_PROCESS_INDEX), next_index);
    }

    if let Err(error) = activate_process_address_space(next_index) {
        panic!(
            "切换到进程 PID={} 的地址空间失败：{}",
            process_id_at(next_index).linux_pid(),
            error.description()
        );
    }

    if previous_index != next_index && process_state(previous_index) == ProcessState::Exited {
        reap_process_resources(previous_index, true);
    }

    if next_index != previous_index {
        crate::klog_debug!(
            "进程切换：PID {} -> PID {}",
            process_id_at(previous_index).linux_pid(),
            process_id_at(next_index).linux_pid()
        );
    }

    let next_frame = process_trap_frame_pointer(next_index);
    // 安全性：调度器只返回初始化或先前 trap 保存的专属进程陷入帧。
    let next_frame = unsafe { &mut *next_frame };
    restore_user_frame(next_frame)
}

fn arm_next_timer() {
    let Some(deadline) = crate::arch::riscv64::sbi::read_time().checked_add(TIME_SLICE_TICKS)
    else {
        panic!("计算下一次定时器截止时间时溢出");
    };

    if let Err(error) = crate::arch::riscv64::sbi::set_timer(deadline) {
        panic!("SBI 设置 supervisor 定时器失败：错误码 {}", error.0);
    }
}

fn find_next_ready_process(previous_index: usize) -> Option<usize> {
    let count = process_count();
    for offset in 1..=count {
        let candidate = (previous_index + offset) % count;
        if process_state(candidate) == ProcessState::Ready {
            return Some(candidate);
        }
    }
    None
}

fn process_kernel_stack_top(index: usize) -> usize {
    // 安全性：只有已创建且仍可调度的进程会请求栈顶；持有的内核栈页帧保持分配。
    let Some(stack) = (unsafe { &*core::ptr::addr_of!(PROCESSES) })
        .get(index)
        .and_then(|process| process.resources.as_ref())
        .map(|resources| &resources.kernel_stack)
    else {
        panic!("进程 {} 的内核栈不存在", index);
    };
    stack.top()
}

fn restore_user_frame(frame: &mut TrapFrame) -> *mut TrapFrame {
    // 安全性：陷入帧位于专用内核栈顶部下方；sscratch 指回该进程的栈顶以处理下一次陷入。
    let kernel_stack_top = frame as *mut TrapFrame as usize + core::mem::size_of::<TrapFrame>();
    unsafe { crate::arch::riscv64::trap::set_user_kernel_stack(kernel_stack_top) };
    frame as *mut TrapFrame
}

fn process_trap_frame_pointer(index: usize) -> *mut TrapFrame {
    let processes = unsafe { &*core::ptr::addr_of!(PROCESSES) };
    let process = processes
        .get(index)
        .unwrap_or_else(|| panic!("进程陷入帧索引越界：{index}"));
    if process.id.index() != index {
        panic!(
            "进程记录 ID 不匹配：索引={}，ID={}",
            index,
            process.id.index()
        );
    }
    let resources = process
        .resources
        .as_ref()
        .unwrap_or_else(|| panic!("进程 {} 的运行资源已经释放", index));
    resources.kernel_stack.trap_frame_pointer()
}

fn current_process_index() -> usize {
    // 安全性：当前进程索引只由单 hart 的启动路径和陷入处理器访问。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(CURRENT_PROCESS_INDEX)) }
}

fn process_count() -> usize {
    // 安全性：进程容器在首次进入 U-mode 前完成初始化，此后长度保持不变。
    unsafe { (&*core::ptr::addr_of!(PROCESSES)).len() }
}

fn process_id_at(index: usize) -> ProcessId {
    let processes = unsafe { &*core::ptr::addr_of!(PROCESSES) };
    let process = processes
        .get(index)
        .unwrap_or_else(|| panic!("进程索引越界：{}，进程数量={}", index, processes.len()));
    if process.id.index() != index {
        panic!(
            "进程记录 ID 不匹配：索引={}，ID={}",
            index,
            process.id.index()
        );
    }
    process.id
}

fn process_state(index: usize) -> ProcessState {
    let processes = unsafe { &*core::ptr::addr_of!(PROCESSES) };
    let process = processes
        .get(index)
        .unwrap_or_else(|| panic!("进程索引越界：{}，进程数量={}", index, processes.len()));
    if process.id.index() != index {
        panic!(
            "进程记录 ID 不匹配：索引={}，ID={}",
            index,
            process.id.index()
        );
    }
    process.state
}

fn set_process_state(index: usize, state: ProcessState) {
    let Some(process) = (unsafe { &mut *core::ptr::addr_of_mut!(PROCESSES) }).get_mut(index) else {
        panic!("无法更新不存在的进程 {} 的状态", index);
    };
    // 安全性：状态只在单 hart 启动或陷入路径更新；S-mode 陷入期间不会发生定时器重入。
    process.state = state;
}

fn mark_process_exited(index: usize, exit_status: ProcessExitStatus) {
    let process_id = {
        let Some(process) = (unsafe { &mut *core::ptr::addr_of_mut!(PROCESSES) }).get_mut(index)
        else {
            panic!("无法结束不存在的进程 {}", index);
        };
        process.state = ProcessState::Exited;
        process.exit_status = Some(exit_status);
        process.wait_request = None;
        process.id
    };
    wait::orphan_children(process_id);
}

fn signal_for_trap(cause: TrapCause) -> u8 {
    match cause {
        TrapCause::Exception(ExceptionCause::IllegalInstruction) => 4,
        TrapCause::Exception(ExceptionCause::Breakpoint) => 5,
        TrapCause::Exception(
            ExceptionCause::LoadAddressMisaligned | ExceptionCause::StoreAmoAddressMisaligned,
        ) => 7,
        TrapCause::Exception(
            ExceptionCause::InstructionAccessFault
            | ExceptionCause::LoadAccessFault
            | ExceptionCause::StoreAmoAccessFault
            | ExceptionCause::InstructionPageFault
            | ExceptionCause::LoadPageFault
            | ExceptionCause::StoreAmoPageFault,
        ) => 11,
        _ => 4,
    }
}

#[derive(Clone, Copy)]
struct ProcessSnapshot {
    program_range: (usize, usize),
}

fn process_record_or_panic(index: usize) -> ProcessSnapshot {
    // 安全性：进程记录只追加、不移动其独立 backing；快照不暴露容器元素引用。
    let Some(process) = (unsafe { &*core::ptr::addr_of!(PROCESSES) }).get(index) else {
        panic!("进程索引越界：{}，进程数量={}", index, process_count());
    };
    if process.id.index() != index {
        panic!(
            "进程记录 ID 不匹配：索引={}，ID={}",
            index,
            process.id.index()
        );
    }
    let Some(resources) = process.resources.as_ref() else {
        panic!("进程 {} 的运行资源已经释放", index);
    };
    ProcessSnapshot {
        program_range: resources.program_range,
    }
}

fn user_stack_range(index: usize) -> Option<(usize, usize)> {
    // 安全性：共享引用仅读取该进程仍拥有的用户栈描述；退出进程不再接受 syscall。
    unsafe { &*core::ptr::addr_of!(PROCESSES) }
        .get(index)?
        .resources
        .as_ref()
        .map(|resources| resources.user_stack.range())
}

fn activate_process_address_space(index: usize) -> Result<(), super::memory::PageTableError> {
    let processes = unsafe { &*core::ptr::addr_of!(PROCESSES) };
    let process = processes
        .get(index)
        .unwrap_or_else(|| panic!("进程索引越界：{}，进程数量={}", index, processes.len()));
    let address_space = &process
        .resources
        .as_ref()
        .unwrap_or_else(|| panic!("进程 {} 的运行资源已经释放", index))
        .address_space;
    super::memory::address_space::activate_address_space(address_space)
}

fn reap_process_resources(index: usize, defer_kernel_stack: bool) {
    // 安全性：调用方已切换到其他地址空间；当前进程的页表、用户内存和栈不再活动。
    let process = unsafe { &mut *core::ptr::addr_of_mut!(PROCESSES) }
        .get_mut(index)
        .unwrap_or_else(|| panic!("无法回收不存在的进程 {}", index));
    if process.state != ProcessState::Exited {
        panic!("拒绝回收仍在运行的进程 {}", index);
    }
    let process_id = process.id;
    let parent_id = process.parent_id;
    let exit_status = process.exit_status;
    let resources = process
        .resources
        .take()
        .unwrap_or_else(|| panic!("进程 {} 的运行资源已经回收", index));
    let ProcessResources {
        address_space,
        program_image,
        user_stack,
        kernel_stack,
        ..
    } = resources;
    let released_page_count = super::memory::address_space::destroy_address_space(address_space)
        .unwrap_or_else(|error| panic!("回收进程 {} 的页表失败：{}", index, error.description()));
    drop(program_image);
    drop(user_stack);

    if defer_kernel_stack {
        defer_kernel_stack_release(ProcessId::from_index(index), kernel_stack);
    } else {
        drop(kernel_stack);
    }
    if process.parent_id.is_none() || process.wait_collected {
        process.state = ProcessState::Reaped;
    }
    crate::klog_info!(
        "已回收进程 PID={} 的运行资源：父进程 PID={:?}，退出状态={:?}，释放 {} 个页表页",
        process_id.linux_pid(),
        parent_id.map(ProcessId::linux_pid),
        exit_status,
        released_page_count
    );
}

fn log_process_address_spaces() {
    let processes = unsafe { &*core::ptr::addr_of!(PROCESSES) };
    for process in processes {
        let Some(resources) = process.resources.as_ref() else {
            continue;
        };
        super::memory::page_table_log::log_address_space(
            process.id.index(),
            &resources.address_space,
        );
    }
}

fn defer_kernel_stack_release(process_id: ProcessId, kernel_stack: KernelStack) {
    // 安全性：当前仍在被退出进程的内核栈上运行，因此暂存其所有权，稍后在安全栈上释放。
    unsafe {
        let pending = core::ptr::addr_of_mut!(PENDING_KERNEL_STACK_RELEASE);
        if (&*pending).is_some() {
            panic!("已有尚未释放的进程内核栈");
        }
        core::ptr::write_volatile(pending, Some((process_id, kernel_stack)));
    }
}

fn release_deferred_kernel_stack(current_index: usize) {
    // 安全性：此函数在新进程自己的陷入栈上运行；待释放栈属于已经退出的其他进程。
    let pending = unsafe {
        let slot = core::ptr::addr_of_mut!(PENDING_KERNEL_STACK_RELEASE);
        let pending = core::ptr::read_volatile(slot);
        core::ptr::write_volatile(slot, None);
        pending
    };
    let Some((process_id, kernel_stack)) = pending else {
        return;
    };
    if process_id.index() == current_index {
        panic!("拒绝释放当前进程正在使用的内核栈");
    }
    // 安全性：当前陷入帧已位于另一个进程的栈上，暂存的栈不再活动。
    drop(kernel_stack);
}

fn switch_to_reaper(process_index: usize) -> ! {
    let reaper_stack_top = core::ptr::addr_of!(__boot_stack_virtual_end) as usize;
    // 安全性：启动栈在每张进程页表中保持高半区映射；续接函数不返回旧进程栈。
    unsafe {
        crate::arch::riscv64::trap::call_on_stack(
            reaper_stack_top,
            reap_last_process_on_reaper_stack,
            process_index,
        )
    }
}

extern "C" fn reap_last_process_on_reaper_stack(process_index: usize) -> ! {
    if let Err(error) = super::memory::address_space::activate_boot_address_space() {
        panic!("切换到内核回收页表失败：{}", error.description());
    }

    // 安全性：当前已运行在不属于任何用户进程的启动栈和内核页表上，可释放最后一个进程的栈与页表。
    reap_process_resources(process_index, false);
    crate::klog_info!("最后一个用户进程已回收，内核进入空闲等待");
    idle_forever()
}

fn idle_forever() -> ! {
    loop {
        // 安全性：所有用户进程资源均已回收；WFI 让 hart 等待，不再恢复已退出进程。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

extern "C" {
    static __boot_stack_virtual_start: u8;
    static __boot_stack_virtual_end: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __data_end: u8;
    static __bss_start: u8;
    static __bss_end: u8;
}
