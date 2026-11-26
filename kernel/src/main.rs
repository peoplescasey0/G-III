// POST CARD SPATIAL OS - MULTIBOOT2 KERNEL
// Boots via GRUB using multiboot2 protocol

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod coprocessor;
mod framebuffer;
mod gdt;
mod hal_ime;
mod heap;
mod idt;
mod keyboard;
mod memory;
mod paging;
mod pci;
mod pic;
mod pipeline;
mod ramdisk;
mod sandbox;
mod sched;
mod shell;
mod spatial_wm;
mod storage;
mod sync;
mod usermode;
mod vga;

use core::panic::PanicInfo;
use core::arch::global_asm;

// The multiboot2 header MUST be in the first 32KB of the file.
// We place it in .text.boot which the linker puts at the very start.
// The header is followed immediately by the 64-bit entry stub.
global_asm!(
    // Put everything in .text.boot - the linker script places this FIRST
    ".section .text.boot, \"ax\"",

    // ── Multiboot2 header (for GRUB2 ISO / disk boot) ──────────────
    ".align 8",
    "multiboot2_start:",
    "   .long 0xe85250d6",          // magic
    "   .long 0",                   // architecture: 0 = i386 protected mode
    "   .long multiboot2_end - multiboot2_start", // header length
    // checksum: magic + arch + length + checksum must equal 0 mod 2^32
    "   .long -(0xe85250d6 + 0 + (multiboot2_end - multiboot2_start))",
    // end tag
    "   .short 0",                  // type
    "   .short 0",                  // flags
    "   .long 8",                   // size
    "multiboot2_end:",

    // ── Multiboot1 header (for direct `qemu-system-x86_64 -kernel`) ─
    ".align 4",
    "multiboot1_start:",
    "   .long 0x1badb002",          // Multiboot1 magic
    "   .long 0x00000003",          // flags: page_align (bit 0) | mem_info (bit 1)
    "   .long -(0x1badb002 + 0x00000003)", // checksum

    // ── Entry stub ────────────────────────────────────────────────
    // GRUB or QEMU hands off in 32-bit protected mode; switch to 64-bit.
    ".code32",
    ".global entry32",
    "entry32:",
    "   cli",
    // Preserve bootloader magic from EAX in EBP (EBX holds info struct pointer).
    "   mov ebp, eax",
    // Build an identity map for the first GiB using 2 MiB pages.
    "   mov eax, offset boot_pdpt",
    "   or eax, 0x3",
    "   mov dword ptr [boot_pml4], eax",
    "   mov eax, offset boot_pd",
    "   or eax, 0x3",
    "   mov dword ptr [boot_pdpt], eax",
    "   xor ecx, ecx",
    "2:",
    "   mov eax, ecx",
    "   shl eax, 21",
    "   or eax, 0x83",
    "   mov dword ptr [boot_pd + ecx * 8], eax",
    "   mov dword ptr [boot_pd + ecx * 8 + 4], 0",
    "   inc ecx",
    "   cmp ecx, 512",
    "   jne 2b",
    // Enable PAE, SSE (OSFXSR + OSXMMEXCPT), long mode, and paging before loading the 64-bit GDT.
    "   mov eax, cr4",
    "   or eax, 0x620",
    "   mov cr4, eax",
    "   mov eax, offset boot_pml4",
    "   mov cr3, eax",
    "   mov ecx, 0xC0000080",
    "   rdmsr",
    "   or eax, 0x100",
    "   wrmsr",
    "   lgdt [boot_gdt_ptr]",
    "   mov eax, cr0",
    "   and eax, 0xFFFFFFFB",       // Clear CR0.EM (bit 2) for hardware SSE f32/f64
    "   or eax, 0x80000002",        // Set CR0.PG (bit 31) | CR0.MP (bit 1)
    "   mov cr0, eax",
    "   .byte 0xea",
    "   .long long_mode_start",
    "   .word 0x08",
    ".code64",
    "long_mode_start:",
    "   mov ax, 0x10",
    "   mov ds, ax",
    "   mov es, ax",
    "   mov ss, ax",
    "   lea rsp, [rip + boot_stack_top]",
    "   and rsp, -16",
    // EBX holds the Multiboot1/2 info pointer; EBP holds the magic from EAX.
    "   mov edi, ebx",
    "   mov esi, ebp",
    "   xor rbp, rbp",
    "   call _start",
    "1: hlt",
    "   jmp 1b",

    ".section .rodata",
    ".align 8",
    "boot_gdt:",
    "   .quad 0",
    "   .quad 0x00af9a000000ffff",
    "   .quad 0x00cf92000000ffff",
    "boot_gdt_end:",
    "boot_gdt_ptr:",
    "   .word boot_gdt_end - boot_gdt - 1",
    "   .long boot_gdt",

    ".section .bss",
    ".align 4096",
    "boot_pml4:",
    "   .skip 4096",
    "boot_pdpt:",
    "   .skip 4096",
    "boot_pd:",
    "   .skip 4096",
    ".align 4096",
    ".global boot_stack_guard",
    "boot_stack_guard:",
    "   .skip 4096",
    "boot_stack:",
    "   .skip 65536",
    "boot_stack_top:",
);
// Minimal serial implementation
mod serial {
    use uart_16550::SerialPort;
    use crate::sync::Mutex;
    use core::fmt;

    pub static SERIAL1: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(0x3F8) });

    pub fn init() {
        SERIAL1.lock().init();
    }

    /// Non-blocking read of one byte from COM1.
    pub fn try_read() -> Option<u8> {
        use x86_64::instructions::port::Port;
        let _guard = SERIAL1.lock(); // don't interleave with a writer
        unsafe {
            if Port::<u8>::new(0x3F8 + 5).read() & 1 != 0 {
                Some(Port::<u8>::new(0x3F8).read())
            } else {
                None
            }
        }
    }

    #[doc(hidden)]
    pub fn _print(args: fmt::Arguments) {
        use core::fmt::Write;
        SERIAL1.lock().write_fmt(args).expect("Printing to serial failed");
    }

    #[macro_export]
    macro_rules! serial_print {
        ($($arg:tt)*) => {
            $crate::serial::_print(format_args!($($arg)*))
        };
    }

    #[macro_export]
    macro_rules! serial_println {
        () => ($crate::serial_print!("\n"));
        ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
        ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(concat!($fmt, "\n"), $($arg)*));
    }
}

// Entry point - called from the 32-bit stub above
#[no_mangle]
pub extern "C" fn _start(mb_info: usize, mb_magic: u32) -> ! {
    // Must run before anything else: the info struct may overlap our bss.
    unsafe { memory::init(mb_info, mb_magic) };
    serial::init();

    serial_println!("POST CARD SPATIAL OS - BOOTED!");
    serial_println!("================================");
    serial_println!("Kernel: multiboot2 via GRUB, 64-bit long mode");
    serial_println!("");

    gdt::init();
    serial_println!("[ok] GDT + TSS loaded");
    idt::init();
    serial_println!("[ok] IDT loaded");

    // Prove exceptions work: breakpoint must be handled and execution resume.
    x86_64::instructions::interrupts::int3();
    serial_println!("[ok] breakpoint exception handled, execution resumed");

    let (is, ie) = memory::info_range();
    serial_println!("[..] multiboot2 info was at {:#x}-{:#x} (copied, no longer used)", is, ie);
    serial_println!("[..] kernel image ends at {:#x}", memory::kernel_end_addr());
    let total = memory::report();
    serial_println!("[ok] {} MiB usable RAM", total / (1024 * 1024));

    match unsafe { heap::init() } {
        Some((start, size)) => {
            serial_println!("[ok] heap: {:#x}, {} KiB", start, size / 1024);
            heap_selftest();
            let n = unsafe { paging::init_frames(start + size) };
            serial_println!("[ok] frame allocator: {} frames ({} MiB) above heap", n, n * 4 / 1024);
            unsafe { paging::remap_kernel() };
            serial_println!("[ok] kernel page tables active (W^X, NX, null + stack guard pages)");
            remap_check();
            paging_selftest();
        }
        None => serial_println!("[!!] no suitable region for heap"),
    }

    pic::init();
    serial_println!("[ok] PIC remapped to vectors 32-47, PIT at {} Hz", pic::TIMER_HZ);
    x86_64::instructions::interrupts::enable();
    while pic::ticks() < pic::TIMER_HZ {
        x86_64::instructions::hlt();
    }
    serial_println!("[ok] timer IRQ: {} ticks received (~1 second)", pic::ticks());

    serial_println!("");
    serial_println!("Spatial OS is ALIVE!");

    sched::init("shell");
    sched_selftest();
    preempt_selftest();
    usermode::init();
    usermode_selftest();
    pci_vfio_selftest();
    storage_spool_selftest();
    sandbox_contract_selftest();
    cxl_boundary_selftest();
    pipeline_sdf_selftest();
    tiered_storage_selftest();
    hal_ime_selftest();
    framebuffer_selftest();
    spatial_wm_selftest();
    neural_copilot_selftest();
    patent_suite_selftest();

    let mut shell = shell::Shell::new();
    let mut decoder = keyboard::Decoder::new();
    let mut ansi = keyboard::AnsiDecoder::new();
    shell.start();
    loop {
        while let Some(sc) = keyboard::pop() {
            if let Some(key) = decoder.feed(sc) {
                shell.input(key);
            }
        }
        while let Some(b) = serial::try_read() {
            if let Some(key) = ansi.feed(b) {
                shell.input(key);
            }
        }
        sched::yield_now();
        x86_64::instructions::hlt();
    }
}

fn preempt_selftest() {
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
    static STOP: AtomicBool = AtomicBool::new(false);
    static SPIN_A: AtomicU64 = AtomicU64::new(0);
    static SPIN_B: AtomicU64 = AtomicU64::new(0);
    static DONE: AtomicU64 = AtomicU64::new(0);

    sched::enable_preemption();

    // Two busy loops that never yield must both make progress.
    sched::spawn("spinA", || while !STOP.load(SeqCst) { SPIN_A.fetch_add(1, SeqCst); });
    sched::spawn("spinB", || while !STOP.load(SeqCst) { SPIN_B.fetch_add(1, SeqCst); });
    // Heap + serial users that get preempted at arbitrary points: no deadlock,
    // no corruption.
    for name in ["allocA", "allocB"] {
        sched::spawn(name, move || {
            let mut sum = 0u64;
            for i in 0..3000u64 {
                let v: alloc::vec::Vec<u64> = (0..64).map(|x| x + i).collect();
                sum += v.iter().sum::<u64>();
                if i % 1000 == 0 {
                    serial_println!("[{}] iteration {}", name, i);
                }
            }
            core::hint::black_box(sum);
            DONE.fetch_add(1, SeqCst);
        });
    }

    let t0 = pic::ticks();
    while pic::ticks() < t0 + 30 {
        x86_64::instructions::hlt();
    }
    STOP.store(true, SeqCst);
    while sched::live_tasks() > 1 {
        x86_64::instructions::hlt();
    }
    let (a, b) = (SPIN_A.load(SeqCst), SPIN_B.load(SeqCst));
    assert!(a > 0 && b > 0, "a spinner starved: {} {}", a, b);
    assert_eq!(DONE.load(SeqCst), 2, "alloc tasks did not finish");
    serial_println!("[ok] preemption: non-yielding tasks both ran (A={}, B={}), alloc/serial stress ok", a, b);
}

fn usermode_selftest() {
    use core::sync::atomic::Ordering::SeqCst;
    let wait_all = || {
        let t0 = pic::ticks();
        while sched::live_tasks() > 1 {
            assert!(pic::ticks() < t0 + 500, "user process did not finish");
            x86_64::instructions::hlt();
        }
    };

    let hello = usermode::spawn("hello", usermode::prog_hello());
    wait_all();
    assert_eq!(usermode::exit_code(hello), Some(42), "user exit code");
    serial_println!("[ok] user mode: ring 3 process ran, syscalls write/sleep/exit worked (exit code 42)");
    let frames = paging::frames_in_use();

    usermode::spawn("evil", usermode::prog_evil());
    wait_all();
    assert_eq!(usermode::KILLED.load(SeqCst), 1, "evil process should have been killed");
    assert_eq!(paging::frames_in_use(), frames, "user pages leaked");
    serial_println!("[ok] user mode: kernel pointer rejected, kernel read faulted, process killed, no frames leaked");

    // Two processes use the same virtual address at the same time.
    let poke = usermode::spawn("poke", usermode::prog_poke());
    let peek = usermode::spawn("peek", usermode::prog_peek());
    wait_all();
    assert_eq!(usermode::exit_code(peek), Some(5), "peek saw another process's memory");
    assert_eq!(usermode::exit_code(poke), Some(7), "poke's data was corrupted");
    assert_eq!(paging::frames_in_use(), frames, "address space leaked");
    serial_println!("[ok] address spaces: same virtual address, separate memory (poke/peek), all page tables freed");
}

fn sched_selftest() {
    use spin::Mutex;
    static LOG: Mutex<alloc::string::String> = Mutex::new(alloc::string::String::new());
    static WOKE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

    // Two tasks alternating via yield_now must interleave strictly.
    for (name, ch) in [("A", 'A'), ("B", 'B')] {
        sched::spawn(name, move || {
            for _ in 0..3 {
                LOG.lock().push(ch);
                sched::yield_now();
            }
        });
    }
    while sched::live_tasks() > 1 {
        sched::yield_now();
    }
    assert_eq!(LOG.lock().as_str(), "ABABAB", "yield order");
    serial_println!("[ok] scheduler: 2 tasks interleaved via yield ({})", LOG.lock().as_str());

    // A sleeping task must wake no earlier than requested while others run.
    let t0 = pic::ticks();
    sched::spawn("sleeper", || {
        sched::sleep(50);
        WOKE.store(pic::ticks(), core::sync::atomic::Ordering::SeqCst);
    });
    while sched::live_tasks() > 1 {
        sched::yield_now();
        x86_64::instructions::hlt();
    }
    let dt = WOKE.load(core::sync::atomic::Ordering::SeqCst) - t0;
    assert!(dt >= 50 && dt < 70, "sleep took {} ticks", dt);
    serial_println!("[ok] scheduler: sleep(50) woke after {} ticks, task exited and was reaped", dt);
}

fn remap_check() {
    use x86_64::structures::paging::PageTableFlags as F;
    static DATA_PROBE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);
    static RO_PROBE: [u8; 8] = *b"rodata!!";
    let text = remap_check as usize as u64;
    let data = core::ptr::addr_of!(DATA_PROBE) as u64;
    let ro = core::ptr::addr_of!(RO_PROBE) as u64;
    let tf = paging::page_flags(text).expect("text mapped");
    let df = paging::page_flags(data).expect("data mapped");
    let rf = paging::page_flags(ro).expect("rodata mapped");
    assert!(!tf.contains(F::WRITABLE) && !tf.contains(F::NO_EXECUTE), "text must be RX");
    assert!(df.contains(F::WRITABLE) && df.contains(F::NO_EXECUTE), "data must be RW+NX");
    assert!(!rf.contains(F::WRITABLE) && rf.contains(F::NO_EXECUTE), "rodata must be R+NX");
    assert!(paging::translate(0).is_none(), "null page mapped");
    serial_println!("[ok] layout: text {:#x} RX, rodata {:#x} R, data {:#x} RW+NX, null page unmapped", text, ro, data);
}

fn paging_selftest() {
    use x86_64::structures::paging::PageTableFlags as F;
    const A: u64 = 0x4000_0000; // 1 GiB: outside the boot identity map
    const B: u64 = 0x4000_1000;
    let rw = F::WRITABLE;

    let before = paging::frames_in_use();
    assert!(paging::translate(A).is_none(), "A already mapped");
    let pa = paging::map_new_page(A, rw).expect("map A");
    let pb = paging::map_new_page(B, rw).expect("map B");
    assert_eq!(paging::translate(A), Some(pa));
    assert_eq!(paging::translate(B), Some(pb));
    unsafe {
        (A as *mut u64).write_volatile(0xDEAD_BEEF_CAFE_F00D);
        (B as *mut u64).write_volatile(0x1122_3344_5566_7788);
        assert_eq!((A as *const u64).read_volatile(), 0xDEAD_BEEF_CAFE_F00D);
        assert_eq!((B as *const u64).read_volatile(), 0x1122_3344_5566_7788);
        // Same data visible through the identity mapping of the frame.
        assert_eq!((pa.as_u64() as *const u64).read_volatile(), 0xDEAD_BEEF_CAFE_F00D);
    }
    serial_println!("[ok] paging: {:#x} -> {:#x}, {:#x} -> {:#x}, read/write verified",
        A, pa.as_u64(), B, pb.as_u64());

    assert!(paging::unmap_page(A));
    assert!(paging::unmap_page(B));
    assert!(paging::translate(A).is_none());
    let pc = paging::map_new_page(A, rw).expect("remap A");
    assert!(pc == pb || pc == pa, "freed frame not reused");
    unsafe { assert_eq!((A as *const u64).read_volatile(), 0, "frame not zeroed") };
    assert!(paging::unmap_page(A));
    serial_println!("[ok] unmap + frame reuse + zeroing verified ({} frames in use, {} for page tables)",
        paging::frames_in_use(), paging::frames_in_use() - before);
}

fn heap_selftest() {
    use alloc::{boxed::Box, string::String, vec::Vec};

    let boxed = Box::new(0xC0FFEEu32);
    let mut v: Vec<u64> = Vec::new();
    for i in 0..1000 {
        v.push(i * i);
    }
    let sum: u64 = v.iter().sum();
    let mut s = String::from("heap");
    s.push_str(" works");

    assert_eq!(*boxed, 0xC0FFEE);
    assert_eq!(sum, 332_833_500);
    serial_println!("[ok] heap selftest: Box, Vec(1000, sum={}), String(\"{}\")", sum, s);
}

fn pci_vfio_selftest() {
    let count = pci::init();
    assert!(count > 0, "PCI bus enumeration found no devices");
    assert!(pci::validate_bdf("0000:00:00.0").is_ok());
    assert!(pci::validate_bdf("bad-bdf").is_err());
    assert!(pci::is_gpu_class("0x030000") && pci::is_gpu_class("0x030200"));
    assert!(!pci::is_gpu_class("0x020000"));

    let devs = pci::devices();
    if let Some(gpu) = devs.iter().find(|d| d.is_gpu()) {
        let orig_driver = gpu.driver;
        {
            let guard = pci::prepare_gpu_for_passthrough(&gpu.bdf).expect("bind GPU to vfio-pci");
            assert_eq!(guard.bdf(), gpu.bdf);
            let bound = pci::devices().into_iter().find(|d| d.bdf == gpu.bdf).unwrap();
            assert_eq!(bound.driver, "vfio-pci");
        }
        let restored = pci::devices().into_iter().find(|d| d.bdf == gpu.bdf).unwrap();
        assert_eq!(restored.driver, orig_driver, "PciBindGuard must restore host driver on drop");
    }
    serial_println!(
        "[ok] PCI + BitVisor/OpenShell VFIO: {} PCI devices discovered, BDF/GPU/RAII guard verified",
        count
    );
}

fn storage_spool_selftest() {
    let total_sectors = 1_048_576u32; // 512 MiB
    let mbr = storage::build_default_mbr(total_sectors);
    let parts = storage::discover_mbr_partitions(&mbr, total_sectors as u64)
        .expect("discover MBR partitions");
    assert_eq!(parts.len(), 3);
    assert!(parts[0].bootable);
    assert_eq!(parts[0].partition_type, 0x83);
    assert_eq!(parts[1].partition_type, 0x0C);
    assert_eq!(parts[2].partition_type, 0x83);

    let access = storage::StorageAccess {
        lba: 2048,
        count: 1,
        sector_size: storage::SECTOR_SIZE,
        rw: storage::StorageRw::Write,
    };
    let job1 = storage::submit_spool_job("vda", access, alloc::vec![0xAA; 64]).expect("spool job 1");
    let job2 = storage::submit_spool_job("vda", access, alloc::vec![0xBB; 64]).expect("spool job 2");
    storage::cancel_spool_job(job2).expect("cancel job 2");
    let done = storage::process_next_spool_job().expect("process job 1");
    assert_eq!(done.id, job1);
    assert_eq!(done.state, storage::SpoolJobState::Completed);
    assert!(storage::process_next_spool_job().is_none());
    serial_println!(
        "[ok] storage + MBR + I/O spooler: partition #{} ({}) verified, FIFO spool & cancel ok",
        parts[0].index,
        parts[0].type_name()
    );
}

fn sandbox_contract_selftest() {
    assert!(sandbox::ResolvedWorkloadIdentity::new(0, 1000, alloc::vec![], "policy", "sha256:abc").is_err());
    let id = sandbox::ResolvedWorkloadIdentity::new(
        1000,
        1000,
        alloc::vec![1000, 27, 44, 27],
        "policy",
        "sha256:deadbeef",
    )
    .expect("valid non-root workload identity");
    assert_eq!(id.supplementary_gids, alloc::vec![27, 44]);

    let ev = sandbox::ProvisioningEvidence::from_enforcement_evidence("gen-1", b"kvm+tpm2+vfio")
        .expect("provisioning evidence");
    assert!(ev.validate("gen-1").is_ok());
    assert!(ev.validate("gen-2").is_err());

    assert!(sandbox::LoopbackTarget::new([127, 0, 0, 1], 5700).is_ok());
    assert!(sandbox::LoopbackTarget::new([192, 168, 1, 1], 5700).is_err());
    serial_println!(
        "[ok] OpenShell/OpenSandbox contract: non-root UID={}, generation digest & loopback guard verified",
        id.uid
    );
}

fn cxl_boundary_selftest() {
    let init_stats = paging::init_cxl_pools(8);
    assert_eq!(init_stats.total_pooled_frames, 16);

    // 1. On-demand CXL memory provisioning & cross-host reallocation
    let blk = paging::allocate_pooled_block(1, 10).expect("allocate pooled block across CXL nodes");
    assert_eq!(blk.frames.len(), 10);
    let migrated = paging::reallocate_pooled_block_to(blk.block_id, 2).expect("reallocate pooled block");
    assert_eq!(migrated.host_id, 2);
    let mid_stats = paging::pool_stats();
    assert!(mid_stats.rebalance_migrations >= 2);
    paging::release_pooled_block(blk.block_id).expect("release pooled block");
    assert_eq!(paging::pool_stats().allocated_pooled_frames, 0);

    // 2. Hardware-enforced spatial, temporal, and modularity boundaries
    assert_eq!(
        paging::create_bounded_region(1, true, true, 100, 0x10).unwrap_err(),
        paging::BoundaryError::PermissionViolation {
            write: true,
            execute: true
        }
    );

    let reg = paging::create_bounded_region(2, true, false, 50, 0x42).expect("create bounded region");
    assert!(paging::translate(reg.lower_guard_va).is_none(), "lower guard must be unmapped");
    assert!(paging::translate(reg.upper_guard_va).is_none(), "upper guard must be unmapped");
    assert!(paging::translate(reg.base_va).is_some(), "region base must be mapped");

    let now = reg.lease_start_tick;
    assert!(paging::validate_boundary_access(reg.id, reg.base_va, 64, true, false, 0x42, now).is_ok());
    // Modularity violation (unauthorized caller module)
    assert_eq!(
        paging::validate_boundary_access(reg.id, reg.base_va, 64, false, false, 0x99, now),
        Err(paging::BoundaryError::ModularityViolation {
            caller_module: 0x99,
            owner_module: 0x42
        })
    );
    // Spatial violation (overflow past upper guard)
    assert!(matches!(
        paging::validate_boundary_access(reg.id, reg.end_va - 8, 32, false, false, 0x42, now),
        Err(paging::BoundaryError::SpatialOutOfBounds { .. })
    ));
    // Temporal violation (expired lease)
    assert!(matches!(
        paging::validate_boundary_access(
            reg.id,
            reg.base_va,
            64,
            false,
            false,
            0x42,
            reg.lease_expiry_tick + 1
        ),
        Err(paging::BoundaryError::TemporalLeaseExpired { .. })
    ));

    let revoked = paging::revoke_expired_boundaries(reg.lease_expiry_tick + 1);
    assert_eq!(revoked, 1);
    assert!(paging::translate(reg.base_va).is_none(), "revoked region must be unmapped");

    serial_println!(
        "[ok] CXL pooling & HW boundaries: {} pooled frames, cross-host rebalance & guard/lease verified",
        init_stats.total_pooled_frames
    );
}

fn pipeline_sdf_selftest() {
    use pipeline::ServiceCapability as Cap;
    pipeline::init();

    // 1. Verify Ontological SDF capability resolution
    let crypto = pipeline::sdf_resolve(Cap::Decrypt).expect("resolve Decrypt");
    let dpi = pipeline::sdf_resolve(Cap::DeepPacketInspection).expect("resolve DPI");
    let analytics = pipeline::sdf_resolve(Cap::Analytics).expect("resolve Analytics");
    assert!(crypto.conn.zero_copy_capable && dpi.conn.zero_copy_capable && analytics.conn.zero_copy_capable);

    // 2. Allocate shared-memory zero-copy payload slot and encrypt in-place via Universal Microservice API
    let original = b"POSTCARD-AUTOBUHN-PACKET:TELEMETRY=OK";
    let slot = pipeline::alloc_zero_copy_bytes(original).expect("alloc zero-copy slot");
    let enc_desc = pipeline::Service::encrypt(slot.slot_id).expect("Service.Encrypt");
    assert_ne!(enc_desc.flags & pipeline::DESC_FLAG_ENCRYPTED, 0);
    let cipher_bytes = pipeline::read_slot_bytes(slot.slot_id).expect("read encrypted slot");
    assert_ne!(cipher_bytes.as_slice(), original);

    // 3. Direct-Flight Chaining: Decrypt -> DeepPacketInspection -> Analytics with 0 CPU staging copies
    let chain = [Cap::Decrypt, Cap::DeepPacketInspection, Cap::Analytics];
    let report = pipeline::execute_direct_flight(slot.slot_id, &chain).expect("execute direct-flight chain");
    assert_eq!(report.cpu_staging_copies, 0, "must perform zero CPU staging copies");
    assert_eq!(report.stages_executed, 3);
    assert_eq!(report.ring_handoffs, 3);
    assert_ne!(report.flags & pipeline::DESC_FLAG_DECRYPTED, 0);
    assert_ne!(report.flags & pipeline::DESC_FLAG_DPI_CLEAN, 0);
    assert_ne!(report.flags & pipeline::DESC_FLAG_ANALYTICS_DONE, 0);
    assert_ne!(report.telemetry_digest, 0);

    let clear_bytes = pipeline::read_slot_bytes(slot.slot_id).expect("read decrypted slot");
    assert_eq!(clear_bytes.as_slice(), original, "in-place decrypt must restore exact payload");
    pipeline::free_zero_copy_slot(slot.slot_id).expect("free zero-copy slot");

    serial_println!(
        "[ok] Phase 1 SDF & Zero-Copy Pipeline: {} hops ({} -> {} -> {}), 0 CPU copies, digest={:#x}",
        report.stages_executed,
        crypto.name,
        dpi.name,
        analytics.name,
        report.telemetry_digest
    );
}

fn tiered_storage_selftest() {
    // 1. Explicit FTL Confirm Marks: 3-block stream only writes confirm mark on final block
    for (offset, is_last) in [(0u64, false), (1, false), (2, true)] {
        let blk = storage::tiered_write_block(
            5000 + offset,
            b"stream-block-payload",
            storage::FtlAttr {
                tx_id: 77,
                require_confirm_mark: is_last,
            },
        );
        if is_last {
            assert!(blk.nvm_confirmed);
        }
    }

    // 2. Hybrid Block Temperature Tiering & Smart Recopy Omission (dirty_fg == false)
    let (_, b1) = storage::tiered_read_block(5000).expect("read 1");
    assert_eq!(b1.tier, storage::StorageTier::HighCapacityFlash);
    let (_, b2) = storage::tiered_read_block(5000).expect("read 2 (promotes to Hot DDR-SSD Cache)");
    assert_eq!(b2.tier, storage::StorageTier::DdrSsdCache);
    assert_eq!(b2.temperature, storage::BlockTemperature::Hot);
    assert!(!b2.dirty_fg, "read-promoted cache block must have dirty_fg == false");

    let recopy_omitted = storage::demote_block_to_cold(5000).expect("demote clean cached block");
    assert!(recopy_omitted, "Smart Recopy Omission must skip recopy when dirty_fg is false");

    // 3. Vervain's MLC-to-SLC Flash Remapping on ECC error / wear
    let remapped = storage::report_flash_ecc_error_lba(5001).expect("report ECC error on MLC block");
    assert_eq!(remapped.cell_kind, storage::FlashCellKind::Slc, "worn/error MLC block must remap to SLC");

    // 4. Dual-VFS Metadata Separation, Concurrent Writable Snapshots, and Non-WORM Lifecycle
    let f = storage::create_vfs_file(
        "/cards/telemetry.log",
        b"immutable-stream-v1-payload-across-multiple-blocks-for-confirm-mark-test",
        "tier=hot,index=v1",
        0,
    );
    assert!(!f.reindex_flag);
    let (updated_meta, data_digest_after) =
        storage::update_vfs_metadata("/cards/telemetry.log", 0, "tier=hot,index=v2")
            .expect("update Dual-VFS metadata");
    assert!(updated_meta.reindex_flag, "metadata edit must set reindex_flag");
    assert_ne!(data_digest_after, 0);

    // Point-in-time CoW snapshot -> promote to concurrent writable snapshot -> mutate branch
    let snap = storage::create_snapshot("snap-gold-1");
    assert!(storage::modify_vfs_file("/cards/telemetry.log", snap.snap_id, b"mutated").is_err());
    storage::promote_snapshot_writable(snap.snap_id).expect("promote snapshot writable");
    storage::modify_vfs_file("/cards/telemetry.log", snap.snap_id, b"branch-v2-payload")
        .expect("modify file in writable snapshot");

    let root_bytes = storage::read_vfs_file("/cards/telemetry.log", 0).expect("read root file");
    let branch_bytes =
        storage::read_vfs_file("/cards/telemetry.log", snap.snap_id).expect("read snapshot file");
    assert_ne!(root_bytes, branch_bytes, "CoW writable snapshot must not overwrite root view");

    // Non-WORM retention pruning & deletion
    storage::create_vfs_file("/tmp/ephemeral.bin", b"temp", "ephemeral", 10);
    let pruned = storage::prune_expired_vfs_files(20);
    assert_eq!(pruned, 1, "expired file must be pruned without WORM lockout");
    storage::delete_vfs_file("/cards/telemetry.log", snap.snap_id).expect("non-WORM delete");

    let st = storage::tiered_engine_stats();
    assert!(st.recopies_omitted >= 1 && st.mlc_to_slc_remaps >= 1 && st.confirm_marks_skipped >= 2);
    serial_println!(
        "[ok] Phase 2 Non-WORM Tiered Storage: recopies_omitted={}, MLC->SLC={}, confirm_skipped={}, snaps={}",
        st.recopies_omitted,
        st.mlc_to_slc_remaps,
        st.confirm_marks_skipped,
        st.snapshots
    );
}

fn hal_ime_selftest() {
    // 1. Prototype HAL: normalized 9-DoF IMU over I2C and SPI buses
    let imu_i2c = hal_ime::hal_read_imu_9dof(hal_ime::HalBus::I2c {
        bus_id: 0,
        addr: 0x68,
    });
    assert_eq!(imu_i2c.heading_deg, 42);
    assert!(imu_i2c.vibration_magnitude_mg >= 980);

    // 2. Spatial Vision (MIPI CSI-2 ToF depth grid) -> Zero-Copy IPC Ring Buffer
    let mut tof_ring = pipeline::ZeroCopyRingBuffer::new(1);
    hal_ime::hal_capture_tof_depth_zero_copy(
        hal_ime::HalBus::MipiCsi2 {
            lane_count: 4,
            virtual_channel: 3,
        },
        &mut tof_ring,
    )
    .expect("capture MIPI CSI-2 ToF frame into zero-copy ring");
    let dequeued = tof_ring.pop().expect("dequeue ToF frame descriptor");
    assert_eq!(dequeued.payload_len, 1024);
    assert_eq!(dequeued.source_id, 3);

    // 3. Power & Thermal Nervous System (PMIC)
    let pmic_nom = hal_ime::hal_read_pmic_thermal(hal_ime::HalBus::SmbusPmic { rail_id: 0 });
    assert_eq!(pmic_nom.vcore_mv, 900);
    assert_eq!(pmic_nom.thermal_gradient_mc, 3500);

    // 4. Out-of-Band IME Inference Loop + Preemptive Defenses + Local Attestation
    hal_ime::ime_step_oob_inference(imu_i2c, pmic_nom);

    let mut shock_imu = imu_i2c;
    shock_imu.vibration_magnitude_mg = 4000;
    hal_ime::ime_step_oob_inference(shock_imu, pmic_nom);

    let mut hot_pmic = pmic_nom;
    hot_pmic.storage_array_temp_mc = 82_000;
    hot_pmic.thermal_gradient_mc = 48_000;
    hal_ime::ime_step_oob_inference(imu_i2c, hot_pmic);

    let records = hal_ime::ime_attestations();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].anomaly, hal_ime::ImeAnomalyClass::Nominal);
    assert_eq!(
        records[1].anomaly,
        hal_ime::ImeAnomalyClass::ViolentDropShock
    );
    assert_eq!(
        records[2].anomaly,
        hal_ime::ImeAnomalyClass::LocalizedThermalHotspot
    );

    // Verify local-first cryptographic attestation digests (zero cloud spyware)
    for rec in &records {
        assert!(rec.verify("ime-local-epoch-v1"), "IME local attestation verification failed");
        assert!(!rec.verify("tampered-epoch"));
    }

    // 5. Expanded Physical Senses: Global Shutter HDR, ALS & Barometric Altimeter
    let gs_hdr = hal_ime::hal_capture_global_shutter_hdr(0, 64, 64, 800, 120, b"GLOBAL-SHUTTER-HDR-FRAME")
        .expect("capture Global Shutter HDR frame");
    assert_eq!(gs_hdr.rolling_shutter_skew_ns, 0, "global shutter must have 0ns skew");
    assert_eq!(gs_hdr.cpu_staging_copies, 0);
    let _ = pipeline::ring_dequeue(1);
    pipeline::free_zero_copy_slot(gs_hdr.zero_copy_slot_id).expect("free HDR slot");

    let env = hal_ime::hal_read_environmental(800, 100_125);
    assert_eq!(env.display_brightness_pct, 80);
    assert!(env.altitude_cm > 0);

    // 6. Bleeding-Edge Silicon: Neuromorphic Event Camera (DVS), QRNG & CXL Fabric + Agentic Mesh
    let spikes = [
        hal_ime::NeuromorphicSpikeEvent {
            x: 12,
            y: 34,
            polarity: 1,
            delta_log_lux: 9,
            timestamp_us: 105,
        },
        hal_ime::NeuromorphicSpikeEvent {
            x: 88,
            y: 19,
            polarity: -1,
            delta_log_lux: 7,
            timestamp_us: 108,
        },
    ];
    let dvs = hal_ime::hal_stream_neuromorphic_events(0, 65_536, &spikes)
        .expect("stream neuromorphic events");
    assert_eq!(dvs.spike_count, 2);
    assert_eq!(dvs.static_pixels_suppressed, 65_534);
    assert_eq!(dvs.cpu_staging_copies, 0);
    let _ = pipeline::ring_dequeue(2);
    pipeline::free_zero_copy_slot(dvs.zero_copy_slot_id).expect("free DVS slot");

    let qrng = hal_ime::hal_qrng_sample(1);
    assert!(qrng.health_test_passed && qrng.min_entropy_bits_per_byte == 8);

    let mesh = hal_ime::mesh_publish_spatial_map(
        7,
        hal_ime::SpatialMeshPeerKind::WearableHeadset,
        b"SPATIAL-MESH-LIDAR-OCTREE-V1",
    )
    .expect("publish spatial mesh map");
    assert_eq!(mesh.loopback_ipv4, [127, 0, 0, 1]);
    assert_eq!(mesh.cpu_staging_copies, 0);
    pipeline::free_zero_copy_slot(mesh.zero_copy_slot_id).expect("free mesh slot");

    let heal = hal_ime::ime_run_agentic_self_healing(18_000, 240).expect("agentic self-healing");
    assert!(heal.attestation.verify("ime-local-epoch-v1"));
    assert!(heal.containers_migrated >= 2 && heal.background_tasks_shed >= 1);

    let st = hal_ime::stats();
    assert!(st.defensive_interventions >= 2 && st.rail_throttled);
    serial_println!(
        "[ok] Prototype HAL & OOB IME: 9DoF/ToF/HDR/DVS(0-copy)/QRNG/Mesh ok, {} defenses, {} attestations",
        st.defensive_interventions,
        st.attestations_signed
    );
}

fn framebuffer_selftest() {
    let mut fb = framebuffer::init_default_surface();
    let framebuffer_addr = fb.base_address as usize;
    let stride = fb.stride;
    let (x, y) = (4usize, 7usize);

    // Direct pointer write into video memory:
    // let fb_ptr = framebuffer_addr as *mut u32;
    // *fb_ptr.add(y * stride + x) = 0x00FF0000; // Pure Red
    unsafe {
        let fb_ptr = framebuffer_addr as *mut u32;
        *fb_ptr.add(y * stride + x) = framebuffer::COLOR_PURE_RED;
        assert_eq!(fb.read_pixel(x, y), Some(0x00FF0000));

        fb.draw_spatial_card(8, 8, 12, 10, framebuffer::COLOR_CYAN_GLOW, framebuffer::COLOR_VOID_BG);
        assert_eq!(fb.read_pixel(8, 8), Some(framebuffer::COLOR_CYAN_GLOW));
        assert_eq!(fb.read_pixel(10, 10), Some(framebuffer::COLOR_VOID_BG));
    }

    let comp = framebuffer::compose_spatial_frame_zero_copy().expect("compose spatial frame");
    assert_eq!(comp.cpu_staging_copies, 0);
    assert_eq!(comp.cards_rendered, 1);
    assert_eq!(comp.rogue_cards_quarantined, 1);
    assert_eq!(comp.anchor_pixel_rgb, 0x00FF0000);

    serial_println!(
        "[ok] linear framebuffer & spatial compositor: base={:#x}, cards={} (rogue quarantined={}), 0-copy pixel=0x00FF0000",
        framebuffer_addr,
        comp.cards_rendered,
        comp.rogue_cards_quarantined
    );
}

fn spatial_wm_selftest() {
    spatial_wm::init_spatial_workspace();
    assert_eq!(spatial_wm::focused_card_id(), Some(1));

    // 1. Real ray/plane hit testing: the viewport-centre ray lands on card 1.
    let hit = spatial_wm::hit_test_detailed(spatial_wm::PointerEvent {
        x: 500,
        y: 400,
        pressed: false,
        dragged: false,
    })
    .expect("centre ray hits card 1");
    assert_eq!(hit.card_id, 1);
    assert!(hit.distance > 0.0);
    assert!(hit.u > 0.0 && hit.u < 1.0 && hit.v > 0.0 && hit.v < 1.0);

    // 2. A ray through empty space hits nothing.
    assert_eq!(
        spatial_wm::hit_test_spatial_cards(spatial_wm::PointerEvent {
            x: 60,
            y: 700,
            pressed: false,
            dragged: false,
        }),
        None
    );

    // 3. Card 2 sits further down the view axis and is individually hittable.
    let second = spatial_wm::hit_test_detailed(spatial_wm::PointerEvent {
        x: 880,
        y: 350,
        pressed: false,
        dragged: false,
    })
    .expect("right ray hits card 2");
    assert_eq!(second.card_id, 2);
    assert!(second.distance > hit.distance);

    // 4. Press, then drag: the card moves by the real ray/plane displacement.
    let before = spatial_wm::with_world(|world| world.card(1).expect("card 1").pose.position);
    spatial_wm::handle_spatial_input(spatial_wm::PointerEvent {
        x: 500,
        y: 400,
        pressed: true,
        dragged: false,
    });
    assert_eq!(spatial_wm::focused_card_id(), Some(1));
    spatial_wm::handle_spatial_input(spatial_wm::PointerEvent {
        x: 560,
        y: 430,
        pressed: false,
        dragged: true,
    });
    let after = spatial_wm::with_world(|world| world.card(1).expect("card 1").pose.position);
    let delta = after - before;
    assert!(delta.x > 50.0, "drag x follows the pointer: {}", delta.x);
    assert!(delta.y < -20.0, "drag y follows the pointer: {}", delta.y);
    assert!(delta.z.abs() < 0.01, "drag stays in the card plane: {}", delta.z);

    // 5. Focus is a stack; raising a background card routes input to it.
    assert_eq!(spatial_wm::with_world(|world| world.sys_focus_card(2)), 0);
    assert_eq!(spatial_wm::focused_card_id(), Some(2));

    // 6. The hypervisor syscall surface drives the same world model.
    let created = spatial_wm::with_world(|world| {
        world.sys_create_card(spatial_wm::CardDesc {
            width: 100.0,
            height: 100.0,
            pose: spatial_wm::PoseDesc {
                position: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            },
        })
    });
    assert!(created > 0, "sys_create_card returned {}", created);
    let created = created as u32;
    let syscall_hit = spatial_wm::with_world(|world| {
        world.sys_hit_test(spatial_wm::RayDesc {
            origin: [0.0, 0.0, -80.0],
            direction: [0.0, 0.0, 1.0],
        })
    })
    .expect("syscall ray hits the created card");
    assert_eq!(syscall_hit.card_id, created);
    assert_eq!(
        spatial_wm::with_world(|world| world.sys_destroy_card(created as u64)),
        0
    );

    // 7. Rotation is part of the model: a card yawed 90° is edge-on to a
    //    head-on ray and therefore cannot be hit.
    let edge_on = spatial_wm::SpatialCard::new(
        9,
        spatial_wm::Pose::uniform(
            spatial_wm::Vec3::new(0.0, 0.0, 0.0),
            spatial_wm::Quat::from_axis_angle(
                spatial_wm::Vec3::UNIT_Y,
                core::f32::consts::FRAC_PI_2,
            ),
            1.0,
        ),
        200.0,
        100.0,
    );
    let head_on = spatial_wm::Ray::new(
        spatial_wm::Vec3::new(0.0, 0.0, -100.0),
        spatial_wm::Vec3::UNIT_Z,
    )
    .expect("ray");
    assert!(spatial_wm::intersect_card(&head_on, &edge_on).is_none());

    let cards = spatial_wm::active_cards();
    let painter_order = spatial_wm::paint_order();
    serial_println!(
        "[ok] spatial hypervisor: {} cards, ray hit #{} at distance {:.1}, drag delta=({:.1}, {:.1}, {:.1}), z-order={} cards, focus=#{:?}, revision={}",
        cards.len(),
        hit.card_id,
        hit.distance,
        delta.x,
        delta.y,
        delta.z,
        painter_order.len(),
        spatial_wm::focused_card_id(),
        spatial_wm::world_version()
    );
}

fn neural_copilot_selftest() {
    let rep = coprocessor::run_neural_supervisory_copilot()
        .expect("run on-chip neural supervisory co-pilot");
    assert_eq!(rep.coprocessor_id, 0x524F_4D59);
    assert_eq!(rep.perception.tof_zero_copy_bytes, 1024);
    assert_eq!(rep.perception.neuromorphic_spikes, 2);
    assert!(rep.rogue_guard_violation_blocked);
    assert!(rep.attestation_verified);
    assert!(rep.active_spatial_cards >= 2);

    serial_println!(
        "[ok] AI-native neural core: vCPU={} multi-modal(ToF={}B, DVS={}) sdf_reroute=#{} guard_blocked={} att_ok={}",
        rep.assigned_v_cpu,
        rep.perception.tof_zero_copy_bytes,
        rep.perception.neuromorphic_spikes,
        rep.rerouted_service_id,
        rep.rogue_guard_violation_blocked,
        rep.attestation_verified
    );
}

fn patent_suite_selftest() {
    use alloc::string::ToString;

    // 1. Risk-Based Augmented Secrets & Transitivity Modulation
    let leaf_secret = sandbox::AugmentedSecret {
        secret_id: 2,
        label: "db-service-token".to_string(),
        asset_class: sandbox::AssetClass::Secret,
        scope: sandbox::ScopeMetadata {
            dependent_vms: 8,
            dependent_hosts: 4,
            cloud_regions: 2,
        },
        impact: sandbox::ImpactMetadata {
            criticality_score: 50,
            unlocks_financial_or_root: false,
        },
        exposure: sandbox::ExposureMetadata {
            transmitted_in_cleartext: false,
            unguarded_volatile_ram: false,
            copied_to_removable_media: false,
            honeypot_canary: false,
        },
        protection: sandbox::ProtectionMetadata {
            hardware_hsm_or_tpm_backed: true,
            air_gapped: false,
            strict_acl_enforced: true,
            hw_boundary_guarded: true,
        },
        rotation: sandbox::RotationAuditMetadata {
            created_tick: 0,
            last_rotated_tick: 10,
            expiration_tick: 5000,
            audit_access_count: 0,
        },
        unlocks_secret_ids: alloc::vec![],
    };

    let mut master_vault_key = sandbox::AugmentedSecret {
        secret_id: 1,
        label: "root-vault-master-key".to_string(),
        asset_class: sandbox::AssetClass::Secret,
        scope: sandbox::ScopeMetadata {
            dependent_vms: 12,
            dependent_hosts: 6,
            cloud_regions: 3,
        },
        impact: sandbox::ImpactMetadata {
            criticality_score: 70,
            unlocks_financial_or_root: true,
        },
        exposure: sandbox::ExposureMetadata {
            transmitted_in_cleartext: false,
            unguarded_volatile_ram: false,
            copied_to_removable_media: false,
            honeypot_canary: false,
        },
        protection: sandbox::ProtectionMetadata {
            hardware_hsm_or_tpm_backed: true,
            air_gapped: true,
            strict_acl_enforced: true,
            hw_boundary_guarded: true,
        },
        rotation: sandbox::RotationAuditMetadata {
            created_tick: 0,
            last_rotated_tick: 10,
            expiration_tick: 5000,
            audit_access_count: 0,
        },
        unlocks_secret_ids: alloc::vec![2], // Transitive risk from leaf_secret (#2)
    };

    let vault = [leaf_secret];
    let decision =
        sandbox::evaluate_secret_access_with_transitivity(&mut master_vault_key, &vault, 100);
    assert!(matches!(
        decision,
        sandbox::AccessModulationDecision::RequireStepUpAuth { .. }
    ));

    // 2. Credit-Card-Sized Modular Computer (CN108628404A): Brain + Inert Shell Dock
    let mut brain = sandbox::PostCardBrainModule::new_credit_card_brain(0xCAFE_2026, 512);
    assert_eq!(brain.dimensions_mm, (85, 54, 2));
    let dock = sandbox::InertShellDock {
        shell_id: 7,
        display_width: 1920,
        display_height: 1080,
        has_keyboard: true,
        loopback_bus_only: true,
    };
    assert_eq!(brain.dock_into_shell(&dock), Ok(7));

    // 3. IBM VM Disk Descriptor Merging & Hot Volume Swap (CN103853595B)
    let mut vm = storage::VmInstanceConfig {
        vm_uuid: "uuid-postcard-vm-01".to_string(),
        card_name: "debian-card-1".to_string(),
        mac_addr: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        vcpu_count: 4,
        memory_mib: 2048,
        root_disk: storage::VmDiskDescriptor {
            volume_id: "vol-old-01".to_string(),
            base_golden_image: "debian-golden.qcow2".to_string(),
            overlay_path: "overlays/card-01-v1.qcow2".to_string(),
            sectors: 1_048_576,
            descriptor_generation: 1,
            merged_digest: 0,
        },
        disk_swap_count: 0,
    };
    let merged = storage::replace_vm_disk_preserving_uuid(
        &mut vm,
        "vol-new-02",
        "overlays/card-01-v2.qcow2",
        2_097_152,
    )
    .expect("hot-swap VM disk preserving UUID");
    assert_eq!(vm.vm_uuid, "uuid-postcard-vm-01");
    assert_eq!(merged.descriptor_generation, 2);
    assert_eq!(vm.disk_swap_count, 1);

    // 4. Adaptive Spatial Variability & Closed-Loop DoE (TWI867989B)
    let doe = coprocessor::optimize_spatial_variability_doe(&[120, 185, 140, 210]);
    assert!(doe.drift_corrected);
    assert!(doe.optimized_variability_ppm < doe.initial_variability_ppm);

    // 5. DPLL + DDS + EW-RLS Packet-Network Dual Time/Frequency Synchronization
    let sync_packets = [
        hal_ime::SyncPacketSample {
            seq_id: 1,
            master_tx_ns: 1_000_000_000,
            slave_rx_ns: 1_000_000_180,
            pdv_jitter_ns: 45,
            dropped: false,
        },
        hal_ime::SyncPacketSample {
            seq_id: 2,
            master_tx_ns: 2_000_000_000,
            slave_rx_ns: 2_000_000_260,
            pdv_jitter_ns: -30,
            dropped: false,
        },
        hal_ime::SyncPacketSample {
            seq_id: 3,
            master_tx_ns: 3_000_000_000,
            slave_rx_ns: 0,
            pdv_jitter_ns: 0,
            dropped: true, // Simulated packet loss handled via EW-RLS holdover
        },
        hal_ime::SyncPacketSample {
            seq_id: 4,
            master_tx_ns: 4_000_000_000,
            slave_rx_ns: 4_000_000_420,
            pdv_jitter_ns: 15,
            dropped: false,
        },
        hal_ime::SyncPacketSample {
            seq_id: 5,
            master_tx_ns: 5_000_000_000,
            slave_rx_ns: 5_000_000_500,
            pdv_jitter_ns: -10,
            dropped: false,
        },
    ];
    let dpll = hal_ime::sync_packet_network_dpll_rls(&sync_packets);
    assert!(dpll.dds_overflow_pulses > 0);
    assert_eq!(dpll.packets_dropped, 1);
    assert!(dpll.locked_nanosecond_grade);

    serial_println!(
        "[ok] Patent Suite (Augmented Secrets/CN103853595B/CN108628404A/TWI867989B/DPLL-DDS-RLS): mod={:?} dock=#{} vm_gen={} doe={}->{}ppm dpll_err={}ns",
        decision,
        brain.docked_shell_id.unwrap_or(0),
        merged.descriptor_generation,
        doe.initial_variability_ppm,
        doe.optimized_variability_ppm,
        dpll.residual_time_error_ns
    );
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("KERNEL PANIC: {}", info);
    loop {}
}
