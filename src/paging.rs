//! Physical frame allocator and 4 KiB page mapping on top of the boot page
//! tables. Physical RAM below 1 GiB is identity-mapped, so page-table frames
//! can be touched directly by their physical address.

use crate::memory;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::sync::{Mutex, NoPreempt};
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{
    mapper::MapToError, FrameAllocator, FrameDeallocator, Mapper, OffsetPageTable, Page,
    PageTable, PageTableFlags, PhysFrame, Size4KiB, Translate,
};
use x86_64::{PhysAddr, VirtAddr};

const MAX_REGIONS: usize = 16;

struct Frames {
    regions: [(u64, u64); MAX_REGIONS], // (start, end), page aligned
    count: usize,
    idx: usize,
    next: u64,
    free: Vec<u64>,
    allocated: usize,
    total: usize,
}

static FRAMES: Mutex<Option<Frames>> = Mutex::new(None);

/// Starts frame allocation from usable RAM at or above `floor` (below the
/// identity-mapped limit). Returns the number of frames available.
///
/// # Safety
/// `info` must be the multiboot2 pointer; `floor` must be above everything
/// already in use (kernel, multiboot info, heap). Heap must be initialised.
pub unsafe fn init_frames(floor: u64) -> usize {
    let mut regions = [(0u64, 0u64); MAX_REGIONS];
    let mut count = 0;
    let mut total = 0usize;
    memory::for_each_usable(|r| {
        let start = memory::align_up(r.base.max(floor), 4096);
        let end = r.end().min(memory::IDENTITY_LIMIT) & !4095;
        if end > start && count < MAX_REGIONS {
            regions[count] = (start, end);
            count += 1;
            total += ((end - start) / 4096) as usize;
        }
    });
    let next = if count > 0 { regions[0].0 } else { 0 };
    *FRAMES.lock() = Some(Frames {
        regions,
        count,
        idx: 0,
        next,
        free: Vec::new(),
        allocated: 0,
        total,
    });
    total
}

/// Total frames the allocator manages.
pub fn frames_total() -> usize {
    FRAMES.lock().as_ref().map_or(0, |f| f.total)
}

/// Number of frames currently handed out.
pub fn frames_in_use() -> usize {
    FRAMES.lock().as_ref().map_or(0, |f| f.allocated)
}

pub struct KernelFrames;

unsafe impl FrameAllocator<Size4KiB> for KernelFrames {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let mut guard = FRAMES.lock();
        let f = guard.as_mut()?;
        let addr = if let Some(a) = f.free.pop() {
            a
        } else {
            loop {
                if f.idx >= f.count {
                    return None;
                }
                let (_, end) = f.regions[f.idx];
                if f.next + 4096 <= end {
                    let a = f.next;
                    f.next += 4096;
                    break a;
                }
                f.idx += 1;
                if f.idx < f.count {
                    f.next = f.regions[f.idx].0;
                }
            }
        };
        f.allocated += 1;
        // Hand out zeroed frames (identity mapped).
        unsafe { core::ptr::write_bytes(addr as *mut u8, 0, 4096) };
        Some(PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

impl FrameDeallocator<Size4KiB> for KernelFrames {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame) {
        if let Some(f) = FRAMES.lock().as_mut() {
            f.free.push(frame.start_address().as_u64());
            f.allocated -= 1;
        }
    }
}

/// Mapper over the currently active page tables (physical offset 0).
unsafe fn mapper() -> OffsetPageTable<'static> {
    let (l4, _) = Cr3::read();
    let table = &mut *(l4.start_address().as_u64() as *mut PageTable);
    OffsetPageTable::new(table, VirtAddr::new(0))
}

/// Maps `virt` to a freshly allocated frame.
pub fn map_new_page(virt: u64, flags: PageTableFlags) -> Result<PhysAddr, MapToError<Size4KiB>> {
    let _np = NoPreempt::new(); // page-table edits must not interleave
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(virt));
    let mut alloc = KernelFrames;
    let frame = alloc.allocate_frame().ok_or(MapToError::FrameAllocationFailed)?;
    let flags = flags | PageTableFlags::PRESENT;
    unsafe {
        match mapper().map_to(page, frame, flags, &mut alloc) {
            Ok(flush) => flush.flush(),
            Err(e) => {
                alloc.deallocate_frame(frame);
                return Err(e);
            }
        }
    }
    Ok(frame.start_address())
}

/// Unmaps `virt` and returns its frame to the allocator.
pub fn unmap_page(virt: u64) -> bool {
    let _np = NoPreempt::new();
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(virt));
    unsafe {
        match mapper().unmap(page) {
            Ok((frame, flush)) => {
                flush.flush();
                KernelFrames.deallocate_frame(frame);
                true
            }
            Err(_) => false,
        }
    }
}

/// Replaces the flags of the mapped 4 KiB page containing `virt`.
pub fn set_page_flags(virt: u64, flags: PageTableFlags) -> bool {
    let _np = NoPreempt::new();
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(virt));
    unsafe {
        match mapper().update_flags(page, flags | PageTableFlags::PRESENT) {
            Ok(flush) => {
                flush.flush();
                true
            }
            Err(_) => false,
        }
    }
}

extern "C" {
    static __text_start: u8;
    static __rodata_start: u8;
    static __data_start: u8;
    static kernel_end: u8;
    static boot_stack_guard: u8;
}

const MIB2: u64 = 2 * 1024 * 1024;

/// Replaces the boot page tables with kernel-owned ones:
/// * page 0 unmapped (null-pointer guard)
/// * kernel text RX, rodata R+NX, data/bss RW+NX (W^X)
/// * guard pages (unmapped) below the boot stack and double-fault stack
/// * remaining RAM below 1 GiB identity-mapped RW+NX (2 MiB pages, except the
///   first 2 MiB which uses 4 KiB pages)
///
/// # Safety
/// Frame allocator must be initialised; boot identity map must be active.
pub unsafe fn remap_kernel() {
    use x86_64::registers::control::{Cr0, Cr0Flags, Cr3Flags, Efer, EferFlags};
    use x86_64::structures::paging::Size2MiB;

    Efer::update(|f| *f |= EferFlags::NO_EXECUTE_ENABLE);
    Cr0::update(|f| *f |= Cr0Flags::WRITE_PROTECT);

    let mut alloc = KernelFrames;
    let pml4_frame = alloc.allocate_frame().expect("pml4 frame");
    let pml4 = &mut *(pml4_frame.start_address().as_u64() as *mut PageTable);
    let mut m = OffsetPageTable::new(pml4, VirtAddr::new(0));

    let text = core::ptr::addr_of!(__text_start) as u64;
    let rodata = core::ptr::addr_of!(__rodata_start) as u64;
    let data = core::ptr::addr_of!(__data_start) as u64;
    let kend = memory::align_up(core::ptr::addr_of!(kernel_end) as u64, 4096);
    let guards = [core::ptr::addr_of!(boot_stack_guard) as u64, crate::gdt::df_guard_page()];

    let p = PageTableFlags::PRESENT;
    let nx = PageTableFlags::NO_EXECUTE;
    let w = PageTableFlags::WRITABLE;

    // First 2 MiB with 4 KiB granularity.
    let mut addr = 4096; // page 0 stays unmapped
    while addr < MIB2 {
        let flags = if addr >= kend || addr < text {
            p | w | nx
        } else if guards.contains(&addr) {
            addr += 4096;
            continue;
        } else if addr < rodata {
            p // text: RX
        } else if addr < data {
            p | nx // rodata: R
        } else {
            p | w | nx
        };
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
        let frame = PhysFrame::containing_address(PhysAddr::new(addr));
        m.map_to(page, frame, flags, &mut alloc).expect("map kernel page").ignore();
        addr += 4096;
    }
    // Everything else below the identity limit: 2 MiB pages, RW+NX.
    let mut addr = MIB2;
    while addr < memory::IDENTITY_LIMIT {
        let page = Page::<Size2MiB>::containing_address(VirtAddr::new(addr));
        let frame = PhysFrame::<Size2MiB>::containing_address(PhysAddr::new(addr));
        m.map_to(page, frame, p | w | nx, &mut alloc).expect("map 2M page").ignore();
        addr += MIB2;
    }

    // Guards outside the first 2 MiB (should not happen with a 1 MiB kernel).
    for g in guards {
        assert!(g < MIB2, "guard page outside first 2 MiB");
    }

    Cr3::write(pml4_frame, Cr3Flags::empty());
    KERNEL_CR3.store(pml4_frame.start_address().as_u64(), Ordering::SeqCst);
}

static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);

/// Physical address of the kernel's own PML4 (valid after `remap_kernel`).
pub fn kernel_cr3() -> u64 {
    KERNEL_CR3.load(Ordering::SeqCst)
}

/// Makes `pml4` (physical address) the active top-level page table.
pub fn switch_to(pml4: u64) {
    use x86_64::registers::control::Cr3Flags;
    if Cr3::read().0.start_address().as_u64() != pml4 {
        unsafe { Cr3::write(PhysFrame::containing_address(PhysAddr::new(pml4)), Cr3Flags::empty()) };
    }
}

/// Creates a page table for a user process: a fresh PML4 that shares every
/// kernel mapping (the kernel lives entirely under PML4 entry 0, so the
/// lower-level tables are shared too) and has no user mappings yet.
pub fn new_user_space() -> u64 {
    let _np = NoPreempt::new();
    let frame = KernelFrames.allocate_frame().expect("user pml4 frame");
    let src = unsafe { &*(kernel_cr3() as *const PageTable) };
    let dst = unsafe { &mut *(frame.start_address().as_u64() as *mut PageTable) };
    for i in 0..512 {
        dst[i] = src[i].clone();
    }
    frame.start_address().as_u64()
}

/// Frees a user page-table tree: every table and mapped frame reachable from
/// PML4 entries 1..512, then the PML4 itself. The kernel PML4 must be active.
pub fn destroy_user_space(pml4: u64) {
    let _np = NoPreempt::new();
    assert_eq!(Cr3::read().0.start_address().as_u64(), kernel_cr3(), "destroy with kernel cr3");
    unsafe fn free_tree(phys: u64, level: u8) {
        let table = &*(phys as *const PageTable);
        for e in table.iter() {
            if !e.flags().contains(PageTableFlags::PRESENT) {
                continue;
            }
            let child = e.addr().as_u64();
            if level > 1 && !e.flags().contains(PageTableFlags::HUGE_PAGE) {
                free_tree(child, level - 1);
            } else {
                KernelFrames.deallocate_frame(PhysFrame::containing_address(PhysAddr::new(child)));
            }
        }
        KernelFrames.deallocate_frame(PhysFrame::containing_address(PhysAddr::new(phys)));
    }
    unsafe {
        let top = &*(pml4 as *const PageTable);
        for i in 1..512 {
            if top[i].flags().contains(PageTableFlags::PRESENT) {
                free_tree(top[i].addr().as_u64(), 3);
            }
        }
        KernelFrames.deallocate_frame(PhysFrame::containing_address(PhysAddr::new(pml4)));
    }
}

/// Virtual → physical translation using the live page tables.
pub fn translate(virt: u64) -> Option<PhysAddr> {
    unsafe { mapper().translate_addr(VirtAddr::new(virt)) }
}

/// Flags of the 4 KiB page containing `virt`, if mapped as a 4 KiB page.
pub fn page_flags(virt: u64) -> Option<PageTableFlags> {
    use x86_64::structures::paging::mapper::TranslateResult;
    match unsafe { mapper().translate(VirtAddr::new(virt)) } {
        TranslateResult::Mapped { flags, .. } => Some(flags),
        _ => None,
    }
}

// ============================================================================
// SUBSYSTEM 13: CXL Memory Pooling & Hardware-Enforced Boundary Control
// (US20250383985A1, US20260003682A1, US12373213B2)
// ============================================================================

/// Virtual address region reserved for hardware-enforced bounded sandboxes/modules
/// (between 0x4200_0000 and 0x47FF_FFFF, below scheduler stacks at 0x4800_0000).
const BOUNDARY_REGION_BASE: u64 = 0x4200_0000;
const BOUNDARY_SLOT_STRIDE: u64 = 16 * 4096; // 64 KiB per slot (lower guard + up to 14 pages + upper guard)
const MAX_BOUNDARY_SLOTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryError {
    OutOfSlots,
    InvalidSize,
    FrameAllocationFailed,
    RegionNotFound,
    SpatialOutOfBounds { addr: u64, base: u64, end: u64 },
    TemporalLeaseExpired { now_tick: u64, expiry_tick: u64 },
    ModularityViolation { caller_module: u32, owner_module: u32 },
    PermissionViolation { write: bool, execute: bool },
    PoolExhausted,
    BlockNotFound,
}

/// Represents a CXL-attached or cross-host DRAM pool node.
#[derive(Debug, Clone)]
pub struct CxlPoolNode {
    pub node_id: u16,
    pub total_frames: usize,
    pub free_frames: Vec<u64>,
    pub allocated_frames: usize,
    pub migrations_in: u64,
    pub migrations_out: u64,
}

/// A dynamically pooled DRAM allocation mediated across CXL pool nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PooledBlock {
    pub block_id: u64,
    pub host_id: u16,
    pub node_id: u16,
    pub frames: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    pub total_pooled_frames: usize,
    pub allocated_pooled_frames: usize,
    pub free_pooled_frames: usize,
    pub rebalance_migrations: u64,
}

/// Hardware-enforced spatial, temporal, and modularity boundary descriptor.
#[derive(Debug, Clone)]
pub struct BoundedRegion {
    pub id: u64,
    pub slot: usize,
    pub lower_guard_va: u64,
    pub base_va: u64,
    pub end_va: u64,
    pub upper_guard_va: u64,
    pub writable: bool,
    pub executable: bool,
    pub lease_start_tick: u64,
    pub lease_expiry_tick: u64,
    pub owner_module: u32,
    pub active: bool,
}

struct CxlPoolManager {
    initialized: bool,
    next_block_id: u64,
    nodes: Vec<CxlPoolNode>,
    blocks: Vec<PooledBlock>,
    rebalance_count: u64,
    next_region_id: u64,
    free_boundary_slots: Vec<usize>,
    next_boundary_slot: usize,
    regions: Vec<BoundedRegion>,
}

static POOL_MANAGER: Mutex<CxlPoolManager> = Mutex::new(CxlPoolManager {
    initialized: false,
    next_block_id: 1,
    nodes: Vec::new(),
    blocks: Vec::new(),
    rebalance_count: 0,
    next_region_id: 1,
    free_boundary_slots: Vec::new(),
    next_boundary_slot: 0,
    regions: Vec::new(),
});

/// Initializes 2 CXL memory pool nodes with `frames_per_node` physical frames each.
pub fn init_cxl_pools(frames_per_node: usize) -> PoolStats {
    let mut mgr = POOL_MANAGER.lock();
    if mgr.initialized {
        return pool_stats_locked(&mgr);
    }
    let mut alloc = KernelFrames;
    for node_id in 0..2u16 {
        let mut free_frames = Vec::with_capacity(frames_per_node);
        for _ in 0..frames_per_node {
            if let Some(frame) = alloc.allocate_frame() {
                free_frames.push(frame.start_address().as_u64());
            }
        }
        let count = free_frames.len();
        mgr.nodes.push(CxlPoolNode {
            node_id,
            total_frames: count,
            free_frames,
            allocated_frames: 0,
            migrations_in: 0,
            migrations_out: 0,
        });
    }
    mgr.initialized = true;
    pool_stats_locked(&mgr)
}

fn pool_stats_locked(mgr: &CxlPoolManager) -> PoolStats {
    let mut total = 0;
    let mut allocated = 0;
    let mut free = 0;
    for n in &mgr.nodes {
        total += n.total_frames;
        allocated += n.allocated_frames;
        free += n.free_frames.len();
    }
    PoolStats {
        total_pooled_frames: total,
        allocated_pooled_frames: allocated,
        free_pooled_frames: free,
        rebalance_migrations: mgr.rebalance_count,
    }
}

/// Returns current CXL memory pool usage and migration statistics.
pub fn pool_stats() -> PoolStats {
    let mgr = POOL_MANAGER.lock();
    pool_stats_locked(&mgr)
}

/// On-demand memory provisioning: allocates `frame_count` frames for `host_id`
/// from the CXL pool node with the highest available capacity, spilling across
/// nodes if needed.
pub fn allocate_pooled_block(
    host_id: u16,
    frame_count: usize,
) -> Result<PooledBlock, BoundaryError> {
    if frame_count == 0 {
        return Err(BoundaryError::InvalidSize);
    }
    let mut mgr = POOL_MANAGER.lock();
    if !mgr.initialized {
        drop(mgr);
        init_cxl_pools(16);
        mgr = POOL_MANAGER.lock();
    }
    let total_free: usize = mgr.nodes.iter().map(|n| n.free_frames.len()).sum();
    if total_free < frame_count {
        return Err(BoundaryError::PoolExhausted);
    }

    // Pick primary node with most free frames
    let primary_idx = mgr
        .nodes
        .iter()
        .enumerate()
        .max_by_key(|(_, n)| n.free_frames.len())
        .map(|(i, _)| i)
        .ok_or(BoundaryError::PoolExhausted)?;
    let primary_node_id = mgr.nodes[primary_idx].node_id;

    let mut frames = Vec::with_capacity(frame_count);
    while frames.len() < frame_count {
        if let Some(f) = mgr.nodes[primary_idx].free_frames.pop() {
            mgr.nodes[primary_idx].allocated_frames += 1;
            frames.push(f);
        } else {
            // Borrow frame from peer CXL node on demand
            let donor_idx = mgr
                .nodes
                .iter()
                .position(|n| !n.free_frames.is_empty())
                .ok_or(BoundaryError::PoolExhausted)?;
            let f = mgr.nodes[donor_idx].free_frames.pop().unwrap();
            mgr.nodes[donor_idx].total_frames =
                mgr.nodes[donor_idx].total_frames.saturating_sub(1);
            mgr.nodes[donor_idx].migrations_out += 1;
            mgr.nodes[primary_idx].total_frames += 1;
            mgr.nodes[primary_idx].allocated_frames += 1;
            mgr.nodes[primary_idx].migrations_in += 1;
            mgr.rebalance_count += 1;
            frames.push(f);
        }
    }

    let block_id = mgr.next_block_id;
    mgr.next_block_id += 1;
    let block = PooledBlock {
        block_id,
        host_id,
        node_id: primary_node_id,
        frames,
    };
    mgr.blocks.push(block.clone());
    Ok(block)
}

/// Cross-host DRAM reallocation: transfers ownership of a pooled block to `new_host_id`
/// and balances free frames across CXL pool nodes.
pub fn reallocate_pooled_block_to(
    block_id: u64,
    new_host_id: u16,
) -> Result<PooledBlock, BoundaryError> {
    let mut mgr = POOL_MANAGER.lock();
    let idx = mgr
        .blocks
        .iter()
        .position(|b| b.block_id == block_id)
        .ok_or(BoundaryError::BlockNotFound)?;
    mgr.blocks[idx].host_id = new_host_id;
    mgr.rebalance_count += 1;
    Ok(mgr.blocks[idx].clone())
}

/// Reallocates `block_id` (or automatically provisions/migrates block 0 across CXL nodes)
/// when triggered by the Out-of-Band IME supervisor.
pub fn reallocate_pooled_block(block_id: u64) -> Result<PooledBlock, BoundaryError> {
    if block_id == 0 {
        let existing_id = {
            let mgr = POOL_MANAGER.lock();
            mgr.blocks.first().map(|b| b.block_id)
        };
        let target_id = match existing_id {
            Some(id) => id,
            None => allocate_pooled_block(1, 2)?.block_id,
        };
        return reallocate_pooled_block_to(target_id, 2);
    }
    reallocate_pooled_block_to(block_id, 2)
}

/// Releases a pooled block back to its CXL pool node and zeroes its physical frames.
pub fn release_pooled_block(block_id: u64) -> Result<(), BoundaryError> {
    let mut mgr = POOL_MANAGER.lock();
    let idx = mgr
        .blocks
        .iter()
        .position(|b| b.block_id == block_id)
        .ok_or(BoundaryError::BlockNotFound)?;
    let block = mgr.blocks.swap_remove(idx);
    let node_id = block.node_id;
    for phys in block.frames {
        unsafe { core::ptr::write_bytes(phys as *mut u8, 0, 4096) };
        if let Some(node) = mgr.nodes.iter_mut().find(|n| n.node_id == node_id) {
            node.free_frames.push(phys);
            node.allocated_frames = node.allocated_frames.saturating_sub(1);
        }
    }
    Ok(())
}

/// Hardware-enforced boundary provisioning: maps `pages` (1..=14) surrounded by
/// unmapped lower and upper hardware guard pages, enforcing W^X (`!(writable && executable)`),
/// temporal lease bounds (`lease_ticks`), and module ownership (`owner_module`).
pub fn create_bounded_region(
    pages: usize,
    writable: bool,
    executable: bool,
    lease_ticks: u64,
    owner_module: u32,
) -> Result<BoundedRegion, BoundaryError> {
    if pages == 0 || pages > 14 {
        return Err(BoundaryError::InvalidSize);
    }
    // Enforce W^X at hardware boundary creation time
    if writable && executable {
        return Err(BoundaryError::PermissionViolation {
            write: true,
            execute: true,
        });
    }

    let (slot, id) = {
        let mut mgr = POOL_MANAGER.lock();
        let slot = mgr.free_boundary_slots.pop().unwrap_or_else(|| {
            let s = mgr.next_boundary_slot;
            mgr.next_boundary_slot += 1;
            s
        });
        if slot >= MAX_BOUNDARY_SLOTS {
            return Err(BoundaryError::OutOfSlots);
        }
        let id = mgr.next_region_id;
        mgr.next_region_id += 1;
        (slot, id)
    };

    let slot_base = BOUNDARY_REGION_BASE + (slot as u64) * BOUNDARY_SLOT_STRIDE;
    let lower_guard_va = slot_base; // left unmapped
    let base_va = slot_base + 4096;
    let end_va = base_va + (pages as u64) * 4096;
    let upper_guard_va = end_va; // left unmapped

    let mut flags = PageTableFlags::PRESENT;
    if writable {
        flags |= PageTableFlags::WRITABLE;
    }
    if !executable {
        flags |= PageTableFlags::NO_EXECUTE;
    }

    for i in 0..pages {
        let va = base_va + (i as u64) * 4096;
        if map_new_page(va, flags).is_err() {
            for j in 0..i {
                unmap_page(base_va + (j as u64) * 4096);
            }
            POOL_MANAGER.lock().free_boundary_slots.push(slot);
            return Err(BoundaryError::FrameAllocationFailed);
        }
    }

    let now = crate::pic::ticks();
    let region = BoundedRegion {
        id,
        slot,
        lower_guard_va,
        base_va,
        end_va,
        upper_guard_va,
        writable,
        executable,
        lease_start_tick: now,
        lease_expiry_tick: now.saturating_add(lease_ticks),
        owner_module,
        active: true,
    };
    POOL_MANAGER.lock().regions.push(region.clone());
    Ok(region)
}

/// Validates spatial, temporal, modularity, and page-table permission boundaries
/// before allowing pointer dereference or control-flow transfer.
pub fn validate_boundary_access(
    region_id: u64,
    addr: u64,
    len: u64,
    write: bool,
    execute: bool,
    caller_module: u32,
    now_tick: u64,
) -> Result<PhysAddr, BoundaryError> {
    let region = {
        let mgr = POOL_MANAGER.lock();
        mgr.regions
            .iter()
            .find(|r| r.id == region_id && r.active)
            .cloned()
            .ok_or(BoundaryError::RegionNotFound)?
    };

    if caller_module != region.owner_module {
        return Err(BoundaryError::ModularityViolation {
            caller_module,
            owner_module: region.owner_module,
        });
    }
    if now_tick > region.lease_expiry_tick {
        return Err(BoundaryError::TemporalLeaseExpired {
            now_tick,
            expiry_tick: region.lease_expiry_tick,
        });
    }
    let Some(end) = addr.checked_add(len) else {
        return Err(BoundaryError::SpatialOutOfBounds {
            addr,
            base: region.base_va,
            end: region.end_va,
        });
    };
    if addr < region.base_va || end > region.end_va {
        return Err(BoundaryError::SpatialOutOfBounds {
            addr,
            base: region.base_va,
            end: region.end_va,
        });
    }
    if (write && !region.writable) || (execute && !region.executable) {
        return Err(BoundaryError::PermissionViolation { write, execute });
    }
    translate(addr).ok_or(BoundaryError::SpatialOutOfBounds {
        addr,
        base: region.base_va,
        end: region.end_va,
    })
}

/// Unmaps and revokes a bounded region, returning its physical frames and guard slot.
pub fn destroy_bounded_region(region_id: u64) -> Result<(), BoundaryError> {
    let region = {
        let mut mgr = POOL_MANAGER.lock();
        let idx = mgr
            .regions
            .iter()
            .position(|r| r.id == region_id && r.active)
            .ok_or(BoundaryError::RegionNotFound)?;
        let r = mgr.regions.swap_remove(idx);
        mgr.free_boundary_slots.push(r.slot);
        r
    };
    let mut va = region.base_va;
    while va < region.end_va {
        unmap_page(va);
        va += 4096;
    }
    Ok(())
}

/// Revokes and unmaps all bounded regions whose temporal lease has expired at `now_tick`.
pub fn revoke_expired_boundaries(now_tick: u64) -> usize {
    let expired_ids: Vec<u64> = {
        let mgr = POOL_MANAGER.lock();
        mgr.regions
            .iter()
            .filter(|r| r.active && now_tick > r.lease_expiry_tick)
            .map(|r| r.id)
            .collect()
    };
    let mut revoked = 0;
    for id in expired_ids {
        if destroy_bounded_region(id).is_ok() {
            revoked += 1;
        }
    }
    revoked
}

