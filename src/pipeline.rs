//! Phase 1 & Subsystem 14: The Blazing-Fast Pipelined Core & Zero-Copy IPC Ring Buffers
//!
//! Implements:
//! 1. **Ontological Service Discovery Function (SDF)**:
//!    Dynamic registration and capability resolution for hardware and software accelerators
//!    (FPGAs, CXL resource pools, SIMD math units, security coprocessors, local routines).
//! 2. **Zero-Copy Shared-Memory IPC Ring Buffer & Multi-Processor Framework**
//!    (US12568064B2, EP4281865B1, US12591538B1, US12367167B2):
//!    Pre-allocated physical frame payload slots and direct descriptor handoffs across
//!    synchronized transport ring buffers with zero CPU staging copies.
//! 3. **Universal Microservices API & Drivers (`Service::encrypt`, `Service::decrypt`,
//!    `Service::dpi`, `Service::analytics`)**:
//!    Language-agnostic function calls that resolve and dispatch to the optimal registered
//!    accelerator without exposing hardware location or driver details.
//! 4. **Direct-Flight Message Forwarding & Chaining**:
//!    Sequential accelerator pipelines (`Decrypt -> DeepPacketInspection -> Analytics`)
//!    streaming descriptors from one accelerator ring to the next in-place (`cpu_copies == 0`).

use crate::paging::KernelFrames;
use crate::sync::Mutex;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use x86_64::structures::paging::FrameAllocator;

pub const ARENA_SLOTS: usize = 16;
pub const SLOT_CAPACITY: usize = 256;
pub const RING_CAPACITY: usize = 16;

/// Standardized ontological capabilities published into the Service Discovery Function (SDF).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceCapability {
    Encrypt = 1,
    Decrypt = 2,
    DeepPacketInspection = 3,
    Analytics = 4,
    Compression = 5,
    IntegrityHash = 6,
}

impl ServiceCapability {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Encrypt => "Service.Encrypt",
            Self::Decrypt => "Service.Decrypt",
            Self::DeepPacketInspection => "Service.DPI",
            Self::Analytics => "Service.Analytics",
            Self::Compression => "Service.Compress",
            Self::IntegrityHash => "Service.Hash",
        }
    }
}

/// Hardware or software execution substrate class in the ontological catalog.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceleratorKind {
    FpgaCryptoEngine = 1,
    SmartNicDpiTile = 2,
    CxlAnalyticsPool = 3,
    SimdMathRoutine = 4,
}

impl AcceleratorKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::FpgaCryptoEngine => "FPGA-Crypto",
            Self::SmartNicDpiTile => "SmartNIC-DPI",
            Self::CxlAnalyticsPool => "CXL-Analytics",
            Self::SimdMathRoutine => "SIMD-Local",
        }
    }
}

/// Connection attributes describing how the direct-flight switch reaches an accelerator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionAttributes {
    pub numa_or_cxl_node: u8,
    pub ring_channel_id: u8,
    pub zero_copy_capable: bool,
    pub latency_ns: u32,
    pub bandwidth_gbps: u16,
}

/// Registered node entry inside the Ontological Service Discovery Function (SDF) catalog.
#[derive(Debug, Clone)]
pub struct OntologicalServiceEntry {
    pub service_id: u32,
    pub name: String,
    pub kind: AcceleratorKind,
    pub capabilities: Vec<ServiceCapability>,
    pub conn: ConnectionAttributes,
    pub invocations: u64,
    pub online: bool,
}

/// Packet inspection and transformation flags carried in a zero-copy descriptor.
pub const DESC_FLAG_ENCRYPTED: u16 = 1 << 0;
pub const DESC_FLAG_DECRYPTED: u16 = 1 << 1;
pub const DESC_FLAG_DPI_CLEAN: u16 = 1 << 2;
pub const DESC_FLAG_DPI_THREAT: u16 = 1 << 3;
pub const DESC_FLAG_ANALYTICS_DONE: u16 = 1 << 4;

/// 32-byte zero-copy transport layer descriptor passed through IPC ring buffers.
/// The payload itself stays in its physical shared-memory slot (`phys_addr`) and is
/// never copied between intermediate staging buffers.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadDescriptor {
    pub slot_id: u16,
    pub len: u16,
    pub flags: u16,
    pub hop_count: u8,
    pub origin_cpu: u8,
    pub source_id: u32,
    pub payload_len: usize,
    pub phys_addr: u64,
    pub sequence_no: u64,
    pub telemetry_digest: u64,
}

/// Coherent multi-processor zero-copy ring buffer for direct pointer/descriptor handoffs.
#[derive(Debug, Clone)]
pub struct ZeroCopyRingBuffer {
    pub channel_id: u8,
    pub entries: [Option<PayloadDescriptor>; RING_CAPACITY],
    pub head: usize,
    pub tail: usize,
    pub len: usize,
    pub sequence_counter: u64,
    pub handoffs: u64,
}

impl ZeroCopyRingBuffer {
    pub const fn new(channel_id: u8) -> Self {
        Self {
            channel_id,
            entries: [None; RING_CAPACITY],
            head: 0,
            tail: 0,
            len: 0,
            sequence_counter: 0,
            handoffs: 0,
        }
    }

    pub fn push(&mut self, mut desc: PayloadDescriptor) -> Result<u64, PipelineError> {
        if self.len >= RING_CAPACITY {
            return Err(PipelineError::RingFull);
        }
        self.sequence_counter += 1;
        desc.sequence_no = self.sequence_counter;
        self.entries[self.tail] = Some(desc);
        self.tail = (self.tail + 1) % RING_CAPACITY;
        self.len += 1;
        self.handoffs += 1;
        Ok(desc.sequence_no)
    }

    pub fn pop(&mut self) -> Option<PayloadDescriptor> {
        if self.len == 0 {
            return None;
        }
        let desc = self.entries[self.head].take();
        self.head = (self.head + 1) % RING_CAPACITY;
        self.len -= 1;
        desc
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineError {
    ArenaExhausted,
    PayloadTooLarge,
    InvalidSlot,
    RingFull,
    RingEmpty,
    ServiceNotFound(ServiceCapability),
}

/// Summary telemetry returned after a direct-flight pipeline execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectFlightReport {
    pub slot_id: u16,
    pub phys_addr: u64,
    pub payload_len: usize,
    pub stages_executed: u8,
    pub ring_handoffs: u64,
    pub cpu_staging_copies: usize,
    pub flags: u16,
    pub telemetry_digest: u64,
}

struct PipelineSubsystem {
    initialized: bool,
    arena_frame_phys: u64,
    slot_in_use: [bool; ARENA_SLOTS],
    descriptors: [PayloadDescriptor; ARENA_SLOTS],
    catalog: Vec<OntologicalServiceEntry>,
    next_service_id: u32,
    rings: [ZeroCopyRingBuffer; 4],
    total_packets_chained: u64,
    total_ring_handoffs: u64,
}

const EMPTY_DESC: PayloadDescriptor = PayloadDescriptor {
    slot_id: 0,
    len: 0,
    flags: 0,
    hop_count: 0,
    origin_cpu: 0,
    source_id: 0,
    payload_len: 0,
    phys_addr: 0,
    sequence_no: 0,
    telemetry_digest: 0,
};

static PIPELINE: Mutex<PipelineSubsystem> = Mutex::new(PipelineSubsystem {
    initialized: false,
    arena_frame_phys: 0,
    slot_in_use: [false; ARENA_SLOTS],
    descriptors: [EMPTY_DESC; ARENA_SLOTS],
    catalog: Vec::new(),
    next_service_id: 1,
    rings: [
        ZeroCopyRingBuffer::new(0),
        ZeroCopyRingBuffer::new(1),
        ZeroCopyRingBuffer::new(2),
        ZeroCopyRingBuffer::new(3),
    ],
    total_packets_chained: 0,
    total_ring_handoffs: 0,
});

/// Initializes the shared-memory zero-copy arena (backed by a physical 4 KiB frame)
/// and populates default hardware/software accelerators in the Ontological SDF catalog.
pub fn init() {
    let mut sub = PIPELINE.lock();
    if sub.initialized {
        return;
    }
    let frame = KernelFrames
        .allocate_frame()
        .expect("allocate zero-copy pipeline arena frame");
    sub.arena_frame_phys = frame.start_address().as_u64();
    for i in 0..ARENA_SLOTS {
        let slot_phys = sub.arena_frame_phys + (i * SLOT_CAPACITY) as u64;
        sub.descriptors[i] = PayloadDescriptor {
            slot_id: i as u16,
            len: 0,
            flags: 0,
            hop_count: 0,
            origin_cpu: 0,
            source_id: 0,
            payload_len: 0,
            phys_addr: slot_phys,
            sequence_no: 0,
            telemetry_digest: 0,
        };
    }

    // Register standard accelerators into the Ontological SDF catalog
    register_locked(
        &mut sub,
        "fpga-aes-xts-0",
        AcceleratorKind::FpgaCryptoEngine,
        alloc::vec![ServiceCapability::Encrypt, ServiceCapability::Decrypt],
        ConnectionAttributes {
            numa_or_cxl_node: 0,
            ring_channel_id: 0,
            zero_copy_capable: true,
            latency_ns: 45,
            bandwidth_gbps: 100,
        },
    );
    register_locked(
        &mut sub,
        "smartnic-dpi-0",
        AcceleratorKind::SmartNicDpiTile,
        alloc::vec![ServiceCapability::DeepPacketInspection],
        ConnectionAttributes {
            numa_or_cxl_node: 0,
            ring_channel_id: 1,
            zero_copy_capable: true,
            latency_ns: 60,
            bandwidth_gbps: 100,
        },
    );
    register_locked(
        &mut sub,
        "cxl-telemetry-pool-0",
        AcceleratorKind::CxlAnalyticsPool,
        alloc::vec![ServiceCapability::Analytics, ServiceCapability::IntegrityHash],
        ConnectionAttributes {
            numa_or_cxl_node: 1,
            ring_channel_id: 2,
            zero_copy_capable: true,
            latency_ns: 80,
            bandwidth_gbps: 64,
        },
    );
    register_locked(
        &mut sub,
        "simd-local-fallback",
        AcceleratorKind::SimdMathRoutine,
        alloc::vec![
            ServiceCapability::Encrypt,
            ServiceCapability::Decrypt,
            ServiceCapability::Analytics,
            ServiceCapability::Compression,
        ],
        ConnectionAttributes {
            numa_or_cxl_node: 0,
            ring_channel_id: 3,
            zero_copy_capable: true,
            latency_ns: 180,
            bandwidth_gbps: 32,
        },
    );

    sub.initialized = true;
}

fn register_locked(
    sub: &mut PipelineSubsystem,
    name: &str,
    kind: AcceleratorKind,
    capabilities: Vec<ServiceCapability>,
    conn: ConnectionAttributes,
) -> u32 {
    let id = sub.next_service_id;
    sub.next_service_id += 1;
    sub.catalog.push(OntologicalServiceEntry {
        service_id: id,
        name: name.to_string(),
        kind,
        capabilities,
        conn,
        invocations: 0,
        online: true,
    });
    id
}

/// Dynamically registers an accelerator and its compute capabilities into the Ontological SDF.
pub fn sdf_register(
    name: &str,
    kind: AcceleratorKind,
    capabilities: Vec<ServiceCapability>,
    conn: ConnectionAttributes,
) -> u32 {
    init();
    let mut sub = PIPELINE.lock();
    register_locked(&mut sub, name, kind, capabilities, conn)
}

/// Ontological SDF query: discovers the lowest-latency online accelerator that supports
/// `capability` with zero-copy ring buffer connectivity.
pub fn sdf_resolve(capability: ServiceCapability) -> Result<OntologicalServiceEntry, PipelineError> {
    init();
    let sub = PIPELINE.lock();
    sub.catalog
        .iter()
        .filter(|e| e.online && e.capabilities.contains(&capability))
        .min_by_key(|e| e.conn.latency_ns)
        .cloned()
        .ok_or(PipelineError::ServiceNotFound(capability))
}

/// Snapshot of all accelerators registered in the Ontological SDF catalog.
pub fn sdf_catalog() -> Vec<OntologicalServiceEntry> {
    init();
    PIPELINE.lock().catalog.clone()
}

/// Allocates a zero-copy staging slot directly inside `ring` (`cpu_staging_copies == 0`)
/// and returns a mutable reference to its `PayloadDescriptor`.
pub fn alloc_zero_copy_slot(
    ring: &mut ZeroCopyRingBuffer,
) -> Result<&mut PayloadDescriptor, &'static str> {
    if ring.len >= RING_CAPACITY {
        return Err("Zero-copy ring buffer is full");
    }
    ring.sequence_counter += 1;
    let idx = ring.tail;
    ring.tail = (ring.tail + 1) % RING_CAPACITY;
    ring.len += 1;
    ring.handoffs += 1;
    ring.entries[idx] = Some(PayloadDescriptor {
        slot_id: idx as u16,
        len: 0,
        flags: 0,
        hop_count: 0,
        origin_cpu: 0,
        source_id: 0,
        payload_len: 0,
        phys_addr: 0,
        sequence_no: ring.sequence_counter,
        telemetry_digest: 0,
    });
    ring.entries[idx].as_mut().ok_or("Failed to allocate ring slot")
}

/// Allocates a zero-copy slot in the shared physical arena and writes `initial_data` once.
/// All subsequent microservice calls and pipeline hops operate on this slot in-place.
pub fn alloc_zero_copy_bytes(initial_data: &[u8]) -> Result<PayloadDescriptor, PipelineError> {
    init();
    if initial_data.len() > SLOT_CAPACITY {
        return Err(PipelineError::PayloadTooLarge);
    }
    let mut sub = PIPELINE.lock();
    let slot_idx = sub
        .slot_in_use
        .iter()
        .position(|&used| !used)
        .ok_or(PipelineError::ArenaExhausted)?;
    sub.slot_in_use[slot_idx] = true;
    let phys = sub.arena_frame_phys + (slot_idx * SLOT_CAPACITY) as u64;
    unsafe {
        let dst = phys as *mut u8;
        core::ptr::write_bytes(dst, 0, SLOT_CAPACITY);
        core::ptr::copy_nonoverlapping(initial_data.as_ptr(), dst, initial_data.len());
    }
    let desc = PayloadDescriptor {
        slot_id: slot_idx as u16,
        len: initial_data.len() as u16,
        flags: 0,
        hop_count: 0,
        origin_cpu: 0,
        source_id: 0,
        payload_len: initial_data.len(),
        phys_addr: phys,
        sequence_no: 0,
        telemetry_digest: 0,
    };
    sub.descriptors[slot_idx] = desc;
    Ok(desc)
}

/// Frees a zero-copy arena slot so it can be reused.
pub fn free_zero_copy_slot(slot_id: u16) -> Result<(), PipelineError> {
    let idx = slot_id as usize;
    if idx >= ARENA_SLOTS {
        return Err(PipelineError::InvalidSlot);
    }
    let mut sub = PIPELINE.lock();
    if !sub.slot_in_use[idx] {
        return Err(PipelineError::InvalidSlot);
    }
    let phys = sub.descriptors[idx].phys_addr;
    unsafe {
        core::ptr::write_bytes(phys as *mut u8, 0, SLOT_CAPACITY);
    }
    sub.slot_in_use[idx] = false;
    sub.descriptors[idx].len = 0;
    sub.descriptors[idx].flags = 0;
    sub.descriptors[idx].hop_count = 0;
    sub.descriptors[idx].telemetry_digest = 0;
    Ok(())
}

/// Reads the current in-place payload bytes from a zero-copy slot (for verification/display).
pub fn read_slot_bytes(slot_id: u16) -> Result<Vec<u8>, PipelineError> {
    let idx = slot_id as usize;
    if idx >= ARENA_SLOTS {
        return Err(PipelineError::InvalidSlot);
    }
    let sub = PIPELINE.lock();
    if !sub.slot_in_use[idx] {
        return Err(PipelineError::InvalidSlot);
    }
    let desc = sub.descriptors[idx];
    let slice = unsafe { core::slice::from_raw_parts(desc.phys_addr as *const u8, desc.len as usize) };
    Ok(slice.to_vec())
}

/// Returns the current descriptor metadata for `slot_id`.
pub fn slot_descriptor(slot_id: u16) -> Result<PayloadDescriptor, PipelineError> {
    let idx = slot_id as usize;
    if idx >= ARENA_SLOTS {
        return Err(PipelineError::InvalidSlot);
    }
    let sub = PIPELINE.lock();
    if !sub.slot_in_use[idx] {
        return Err(PipelineError::InvalidSlot);
    }
    Ok(sub.descriptors[idx])
}

/// Pushes a descriptor onto a zero-copy IPC ring buffer (`channel_id` 0..4) for cross-core or
/// accelerator handoff without copying the underlying payload.
pub fn ring_enqueue(channel_id: u8, desc: PayloadDescriptor) -> Result<u64, PipelineError> {
    init();
    let mut sub = PIPELINE.lock();
    let ch = (channel_id as usize) % sub.rings.len();
    let seq = sub.rings[ch].push(desc)?;
    sub.total_ring_handoffs += 1;
    Ok(seq)
}

/// Pops the next descriptor from a zero-copy IPC ring buffer (`channel_id` 0..4).
pub fn ring_dequeue(channel_id: u8) -> Result<PayloadDescriptor, PipelineError> {
    let mut sub = PIPELINE.lock();
    let ch = (channel_id as usize) % sub.rings.len();
    sub.rings[ch].pop().ok_or(PipelineError::RingEmpty)
}

const STREAM_KEY: [u8; 8] = [0xA5, 0x5A, 0xF0, 0x0F, 0x3C, 0xC3, 0x69, 0x96];

/// Executes a single capability in-place on `desc` using the resolved ontological accelerator.
fn execute_stage_locked(
    sub: &mut PipelineSubsystem,
    cap: ServiceCapability,
    mut desc: PayloadDescriptor,
) -> Result<PayloadDescriptor, PipelineError> {
    // 1. Resolve optimal accelerator in Ontological SDF
    let best_idx = sub
        .catalog
        .iter()
        .enumerate()
        .filter(|(_, e)| e.online && e.capabilities.contains(&cap))
        .min_by_key(|(_, e)| e.conn.latency_ns)
        .map(|(i, _)| i)
        .ok_or(PipelineError::ServiceNotFound(cap))?;

    let ring_ch = (sub.catalog[best_idx].conn.ring_channel_id as usize) % sub.rings.len();
    sub.catalog[best_idx].invocations += 1;

    // 2. Direct-flight descriptor handoff through the accelerator's shared ring buffer
    sub.rings[ring_ch].push(desc)?;
    sub.total_ring_handoffs += 1;
    desc = sub.rings[ring_ch].pop().ok_or(PipelineError::RingEmpty)?;

    // 3. Operate directly on the physical shared-memory slot (zero staging copies)
    let buf = unsafe { core::slice::from_raw_parts_mut(desc.phys_addr as *mut u8, desc.len as usize) };
    match cap {
        ServiceCapability::Encrypt => {
            for (i, b) in buf.iter_mut().enumerate() {
                *b ^= STREAM_KEY[i % STREAM_KEY.len()] ^ ((i as u8).wrapping_mul(17));
            }
            desc.flags |= DESC_FLAG_ENCRYPTED;
            desc.flags &= !DESC_FLAG_DECRYPTED;
        }
        ServiceCapability::Decrypt => {
            for (i, b) in buf.iter_mut().enumerate() {
                *b ^= STREAM_KEY[i % STREAM_KEY.len()] ^ ((i as u8).wrapping_mul(17));
            }
            desc.flags |= DESC_FLAG_DECRYPTED;
            desc.flags &= !DESC_FLAG_ENCRYPTED;
        }
        ServiceCapability::DeepPacketInspection => {
            // Scan payload in-place for malicious signature pattern b"MALWARE"
            let has_threat = buf.windows(7).any(|w| w == b"MALWARE");
            if has_threat {
                desc.flags |= DESC_FLAG_DPI_THREAT;
                desc.flags &= !DESC_FLAG_DPI_CLEAN;
            } else {
                desc.flags |= DESC_FLAG_DPI_CLEAN;
                desc.flags &= !DESC_FLAG_DPI_THREAT;
            }
        }
        ServiceCapability::Analytics | ServiceCapability::IntegrityHash => {
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for &b in buf.iter() {
                hash ^= b as u64;
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
            desc.telemetry_digest = hash;
            desc.flags |= DESC_FLAG_ANALYTICS_DONE;
        }
        ServiceCapability::Compression => {
            // Tag-only non-destructive pass so payload remains reversible in place
            desc.telemetry_digest = desc.len as u64;
        }
    }

    desc.hop_count = desc.hop_count.saturating_add(1);
    sub.descriptors[desc.slot_id as usize] = desc;
    Ok(desc)
}

/// Universal Microservices API: clean, location-transparent function calls.
pub struct Service;

impl Service {
    pub fn invoke(cap: ServiceCapability, slot_id: u16) -> Result<PayloadDescriptor, PipelineError> {
        init();
        let idx = slot_id as usize;
        if idx >= ARENA_SLOTS {
            return Err(PipelineError::InvalidSlot);
        }
        let mut sub = PIPELINE.lock();
        if !sub.slot_in_use[idx] {
            return Err(PipelineError::InvalidSlot);
        }
        let desc = sub.descriptors[idx];
        execute_stage_locked(&mut sub, cap, desc)
    }

    pub fn encrypt(slot_id: u16) -> Result<PayloadDescriptor, PipelineError> {
        Self::invoke(ServiceCapability::Encrypt, slot_id)
    }

    pub fn decrypt(slot_id: u16) -> Result<PayloadDescriptor, PipelineError> {
        Self::invoke(ServiceCapability::Decrypt, slot_id)
    }

    pub fn dpi(slot_id: u16) -> Result<PayloadDescriptor, PipelineError> {
        Self::invoke(ServiceCapability::DeepPacketInspection, slot_id)
    }

    pub fn analytics(slot_id: u16) -> Result<PayloadDescriptor, PipelineError> {
        Self::invoke(ServiceCapability::Analytics, slot_id)
    }
}

/// Direct-Flight Message Forwarding & Chaining: streams a packet descriptor sequentially
/// through `stages` (e.g., `[Decrypt, DeepPacketInspection, Analytics]`) across accelerator
/// ring buffers without a single CPU staging copy (`cpu_staging_copies == 0`).
pub fn execute_direct_flight(
    slot_id: u16,
    stages: &[ServiceCapability],
) -> Result<DirectFlightReport, PipelineError> {
    init();
    let idx = slot_id as usize;
    if idx >= ARENA_SLOTS {
        return Err(PipelineError::InvalidSlot);
    }
    let mut sub = PIPELINE.lock();
    if !sub.slot_in_use[idx] {
        return Err(PipelineError::InvalidSlot);
    }
    let handoffs_before = sub.total_ring_handoffs;
    let mut desc = sub.descriptors[idx];
    for &cap in stages {
        desc = execute_stage_locked(&mut sub, cap, desc)?;
    }
    sub.total_packets_chained += 1;
    let ring_handoffs = sub.total_ring_handoffs - handoffs_before;
    Ok(DirectFlightReport {
        slot_id: desc.slot_id,
        phys_addr: desc.phys_addr,
        payload_len: desc.len as usize,
        stages_executed: stages.len() as u8,
        ring_handoffs,
        cpu_staging_copies: 0,
        flags: desc.flags,
        telemetry_digest: desc.telemetry_digest,
    })
}

/// Returns `(registered_services, total_packets_chained, total_ring_handoffs)`.
pub fn pipeline_stats() -> (usize, u64, u64) {
    init();
    let sub = PIPELINE.lock();
    (
        sub.catalog.len(),
        sub.total_packets_chained,
        sub.total_ring_handoffs,
    )
}
