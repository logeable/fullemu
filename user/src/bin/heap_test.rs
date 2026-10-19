#![no_std]
#![no_main]

//! 验证进程堆增长、缩小、清零以及 fork 后的数据隔离。

const PAGE_SIZE: usize = 4096;

pub fn main(_arguments: fullemu_user::UserArguments) {
    let initial_break = fullemu_user::syscall::brk(0);
    let Some(requested_break) = initial_break.checked_add(PAGE_SIZE * 2 + 17) else {
        fail("用户堆地址溢出");
    };
    if fullemu_user::syscall::brk(requested_break) != requested_break {
        fail("内核未能扩展用户堆");
    }

    // 安全性：brk 成功后，三个探测地址均位于内核映射的用户堆页内。
    unsafe {
        core::ptr::write_volatile(initial_break as *mut u8, 0x31);
        core::ptr::write_volatile((initial_break + PAGE_SIZE) as *mut u8, 0x52);
        core::ptr::write_volatile((requested_break - 1) as *mut u8, 0x73);
        if core::ptr::read_volatile(initial_break as *const u8) != 0x31
            || core::ptr::read_volatile((initial_break + PAGE_SIZE) as *const u8) != 0x52
            || core::ptr::read_volatile((requested_break - 1) as *const u8) != 0x73
        {
            fail("用户堆跨页读写不一致");
        }
    }

    if fullemu_user::syscall::brk(usize::MAX) != requested_break {
        fail("无效 brk 请求没有保留原边界");
    }

    let child_pid = fullemu_user::syscall::fork();
    if child_pid < 0 {
        fail("带用户堆的 fork 失败");
    }
    if child_pid == 0 {
        // 安全性：fork 复制的堆覆盖 initial_break，且子进程地址空间独立。
        unsafe { core::ptr::write_volatile(initial_break as *mut u8, 0x99) };
        fullemu_user::syscall::exit(0);
    }

    let mut status = 0;
    if fullemu_user::syscall::waitpid(child_pid as i32, Some(&mut status)) != child_pid
        || status != 0
    {
        fail("等待堆测试子进程失败");
    }
    // 安全性：父进程堆仍映射到自己的页帧；子进程修改不应改变此字节。
    if unsafe { core::ptr::read_volatile(initial_break as *const u8) } != 0x31 {
        fail("fork 后父子进程共享了堆页");
    }

    if fullemu_user::syscall::brk(initial_break) != initial_break
        || fullemu_user::syscall::brk(requested_break) != requested_break
    {
        fail("用户堆缩小或再次扩展失败");
    }
    // 安全性：缩小释放了完整堆页，再次扩展会映射内核清零的新物理页帧。
    unsafe {
        if core::ptr::read_volatile(initial_break as *const u8) != 0
            || core::ptr::read_volatile((initial_break + PAGE_SIZE) as *const u8) != 0
            || core::ptr::read_volatile((requested_break - 1) as *const u8) != 0
        {
            fail("重新分配的用户堆页没有清零");
        }
    }

    fullemu_user::println!("用户堆验证通过：brk、跨页访问、fork 隔离、缩小回收和重新清零");
}

fn fail(message: &str) -> ! {
    fullemu_user::println!("用户堆验证失败：{}", message);
    fullemu_user::syscall::exit(1)
}

fullemu_user::user_entry!(main);
