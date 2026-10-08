//! 在单 hart 上启动并调度多个同时驻留的 U-mode 程序。

use crate::arch::riscv64::trap::{ExceptionCause, InterruptCause, TrapCause, TrapFrame};

const MAX_USER_TASKS: usize = 8;
const BOOT_PROGRAM_NAME: &str = match option_env!("FULLEMU_BOOT_PROGRAM") {
    Some(name) => name,
    None => "shell",
};
const USER_STACK_SIZE: usize = 16 * 1024;
const KERNEL_TRAP_STACK_SIZE: usize = 16 * 1024;
const SSTATUS_SPP: usize = 1 << 8;
// 按 QEMU virt 的时间基准将时间片固定为 10 ms。
const TIME_SLICE_TICKS: u64 = crate::arch::riscv64::sbi::QEMU_VIRT_TIMEBASE_FREQUENCY_HZ / 100;

#[repr(align(4096))]
#[derive(Clone, Copy)]
struct TaskStack([u8; USER_STACK_SIZE]);

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaskState {
    Empty,
    Ready,
    Running,
    Exited,
    Faulted,
}

static mut USER_STACKS: [TaskStack; MAX_USER_TASKS] =
    [TaskStack([0; USER_STACK_SIZE]); MAX_USER_TASKS];
static mut KERNEL_TRAP_STACKS: [TaskStack; MAX_USER_TASKS] =
    [TaskStack([0; USER_STACK_SIZE]); MAX_USER_TASKS];
static mut TASK_STATES: [TaskState; MAX_USER_TASKS] = [TaskState::Empty; MAX_USER_TASKS];
static mut SAVED_FRAMES: [usize; MAX_USER_TASKS] = [0; MAX_USER_TASKS];
static mut TASK_PROGRAM_INDICES: [usize; MAX_USER_TASKS] = [0; MAX_USER_TASKS];
static mut TASK_COUNT: usize = 0;
static mut CURRENT_TASK_INDEX: usize = 0;

/// 装入构建配置指定的入口用户程序并开始运行。
pub fn run_boot_program() -> ! {
    let Some(program_index) = super::user_program::find_index(BOOT_PROGRAM_NAME) else {
        crate::klog_error!("用户程序清单中没有入口程序：{BOOT_PROGRAM_NAME}");
        stop_forever();
    };

    start_programs(&[program_index]);
}

/// 同时装入并调度构建清单中的全部程序。
pub fn run_batch_programs() -> ! {
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

    // 安全性：初始化只在单 hart 启动路径执行；传入的任务数量已限制在静态数组容量内。
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(TASK_COUNT), program_indices.len());
        core::ptr::write_volatile(core::ptr::addr_of_mut!(CURRENT_TASK_INDEX), 0);
    }

    let mut user_stack_ranges = [(0, 0); MAX_USER_TASKS];
    let mut user_image_ranges = [(0, 0); MAX_USER_TASKS];

    for (task_index, program_index) in program_indices.iter().copied().enumerate() {
        // 安全性：任务索引来自已检查长度的切片，程序索引由启动策略从构建清单生成。
        unsafe {
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!(TASK_PROGRAM_INDICES)
                    .cast::<usize>()
                    .add(task_index),
                program_index,
            );
        }

        let program = match super::user_program::load(program_index) {
            Ok(Some(program)) => program,
            Ok(None) => {
                crate::klog_error!("用户程序索引不存在：{program_index}");
                stop_forever();
            }
            Err(error) => {
                report_load_error(error);
                stop_forever();
            }
        };

        let user_stack_top = reset_user_stack(task_index);
        let Some(user_stack_range) = user_stack_range(task_index) else {
            crate::klog_error!("无法计算任务 {} 的用户栈范围", task_index);
            stop_forever();
        };
        user_stack_ranges[task_index] = user_stack_range;
        user_image_ranges[task_index] = (program.slot_start, program.slot_end);

        let kernel_stack_top = task_kernel_stack_top(task_index);
        let initial_frame_address = kernel_stack_top - core::mem::size_of::<TrapFrame>();
        let initial_frame = TrapFrame::for_user_entry(program.entry, user_stack_top);

        // 安全性：陷入帧位于当前任务专属内核栈顶部，保存现场的数组索引已验证有效。
        unsafe {
            core::ptr::write(initial_frame_address as *mut TrapFrame, initial_frame);
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!(SAVED_FRAMES)
                    .cast::<usize>()
                    .add(task_index),
                initial_frame_address,
            );
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!(TASK_STATES)
                    .cast::<TaskState>()
                    .add(task_index),
                TaskState::Ready,
            );
        }

        crate::klog_info!(
            "用户任务已装入：任务={}，程序={}，入口 {:#018x}，镜像 {} 字节",
            task_index,
            program.name,
            program.entry,
            program.image_size
        );
    }

    // 安全性：页表初始化前确保仍处于恒等寻址模式，并关闭中断避免启动状态被抢占。
    unsafe {
        core::arch::asm!(
            "csrw sie, zero",
            "csrw satp, zero",
            "sfence.vma zero, zero",
            options(nostack)
        );
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(TASK_STATES).cast::<TaskState>(),
            TaskState::Running,
        );
    }

    // 安全性：链接脚本将内核、全部用户镜像槽位和静态栈放在恒等映射地址；范围按页对齐。
    let kernel_end = core::ptr::addr_of!(__bss_end) as usize;
    if let Err(error) = super::memory::initialize(
        kernel_end,
        &user_stack_ranges[..program_indices.len()],
        &user_image_ranges[..program_indices.len()],
    ) {
        crate::klog_error!("Sv39 初始化失败：{}", error.description());
        stop_forever();
    }

    crate::klog_info!("已启用 Sv39 用户/内核页权限边界");
    crate::klog_info!(
        "启动 {} 个用户任务；定时器中断保持调度响应",
        program_indices.len()
    );

    arm_next_timer_or_stop();
    crate::arch::riscv64::interrupt::enable_supervisor_timer();

    let first_frame = saved_frame_pointer(0);
    // 安全性：首个陷入帧及其用户栈、内核栈均为静态分配且按 16 字节对齐。
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
        match super::syscall::dispatch(
            current_index,
            task_program_index(current_index),
            syscall_number,
            arguments,
        ) {
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
        set_task_state(current_index, TaskState::Faulted);
        crate::klog_warn!("用户任务 {} 因异常终止", current_index);
        return schedule_next(current_index);
    }

    stop_forever()
}

fn schedule_next(previous_index: usize) -> *mut TrapFrame {
    let Some(next_index) = find_next_ready_task(previous_index) else {
        crate::klog_info!(
            "所有用户任务均已结束或异常终止，共运行 {} 个任务",
            task_count()
        );
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

fn reset_user_stack(index: usize) -> usize {
    // 安全性：索引已通过任务数量检查；初始化时该任务尚未执行。
    let stack = unsafe {
        core::ptr::addr_of_mut!(USER_STACKS)
            .cast::<TaskStack>()
            .add(index)
    };
    // 安全性：启动或任务首次初始化时，当前任务栈尚无有效现场需要保留。
    unsafe {
        core::ptr::write_bytes(
            core::ptr::addr_of_mut!((*stack).0).cast::<u8>(),
            0,
            USER_STACK_SIZE,
        );
    }
    stack_top(stack, USER_STACK_SIZE)
}

fn task_kernel_stack_top(index: usize) -> usize {
    // 安全性：索引已通过任务数量检查，地址计算不解引用栈内容。
    let stack = unsafe {
        core::ptr::addr_of_mut!(KERNEL_TRAP_STACKS)
            .cast::<TaskStack>()
            .add(index)
    };
    stack_top(stack, KERNEL_TRAP_STACK_SIZE)
}

fn stack_top(stack: *mut TaskStack, size: usize) -> usize {
    // 安全性：调用方传入静态 TaskStack 的有效裸指针；只计算数组末尾地址，不解引用。
    unsafe { core::ptr::addr_of_mut!((*stack).0).cast::<u8>().add(size) as usize }
}

fn restore_user_frame(frame: &mut TrapFrame) -> *mut TrapFrame {
    // 安全性：陷入帧位于专用内核栈顶部下方；sscratch 指回该任务的栈顶以处理下一次陷入。
    let kernel_stack_top = frame as *mut TrapFrame as usize + core::mem::size_of::<TrapFrame>();
    unsafe { crate::arch::riscv64::trap::set_user_kernel_stack(kernel_stack_top) };
    frame as *mut TrapFrame
}

fn saved_frame_pointer(index: usize) -> *mut TrapFrame {
    // 安全性：索引已限制在任务数组范围内，陷入帧在该任务栈生命周期内保持有效。
    unsafe {
        core::ptr::read_volatile(core::ptr::addr_of!(SAVED_FRAMES).cast::<usize>().add(index))
            as *mut TrapFrame
    }
}

fn save_current_frame(index: usize, frame: &mut TrapFrame) {
    // 安全性：当前陷入帧由本 hart 独占，索引是当前正在运行的有效任务。
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(SAVED_FRAMES)
                .cast::<usize>()
                .add(index),
            frame as *mut TrapFrame as usize,
        );
    }
}

fn current_task_index() -> usize {
    // 安全性：当前任务索引只由单 hart 的启动路径和陷入处理器访问。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(CURRENT_TASK_INDEX)) }
}

fn task_program_index(index: usize) -> usize {
    // 安全性：当前任务索引由调度器限制在已创建任务范围内。
    unsafe {
        core::ptr::read_volatile(
            core::ptr::addr_of!(TASK_PROGRAM_INDICES)
                .cast::<usize>()
                .add(index),
        )
    }
}

fn task_count() -> usize {
    // 安全性：任务数量在首次进入 U-mode 前初始化，此后保持不变。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(TASK_COUNT)) }
}

fn task_state(index: usize) -> TaskState {
    // 安全性：调用方只传入已创建任务索引，单 hart 保证状态读写不会并发。
    unsafe {
        core::ptr::read_volatile(
            core::ptr::addr_of!(TASK_STATES)
                .cast::<TaskState>()
                .add(index),
        )
    }
}

fn set_task_state(index: usize, state: TaskState) {
    // 安全性：调用方只传入已创建任务索引，单 hart 保证状态读写不会并发。
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(TASK_STATES)
                .cast::<TaskState>()
                .add(index),
            state,
        );
    }
}

/// 判断缓冲区是否完整位于指定任务的 U-mode 栈中。
pub(super) fn contains_user_stack_range(index: usize, address: usize, length: usize) -> bool {
    if length == 0 {
        return true;
    }
    if index >= task_count() {
        return false;
    }

    let Some((stack_start, stack_end)) = user_stack_range(index) else {
        return false;
    };
    let Some(buffer_end) = address.checked_add(length) else {
        return false;
    };

    address >= stack_start && buffer_end <= stack_end
}

fn user_stack_range(index: usize) -> Option<(usize, usize)> {
    if index >= MAX_USER_TASKS {
        return None;
    }
    let stack_offset = index.checked_mul(core::mem::size_of::<TaskStack>())?;
    let stack_start = (core::ptr::addr_of!(USER_STACKS) as usize).checked_add(stack_offset)?;
    let stack_end = stack_start.checked_add(USER_STACK_SIZE)?;
    Some((stack_start, stack_end))
}

fn report_load_error(error: super::user_program::UserProgramLoadError) {
    crate::klog_error!(
        "用户程序 {} 加载失败：{}（镜像 {} 字节，加载槽位 {} 字节）",
        error.program_name,
        error.description(),
        error.image_size,
        error.capacity
    );
}

fn stop_forever() -> ! {
    loop {
        // 安全性：当前没有可运行任务；WFI 让 hart 等待，不再恢复已退出或异常的任务。
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

extern "C" {
    static __bss_end: u8;
}
