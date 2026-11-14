//! Block storage interface, MBR partition table discovery, and asynchronous I/O job spooler.
//!
//! Ported from:
//! - `integrations/hypervisor/bitvisor-3.0/include/storage.h` (`StorageAccess`, LBA sector I/O)
//! - `integrations/hypervisor/cosmos-os` (MBR partition table discovery & validation)
//! - Post Card OS required subsystems (`README.md`: Partitioning, Buffering, Spooling)

use crate::sync::Mutex;
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const SECTOR_SIZE: usize = 512;
pub const LBA_INVALID: u64 = u64::MAX;
pub const MAX_SPOOL_JOBS: usize = 32;

/// BitVisor storage direction (`STORAGE_READ` / `STORAGE_WRITE` in `storage.h`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageRw {
    Read = 0,
    Write = 1,
}

/// BitVisor sector access descriptor (`struct storage_access` in `storage.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageAccess {
    pub lba: u64,
    pub count: u32,
    pub sector_size: usize,
    pub rw: StorageRw,
}

/// Standard MBR Partition Entry (16 bytes at offset `0x1BE + index * 16`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MbrPartition {
    pub index: u8,
    pub bootable: bool,
    pub partition_type: u8,
    pub start_lba: u32,
    pub sector_count: u32,
}

impl MbrPartition {
    pub fn size_bytes(&self) -> u64 {
        (self.sector_count as u64) * (SECTOR_SIZE as u64)
    }

    pub fn type_name(&self) -> &'static str {
        match self.partition_type {
            0x83 => "Linux ext4/xfs",
            0xEF => "EFI System",
            0x07 => "NTFS/exFAT",
            0x0B | 0x0C => "FAT32",
            0x82 => "Linux Swap",
            _ => "Raw/Custom",
        }
    }
}

/// Parse and validate an MBR sector (512 bytes) and discover non-empty partitions
/// within `total_sectors` boundaries.
pub fn discover_mbr_partitions(
    mbr_sector: &[u8; SECTOR_SIZE],
    total_sectors: u64,
) -> Result<Vec<MbrPartition>, &'static str> {
    if mbr_sector[510] != 0x55 || mbr_sector[511] != 0xAA {
        return Err("Invalid MBR boot signature (expected 0x55AA)");
    }
    let mut partitions = Vec::new();
    for idx in 0..4u8 {
        let offset = 0x1BE + (idx as usize) * 16;
        let status = mbr_sector[offset];
        let ptype = mbr_sector[offset + 4];
        let start_lba = u32::from_le_bytes([
            mbr_sector[offset + 8],
            mbr_sector[offset + 9],
            mbr_sector[offset + 10],
            mbr_sector[offset + 11],
        ]);
        let count = u32::from_le_bytes([
            mbr_sector[offset + 12],
            mbr_sector[offset + 13],
            mbr_sector[offset + 14],
            mbr_sector[offset + 15],
        ]);
        if ptype == 0 || count == 0 {
            continue;
        }
        let end_lba = (start_lba as u64) + (count as u64);
        if start_lba == 0 || end_lba > total_sectors {
            return Err("Partition exceeds disk LBA boundaries");
        }
        partitions.push(MbrPartition {
            index: idx + 1,
            bootable: status == 0x80,
            partition_type: ptype,
            start_lba,
            sector_count: count,
        });
    }
    Ok(partitions)
}

/// Synthesize a valid 3-partition MBR sector for Post Card OS:
/// - `P1` (`0x83`, bootable): Root OS / Kernel + Ramdisk partition
/// - `P2` (`0x0C`, FAT32): Immutable `CONFIG` partition (harvested from `#1`/`#2` layout)
/// - `P3` (`0x83`, ext4): Persistent Card Overlay & Spool partition
pub fn build_default_mbr(total_sectors: u32) -> [u8; SECTOR_SIZE] {
    let mut mbr = [0u8; SECTOR_SIZE];
    let p1_start: u32 = 2048;
    let config_sectors: u32 = 2048; // 1 MiB CONFIG partition
    let persist_sectors: u32 = 65536; // 32 MiB PERSIST/SPOOL partition
    let p1_count: u32 = total_sectors
        .saturating_sub(p1_start + config_sectors + persist_sectors);
    let p2_start: u32 = p1_start + p1_count;
    let p3_start: u32 = p2_start + config_sectors;

    let entries = [
        (0x80u8, 0x83u8, p1_start, p1_count),
        (0x00u8, 0x0Cu8, p2_start, config_sectors),
        (0x00u8, 0x83u8, p3_start, persist_sectors),
    ];

    for (i, (status, ptype, start, count)) in entries.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        let off = 0x1BE + i * 16;
        mbr[off] = *status;
        mbr[off + 4] = *ptype;
        mbr[off + 8..off + 12].copy_from_slice(&start.to_le_bytes());
        mbr[off + 12..off + 16].copy_from_slice(&count.to_le_bytes());
    }
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    mbr
}

/// Spooler job status (`README.md` Spooling requirement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpoolJobState {
    Queued,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone)]
pub struct SpoolJob {
    pub id: u64,
    pub device: String,
    pub access: StorageAccess,
    pub payload: Vec<u8>,
    pub state: SpoolJobState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpoolError {
    QueueFull,
    InvalidAccess,
    JobNotFound,
}

struct Spooler {
    next_id: u64,
    pending: VecDeque<SpoolJob>,
    completed: Vec<SpoolJob>,
}

static SPOOLER: Mutex<Spooler> = Mutex::new(Spooler {
    next_id: 1,
    pending: VecDeque::new(),
    completed: Vec::new(),
});

/// Submit a non-blocking I/O work item to the kernel spooler with backpressure check.
pub fn submit_spool_job(
    device: &str,
    access: StorageAccess,
    payload: Vec<u8>,
) -> Result<u64, SpoolError> {
    if access.lba == LBA_INVALID || access.count == 0 || access.sector_size == 0 {
        return Err(SpoolError::InvalidAccess);
    }
    let mut spool = SPOOLER.lock();
    if spool.pending.len() >= MAX_SPOOL_JOBS {
        return Err(SpoolError::QueueFull);
    }
    let id = spool.next_id;
    spool.next_id += 1;
    spool.pending.push_back(SpoolJob {
        id,
        device: device.to_string(),
        access,
        payload,
        state: SpoolJobState::Queued,
    });
    Ok(id)
}

/// Cancel a queued spool job before it executes.
pub fn cancel_spool_job(job_id: u64) -> Result<(), SpoolError> {
    let mut spool = SPOOLER.lock();
    let Some(pos) = spool.pending.iter().position(|j| j.id == job_id) else {
        return Err(SpoolError::JobNotFound);
    };
    let mut job = spool.pending.remove(pos).unwrap();
    job.state = SpoolJobState::Cancelled;
    spool.completed.push(job);
    Ok(())
}

/// Process the next queued I/O job in FIFO order.
pub fn process_next_spool_job() -> Option<SpoolJob> {
    let mut spool = SPOOLER.lock();
    let mut job = spool.pending.pop_front()?;
    job.state = SpoolJobState::Completed;
    let clone = job.clone();
    if spool.completed.len() >= 32 {
        spool.completed.remove(0);
    }
    spool.completed.push(job);
    Some(clone)
}

/// Return `(queued_count, completed_count)`.
pub fn spool_stats() -> (usize, usize) {
    let spool = SPOOLER.lock();
    (spool.pending.len(), spool.completed.len())
}

// ============================================================================
// PHASE 2: The Indestructible, Non-WORM Storage Engine (Tiering & Flash Health)
// ============================================================================

/// Access count threshold at which a logical block becomes Hot and promotes to DDR-SSD cache.
pub const HOT_THRESHOLD: u32 = 3;
/// Wear cycle threshold at which an economical MLC block is proactively remapped to SLC.
pub const MLC_WEAR_THRESHOLD: u32 = 8;

/// Storage media tier for Hybrid Block Temperature Tiering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageTier {
    /// High-speed DDR-backed SSD cache tier for Hot blocks.
    DdrSsdCache,
    /// High-capacity flash storage tier for Cold blocks.
    HighCapacityFlash,
}

/// Block temperature classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockTemperature {
    Cold,
    Hot,
}

/// Physical flash cell endurance class for Vervain MLC-to-SLC remapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashCellKind {
    /// Economical Multi-Level Cell flash (default allocation).
    Mlc,
    /// High-endurance Single-Level Cell flash (remapped target for worn/error blocks).
    Slc,
}

/// Explicit FTL transaction attribute parameters (`Attr`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FtlAttr {
    pub tx_id: u64,
    /// When true, writes a physical confirm mark to NVM (final block of a transaction).
    /// When false, skips individual commit overhead for streaming blocks.
    pub require_confirm_mark: bool,
}

/// Core mapping table entry for a logical block in the tiered storage engine.
#[derive(Debug, Clone)]
pub struct TieredBlockEntry {
    pub lba: u64,
    pub tier: StorageTier,
    pub temperature: BlockTemperature,
    /// Dirty/Update flag (`FG`) while cached in the fast DDR-SSD tier.
    /// If `false` upon demotion to Cold, Smart Recopy Omission skips rewriting to slow flash.
    pub dirty_fg: bool,
    pub cell_kind: FlashCellKind,
    pub phys_block_id: u64,
    pub read_count: u32,
    pub write_count: u32,
    pub wear_cycles: u32,
    pub ecc_errors: u32,
    pub integrity_digest: u64,
    pub last_tx_id: u64,
    pub nvm_confirmed: bool,
    pub fast_cache_data: Vec<u8>,
    pub slow_flash_data: Vec<u8>,
}

/// Immutable Data VFS Plane entry (content payload blocks & digest).
#[derive(Debug, Clone)]
pub struct DataVfsObject {
    pub data_id: u64,
    pub lba_list: Vec<u64>,
    pub byte_len: usize,
    pub content_digest: u64,
}

/// Editable Metadata & Control VFS Plane entry (IBM-style Dual-VFS).
#[derive(Debug, Clone)]
pub struct MetadataVfsEntry {
    pub file_id: u64,
    pub path: String,
    pub data_id: u64,
    pub attribute_tag: String,
    pub retention_expiry_tick: u64,
    /// Automatically set when metadata attributes change without touching the data VFS.
    pub reindex_flag: bool,
    pub snapshot_id: u64, // 0 = active root view
}

/// NetApp-style Copy-on-Write consistency point snapshot that can be promoted to writable.
#[derive(Debug, Clone)]
pub struct StorageSnapshot {
    pub snap_id: u64,
    pub name: String,
    pub writable: bool,
    pub created_tick: u64,
    pub file_count: usize,
}

/// Aggregate telemetry for the Phase 2 storage engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TieredEngineStats {
    pub total_blocks: usize,
    pub hot_blocks: usize,
    pub cold_blocks: usize,
    pub recopies_performed: u64,
    pub recopies_omitted: u64,
    pub mlc_to_slc_remaps: u64,
    pub confirm_marks_written: u64,
    pub confirm_marks_skipped: u64,
    pub active_files: usize,
    pub snapshots: usize,
}

struct TieredStorageEngine {
    blocks: Vec<TieredBlockEntry>,
    next_phys_mlc: u64,
    next_phys_slc: u64,
    recopies_performed: u64,
    recopies_omitted: u64,
    mlc_to_slc_remaps: u64,
    confirm_marks_written: u64,
    confirm_marks_skipped: u64,
    data_vfs: Vec<DataVfsObject>,
    meta_vfs: Vec<MetadataVfsEntry>,
    snapshots: Vec<StorageSnapshot>,
    next_data_id: u64,
    next_file_id: u64,
    next_snap_id: u64,
    next_auto_lba: u64,
}

static ENGINE: Mutex<TieredStorageEngine> = Mutex::new(TieredStorageEngine {
    blocks: Vec::new(),
    next_phys_mlc: 0x1000,
    next_phys_slc: 0x9000,
    recopies_performed: 0,
    recopies_omitted: 0,
    mlc_to_slc_remaps: 0,
    confirm_marks_written: 0,
    confirm_marks_skipped: 0,
    data_vfs: Vec::new(),
    meta_vfs: Vec::new(),
    snapshots: Vec::new(),
    next_data_id: 1,
    next_file_id: 1,
    next_snap_id: 1,
    next_auto_lba: 10_000,
});

fn fnv1a_64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn check_and_remap_mlc_to_slc(eng: &mut TieredStorageEngine, idx: usize) {
    let b = &eng.blocks[idx];
    if b.cell_kind == FlashCellKind::Mlc
        && (b.ecc_errors > 0 || b.wear_cycles >= MLC_WEAR_THRESHOLD)
    {
        let new_slc_phys = eng.next_phys_slc;
        eng.next_phys_slc += 1;
        let blk = &mut eng.blocks[idx];
        blk.cell_kind = FlashCellKind::Slc;
        blk.phys_block_id = new_slc_phys;
        blk.ecc_errors = 0;
        eng.mlc_to_slc_remaps += 1;
    }
}

/// Writes a block through the FTL with Explicit Confirm Marks (`Attr`), temperature tracking,
/// and Vervain MLC-to-SLC wear monitoring.
pub fn tiered_write_block(lba: u64, payload: &[u8], attr: FtlAttr) -> TieredBlockEntry {
    let mut eng = ENGINE.lock();
    let digest = fnv1a_64(payload);
    let idx = if let Some(i) = eng.blocks.iter().position(|b| b.lba == lba) {
        i
    } else {
        let phys = eng.next_phys_mlc;
        eng.next_phys_mlc += 1;
        eng.blocks.push(TieredBlockEntry {
            lba,
            tier: StorageTier::HighCapacityFlash,
            temperature: BlockTemperature::Cold,
            dirty_fg: false,
            cell_kind: FlashCellKind::Mlc,
            phys_block_id: phys,
            read_count: 0,
            write_count: 0,
            wear_cycles: 0,
            ecc_errors: 0,
            integrity_digest: digest,
            last_tx_id: attr.tx_id,
            nvm_confirmed: false,
            fast_cache_data: Vec::new(),
            slow_flash_data: payload.to_vec(),
        });
        eng.blocks.len() - 1
    };

    {
        let blk = &mut eng.blocks[idx];
        blk.write_count += 1;
        blk.wear_cycles += 1;
        blk.integrity_digest = digest;
        blk.last_tx_id = attr.tx_id;

        if blk.tier == StorageTier::DdrSsdCache {
            blk.fast_cache_data = payload.to_vec();
            blk.dirty_fg = true; // Modified while in fast tier -> FG set
        } else {
            blk.slow_flash_data = payload.to_vec();
            let total_access = blk.read_count + blk.write_count;
            if total_access >= HOT_THRESHOLD {
                blk.tier = StorageTier::DdrSsdCache;
                blk.temperature = BlockTemperature::Hot;
                blk.fast_cache_data = payload.to_vec();
                blk.dirty_fg = true;
            }
        }
    }

    // Explicit FTL Confirm Mark handling
    if attr.require_confirm_mark {
        let tx = attr.tx_id;
        for b in eng.blocks.iter_mut() {
            if b.last_tx_id == tx {
                b.nvm_confirmed = true;
            }
        }
        eng.confirm_marks_written += 1;
    } else {
        eng.blocks[idx].nvm_confirmed = false;
        eng.confirm_marks_skipped += 1;
    }

    check_and_remap_mlc_to_slc(&mut eng, idx);
    eng.blocks[idx].clone()
}

/// Reads a logical block, verifying continuous data integrity, updating temperature state,
/// and promoting Cold blocks to the `DdrSsdCache` tier when hotness reaches `HOT_THRESHOLD`
/// (with `dirty_fg = false` since the cached copy matches slow flash).
pub fn tiered_read_block(lba: u64) -> Result<(Vec<u8>, TieredBlockEntry), &'static str> {
    let mut eng = ENGINE.lock();
    let Some(idx) = eng.blocks.iter().position(|b| b.lba == lba) else {
        return Err("LBA not found in tiered storage");
    };

    let data = {
        let blk = &mut eng.blocks[idx];
        blk.read_count += 1;
        let active_bytes = if blk.tier == StorageTier::DdrSsdCache {
            blk.fast_cache_data.clone()
        } else {
            blk.slow_flash_data.clone()
        };

        // Continuous data integrity check on read
        if fnv1a_64(&active_bytes) != blk.integrity_digest {
            blk.ecc_errors += 1;
        }

        // Promote Cold -> Hot into DDR-SSD Cache when frequently accessed
        let total_access = blk.read_count + blk.write_count;
        if blk.tier == StorageTier::HighCapacityFlash && total_access >= HOT_THRESHOLD {
            blk.tier = StorageTier::DdrSsdCache;
            blk.temperature = BlockTemperature::Hot;
            blk.fast_cache_data = blk.slow_flash_data.clone();
            // Crucial for Smart Recopy Omission: block was only read-promoted, not modified!
            blk.dirty_fg = false;
        }
        active_bytes
    };

    check_and_remap_mlc_to_slc(&mut eng, idx);
    Ok((data, eng.blocks[idx].clone()))
}

/// Demotes a Hot block from `DdrSsdCache` back to `HighCapacityFlash` (Cold tier).
/// Implements **Smart Recopy Omission**: if `dirty_fg == false`, skips the physical recopy
/// to slow flash entirely. Returns `Ok(true)` if recopy was omitted, `Ok(false)` if recopied.
pub fn demote_block_to_cold(lba: u64) -> Result<bool, &'static str> {
    let mut eng = ENGINE.lock();
    let Some(idx) = eng.blocks.iter().position(|b| b.lba == lba) else {
        return Err("LBA not found");
    };
    if eng.blocks[idx].tier != StorageTier::DdrSsdCache {
        return Ok(true);
    }
    let omitted = if eng.blocks[idx].dirty_fg {
        eng.blocks[idx].slow_flash_data = eng.blocks[idx].fast_cache_data.clone();
        eng.blocks[idx].dirty_fg = false;
        eng.recopies_performed += 1;
        false
    } else {
        eng.recopies_omitted += 1;
        true
    };
    eng.blocks[idx].tier = StorageTier::HighCapacityFlash;
    eng.blocks[idx].temperature = BlockTemperature::Cold;
    eng.blocks[idx].fast_cache_data.clear();
    Ok(omitted)
}

/// Injects a simulated flash ECC error or wear spike on `lba` and triggers Vervain's
/// automatic MLC-to-SLC health check and migration.
pub fn report_flash_ecc_error_lba(lba: u64) -> Result<TieredBlockEntry, &'static str> {
    let mut eng = ENGINE.lock();
    let Some(idx) = eng.blocks.iter().position(|b| b.lba == lba) else {
        return Err("LBA not found");
    };
    eng.blocks[idx].ecc_errors += 1;
    check_and_remap_mlc_to_slc(&mut eng, idx);
    Ok(eng.blocks[idx].clone())
}

/// Reports a flash ECC error on the active/first MLC block (or provisions a protective block)
/// and remaps it to high-endurance SLC flash.
pub fn report_flash_ecc_error() -> TieredBlockEntry {
    let target_lba = {
        let eng = ENGINE.lock();
        eng.blocks
            .iter()
            .find(|b| b.cell_kind == FlashCellKind::Mlc)
            .map(|b| b.lba)
            .unwrap_or(5000)
    };
    if ENGINE.lock().blocks.iter().all(|b| b.lba != target_lba) {
        tiered_write_block(
            target_lba,
            b"ime-protected-mlc-block",
            FtlAttr {
                tx_id: 900,
                require_confirm_mark: true,
            },
        );
    }
    report_flash_ecc_error_lba(target_lba).expect("remap active MLC block to SLC")
}

/// Dual-VFS File Creation: writes immutable data blocks into the Data VFS (with streaming
/// FTL confirm marks only on the final block) and creates a separate editable Metadata VFS record.
pub fn create_vfs_file(
    path: &str,
    payload: &[u8],
    attribute_tag: &str,
    retention_expiry_tick: u64,
) -> MetadataVfsEntry {
    let chunk_size = 64usize;
    let chunks: Vec<&[u8]> = if payload.is_empty() {
        alloc::vec![&[][..]]
    } else {
        payload.chunks(chunk_size).collect()
    };

    let (tx_id, base_lba) = {
        let mut eng = ENGINE.lock();
        let tx = eng.next_data_id + 1000;
        let lba = eng.next_auto_lba;
        eng.next_auto_lba += chunks.len() as u64;
        (tx, lba)
    };

    let mut lba_list = Vec::with_capacity(chunks.len());
    for (i, chunk) in chunks.iter().enumerate() {
        let is_final = i + 1 == chunks.len();
        let lba = base_lba + i as u64;
        tiered_write_block(
            lba,
            chunk,
            FtlAttr {
                tx_id,
                require_confirm_mark: is_final,
            },
        );
        lba_list.push(lba);
    }

    let mut eng = ENGINE.lock();
    let data_id = eng.next_data_id;
    eng.next_data_id += 1;
    let content_digest = fnv1a_64(payload);
    eng.data_vfs.push(DataVfsObject {
        data_id,
        lba_list,
        byte_len: payload.len(),
        content_digest,
    });

    if let Some(existing) = eng
        .meta_vfs
        .iter_mut()
        .find(|m| m.path == path && m.snapshot_id == 0)
    {
        existing.data_id = data_id;
        existing.attribute_tag = attribute_tag.to_string();
        existing.retention_expiry_tick = retention_expiry_tick;
        existing.reindex_flag = true;
        return existing.clone();
    }

    let file_id = eng.next_file_id;
    eng.next_file_id += 1;
    let meta = MetadataVfsEntry {
        file_id,
        path: path.to_string(),
        data_id,
        attribute_tag: attribute_tag.to_string(),
        retention_expiry_tick,
        reindex_flag: false,
        snapshot_id: 0,
    };
    eng.meta_vfs.push(meta.clone());
    meta
}

/// IBM-Style Dual-VFS Metadata Mutation: updates editable attributes/tags in the Metadata VFS
/// and automatically sets `reindex_flag = true` without reading or rewriting the Data VFS!
pub fn update_vfs_metadata(
    path: &str,
    snapshot_id: u64,
    new_attribute_tag: &str,
) -> Result<(MetadataVfsEntry, u64), &'static str> {
    let mut eng = ENGINE.lock();
    let Some(meta_idx) = eng
        .meta_vfs
        .iter()
        .position(|m| m.path == path && m.snapshot_id == snapshot_id)
    else {
        return Err("File not found in Metadata VFS");
    };
    eng.meta_vfs[meta_idx].attribute_tag = new_attribute_tag.to_string();
    eng.meta_vfs[meta_idx].reindex_flag = true;
    let data_id = eng.meta_vfs[meta_idx].data_id;
    let digest = eng
        .data_vfs
        .iter()
        .find(|d| d.data_id == data_id)
        .map(|d| d.content_digest)
        .unwrap_or(0);
    Ok((eng.meta_vfs[meta_idx].clone(), digest))
}

/// Non-WORM Copy-on-Write File Modification: allows full mutation of a file in either the root
/// active filesystem (`snapshot_id == 0`) or a writable snapshot branch, allocating a new
/// Data VFS object while leaving other snapshots untouched.
pub fn modify_vfs_file(
    path: &str,
    snapshot_id: u64,
    new_payload: &[u8],
) -> Result<MetadataVfsEntry, &'static str> {
    {
        let eng = ENGINE.lock();
        if snapshot_id != 0 {
            let Some(snap) = eng.snapshots.iter().find(|s| s.snap_id == snapshot_id) else {
                return Err("Snapshot not found");
            };
            if !snap.writable {
                return Err("Snapshot is read-only; promote to writable snapshot first");
            }
        }
        if !eng
            .meta_vfs
            .iter()
            .any(|m| m.path == path && m.snapshot_id == snapshot_id)
        {
            return Err("File not found in target view");
        }
    }

    let (tx_id, lba) = {
        let mut eng = ENGINE.lock();
        let tx = eng.next_data_id + 5000;
        let lba = eng.next_auto_lba;
        eng.next_auto_lba += 1;
        (tx, lba)
    };
    tiered_write_block(
        lba,
        new_payload,
        FtlAttr {
            tx_id,
            require_confirm_mark: true,
        },
    );

    let mut eng = ENGINE.lock();
    let new_data_id = eng.next_data_id;
    eng.next_data_id += 1;
    eng.data_vfs.push(DataVfsObject {
        data_id: new_data_id,
        lba_list: alloc::vec![lba],
        byte_len: new_payload.len(),
        content_digest: fnv1a_64(new_payload),
    });

    let meta = eng
        .meta_vfs
        .iter_mut()
        .find(|m| m.path == path && m.snapshot_id == snapshot_id)
        .ok_or("File missing")?;
    meta.data_id = new_data_id;
    meta.reindex_flag = true;
    Ok(meta.clone())
}

/// Reads a file's content bytes via the Dual-VFS (Metadata VFS -> Data VFS -> Tiered Blocks).
pub fn read_vfs_file(path: &str, snapshot_id: u64) -> Result<Vec<u8>, &'static str> {
    let lbas = {
        let eng = ENGINE.lock();
        let meta = eng
            .meta_vfs
            .iter()
            .find(|m| m.path == path && m.snapshot_id == snapshot_id)
            .ok_or("File not found in Metadata VFS")?;
        let data = eng
            .data_vfs
            .iter()
            .find(|d| d.data_id == meta.data_id)
            .ok_or("Data object missing in Data VFS")?;
        data.lba_list.clone()
    };
    let mut out = Vec::new();
    for lba in lbas {
        let (chunk, _) = tiered_read_block(lba)?;
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Creates a point-in-time Copy-on-Write consistency snapshot with explicit `writable` mode.
pub fn create_snapshot_with_mode(name: &str, writable: bool) -> StorageSnapshot {
    let mut eng = ENGINE.lock();
    let snap_id = eng.next_snap_id;
    eng.next_snap_id += 1;

    let root_files: Vec<MetadataVfsEntry> = eng
        .meta_vfs
        .iter()
        .filter(|m| m.snapshot_id == 0)
        .cloned()
        .collect();

    for mut f in root_files.iter().cloned() {
        let fid = eng.next_file_id;
        eng.next_file_id += 1;
        f.file_id = fid;
        f.snapshot_id = snap_id;
        eng.meta_vfs.push(f);
    }

    let snap = StorageSnapshot {
        snap_id,
        name: name.to_string(),
        writable,
        created_tick: crate::pic::ticks(),
        file_count: root_files.len(),
    };
    eng.snapshots.push(snap.clone());
    snap
}

/// Creates a point-in-time Copy-on-Write consistency snapshot (`writable = false` initially)
/// and can be promoted to a concurrent writable snapshot via `promote_snapshot_writable`.
pub fn create_snapshot(name: &str) -> StorageSnapshot {
    create_snapshot_with_mode(name, false)
}

/// Converts a point-in-time snapshot into a fully writable active filesystem on the fly.
pub fn promote_snapshot_writable(snap_id: u64) -> Result<StorageSnapshot, &'static str> {
    let mut eng = ENGINE.lock();
    let snap = eng
        .snapshots
        .iter_mut()
        .find(|s| s.snap_id == snap_id)
        .ok_or("Snapshot not found")?;
    snap.writable = true;
    Ok(snap.clone())
}

/// Non-WORM Deletion: unhindered permission to delete any file in the active view or a
/// writable snapshot.
pub fn delete_vfs_file(path: &str, snapshot_id: u64) -> Result<(), &'static str> {
    let mut eng = ENGINE.lock();
    let Some(pos) = eng
        .meta_vfs
        .iter()
        .position(|m| m.path == path && m.snapshot_id == snapshot_id)
    else {
        return Err("File not found");
    };
    eng.meta_vfs.remove(pos);
    Ok(())
}

/// Non-WORM Retention Pruning: automatically prunes all files whose retention window has
/// expired at `now_tick` (`retention_expiry_tick > 0 && now_tick >= retention_expiry_tick`).
pub fn prune_expired_vfs_files(now_tick: u64) -> usize {
    let mut eng = ENGINE.lock();
    let before = eng.meta_vfs.len();
    eng.meta_vfs.retain(|m| {
        !(m.retention_expiry_tick > 0 && now_tick >= m.retention_expiry_tick)
    });
    before - eng.meta_vfs.len()
}

/// Returns a summary of the Tiered Storage Engine, Flash Health, FTL Confirm Marks, and Dual-VFS.
pub fn tiered_engine_stats() -> TieredEngineStats {
    let eng = ENGINE.lock();
    let hot = eng
        .blocks
        .iter()
        .filter(|b| b.tier == StorageTier::DdrSsdCache)
        .count();
    let cold = eng.blocks.len().saturating_sub(hot);
    TieredEngineStats {
        total_blocks: eng.blocks.len(),
        hot_blocks: hot,
        cold_blocks: cold,
        recopies_performed: eng.recopies_performed,
        recopies_omitted: eng.recopies_omitted,
        mlc_to_slc_remaps: eng.mlc_to_slc_remaps,
        confirm_marks_written: eng.confirm_marks_written,
        confirm_marks_skipped: eng.confirm_marks_skipped,
        active_files: eng.meta_vfs.len(),
        snapshots: eng.snapshots.len(),
    }
}

// ============================================================================
// PATENT PILLAR C: Virtual Machine Disk Descriptor Merging & Hot Volume Swap
// (CN103853595B — Method and System for Replacing Virtual Machine Disks)
// ============================================================================

/// Virtual disk descriptor containing geometry, backing volume ID, and overlay chain digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmDiskDescriptor {
    pub volume_id: String,
    pub base_golden_image: String,
    pub overlay_path: String,
    pub sectors: u64,
    pub descriptor_generation: u32,
    pub merged_digest: u64,
}

/// Virtual machine configuration whose unique identifiers (`vm_uuid`, `mac_addr`) must be
/// strictly preserved across managed cloud (MIaaS) disk replacement operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmInstanceConfig {
    pub vm_uuid: String,
    pub card_name: String,
    pub mac_addr: [u8; 6],
    pub vcpu_count: u8,
    pub memory_mib: u32,
    pub root_disk: VmDiskDescriptor,
    pub disk_swap_count: u32,
}

/// Merges the incoming `replacement_disk` descriptor with the existing VM's golden/overlay
/// metadata and swaps the active storage volume in-place while preserving `vm_uuid`,
/// `mac_addr`, and hardware topology (`CN103853595B`).
pub fn replace_vm_disk_preserving_uuid(
    vm: &mut VmInstanceConfig,
    replacement_volume_id: &str,
    replacement_overlay: &str,
    new_sectors: u64,
) -> Result<VmDiskDescriptor, &'static str> {
    if replacement_volume_id.is_empty() || replacement_overlay.is_empty() || new_sectors == 0 {
        return Err("Invalid replacement virtual disk descriptor");
    }

    // Merge descriptor attributes (preserving base golden lineage + advancing generation)
    let next_gen = vm.root_disk.descriptor_generation + 1;
    let mut hash_input = Vec::new();
    hash_input.extend_from_slice(vm.vm_uuid.as_bytes());
    hash_input.extend_from_slice(vm.root_disk.base_golden_image.as_bytes());
    hash_input.extend_from_slice(replacement_volume_id.as_bytes());
    hash_input.extend_from_slice(replacement_overlay.as_bytes());
    let merged_digest = fnv1a_64(&hash_input);

    let merged = VmDiskDescriptor {
        volume_id: replacement_volume_id.to_string(),
        base_golden_image: vm.root_disk.base_golden_image.clone(),
        overlay_path: replacement_overlay.to_string(),
        sectors: new_sectors,
        descriptor_generation: next_gen,
        merged_digest,
    };

    vm.root_disk = merged.clone();
    vm.disk_swap_count += 1;
    Ok(merged)
}


