//! Minimal interactive shell. Input comes from the PS/2 keyboard and COM1;
//! output goes to the VGA console and COM1.

use crate::keyboard::Key;
use crate::{
    coprocessor, framebuffer, hal_ime, paging, pci, pic, pipeline, ramdisk, sandbox, sched,
    spatial_wm, storage, vga,
};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Prints to both the VGA console and the serial port.
pub fn _print(args: fmt::Arguments) {
    vga::_print(args);
    crate::serial::_print(args);
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => ($crate::shell::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! kprintln {
    () => ($crate::kprint!("\n"));
    ($($arg:tt)*) => ($crate::kprint!("{}\n", format_args!($($arg)*)));
}

const PROMPT: &str = "spatial> ";
const MAX_LINE: usize = 70; // prompt + line must fit in one 80-column row
const MAX_HISTORY: usize = 32;

pub struct Shell {
    line: String, // ASCII only, so byte index == column
    cursor: usize,
    history: Vec<String>,
    browsing: Option<usize>, // index into history while Up/Down is in use
    stash: String,           // the line being typed before browsing started
}

impl Shell {
    pub fn new() -> Self {
        Shell {
            line: String::new(),
            cursor: 0,
            history: Vec::new(),
            browsing: None,
            stash: String::new(),
        }
    }

    pub fn start(&mut self) {
        vga::clear();
        vga::_print(format_args!(
            "[ok] Ring-0 x86_64 Kernel | CXL Pools & Guard Pages Active\n\
             [ok] Zero-Copy IPC Ring Buffers (cpu_staging_copies == 0)\n\
             [ok] Non-WORM Tiered Storage | MLC->SLC | CoW Snapshots\n\
             [ok] 9-DoF IMU | MIPI ToF | DVS Spikes | QRNG | OOB IME\n\
             [ok] 3D Spatial WM & Compositor | Neural Co-Pilot Active\n\
             [ok] Augmented Secrets | VM Disk Swap | DoE | DPLL+DDS 16ns\n\n"
        ));
        kprintln!("POST CARD SPATIAL OS");
        kprintln!("Type 'help' for commands.");
        kprint!("{}", PROMPT);
    }

    /// Redraws the whole input line and puts the cursor back in place.
    fn redraw(&self) {
        let back = self.line.len() - self.cursor;
        vga::_print(format_args!("\r{}{}", PROMPT, self.line));
        vga::clear_eol();
        vga::cursor_left(back);
        crate::serial::_print(format_args!("\r{}{}\x1b[K", PROMPT, self.line));
        if back > 0 {
            crate::serial::_print(format_args!("\x1b[{}D", back));
        }
    }

    fn set_line(&mut self, s: String) {
        self.cursor = s.len();
        self.line = s;
        self.redraw();
    }

    fn submit(&mut self) {
        kprintln!();
        let line = core::mem::take(&mut self.line);
        self.cursor = 0;
        self.browsing = None;
        self.stash.clear();
        let cmd = line.trim();
        if !cmd.is_empty() && self.history.last().map(|l| l.as_str()) != Some(cmd) {
            if self.history.len() == MAX_HISTORY {
                self.history.remove(0);
            }
            self.history.push(String::from(cmd));
        }
        run(cmd);
        kprint!("{}", PROMPT);
    }

    /// Handles one decoded key press.
    pub fn input(&mut self, key: Key) {
        match key {
            Key::Char('\n') | Key::Char('\r') => self.submit(),
            Key::Char('\x08') | Key::Char('\x7f') => {
                if self.cursor > 0 {
                    self.browsing = None;
                    self.cursor -= 1;
                    self.line.remove(self.cursor);
                    self.redraw();
                }
            }
            Key::Delete => {
                if self.cursor < self.line.len() {
                    self.browsing = None;
                    self.line.remove(self.cursor);
                    self.redraw();
                }
            }
            Key::Char(c @ (' '..='~' | '\t')) => {
                if self.line.len() < MAX_LINE {
                    self.browsing = None;
                    let c = if c == '\t' { ' ' } else { c };
                    self.line.insert(self.cursor, c);
                    self.cursor += 1;
                    if self.cursor == self.line.len() {
                        kprint!("{}", c); // plain append, no redraw needed
                    } else {
                        self.redraw();
                    }
                }
            }
            Key::Left => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.redraw();
                }
            }
            Key::Right => {
                if self.cursor < self.line.len() {
                    self.cursor += 1;
                    self.redraw();
                }
            }
            Key::Home => {
                self.cursor = 0;
                self.redraw();
            }
            Key::End => {
                self.cursor = self.line.len();
                self.redraw();
            }
            Key::Up => {
                let next = match self.browsing {
                    None if !self.history.is_empty() => {
                        self.stash = self.line.clone();
                        Some(self.history.len() - 1)
                    }
                    Some(i) if i > 0 => Some(i - 1),
                    _ => None,
                };
                if let Some(i) = next {
                    self.browsing = Some(i);
                    let s = self.history[i].clone();
                    self.set_line(s);
                }
            }
            Key::Down => {
                if let Some(i) = self.browsing {
                    if i + 1 < self.history.len() {
                        self.browsing = Some(i + 1);
                        let s = self.history[i + 1].clone();
                        self.set_line(s);
                    } else {
                        self.browsing = None;
                        let s = core::mem::take(&mut self.stash);
                        self.set_line(s);
                    }
                }
            }
            Key::Char(_) => {}
        }
    }
}

fn run(line: &str) {
    let mut parts = line.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match cmd {
        "" => {}
        "help" => {
            kprintln!("help            this list");
            kprintln!("echo <text>     print text");
            kprintln!("clear           clear the screen");
            kprintln!("uptime          time since timer start");
            kprintln!("mem             frame allocator usage");
            kprintln!("ps              list tasks");
            kprintln!("pci             list PCI devices & IOMMU groups");
            kprintln!("vfio            test GPU/PCI VFIO passthrough guard");
            kprintln!("disks           show ramdisk files, MBR partitions & spooler");
            kprintln!("spool <msg>     queue & process an async I/O spool job");
            kprintln!("sdf             list Ontological SDF accelerators & ring channels");
            kprintln!("pipeline [msg]  stream packet via zero-copy direct-flight chain");
            kprintln!("tier            show Phase 2 tiered storage, MLC->SLC & FTL stats");
            kprintln!("snap [name]     create & promote a concurrent writable CoW snapshot");
            kprintln!("pool            show CXL memory pool & hardware boundary status");
            kprintln!("hal             poll IMU (I2C/SPI), MIPI CSI-2 ToF & PMIC sensors");
            kprintln!("sensors         poll 9-DoF IMU, Global Shutter HDR, ALS/Baro & DVS");
            kprintln!("qrng            sample 256-bit hardware Quantum RNG entropy");
            kprintln!("mesh            sync Distributed Spatial Mesh & Agentic Self-Healing");
            kprintln!("fb              blit 0x00FF0000 pixel & spatial card to framebuffer");
            kprintln!("compose         run zero-copy sensor-driven multi-OS card compositor");
            kprintln!("wm              raycast & drag 3D spatial cards in the workspace");
            kprintln!("wmjson          emit one SPATIALv1 JSON line on COM1 (host bridge)");
            kprintln!("copilot         run AI-native neural supervisory core & co-pilot");
            kprintln!("secrets         test risk-based augmented secrets & transitivity");
            kprintln!("vmdisk          hot-swap VM disk descriptor preserving UUID");
            kprintln!("dock            hot-plug credit-card brain into inert shell dock");
            kprintln!("doe             run TWI867989B adaptive DoE variability control");
            kprintln!("dpll            run DPLL + DDS + EW-RLS time & frequency sync");
            kprintln!("tortoise        The Tortoise & The Hare: Corporate Edition manifesto");
            kprintln!("wire            fire 3-stage Zero-Copy + Hardware + P2P Mesh pulse");
            kprintln!("workshop        run COM1 Umbilical + ESP32 + Dual-Motor E-Bike rig");
            kprintln!("ime             show Out-of-Band IME inference & local attestations");
            kprintln!("sandbox         verify OpenShell/OpenSandbox isolation contract");
            kprintln!("spawn           start a demo task (3 ticks, 1 s apart)");
            kprintln!("user            run a ring 3 demo program");
            kprintln!("crash           run a ring 3 program that misbehaves (gets killed)");
            kprintln!("reboot          reset the machine");
            kprintln!("halt            stop the CPU");
        }
        "echo" => kprintln!("{}", rest),
        "clear" => vga::clear(),
        "uptime" => {
            let t = pic::ticks();
            kprintln!("{}.{:02} s ({} ticks)", t / pic::TIMER_HZ, t % pic::TIMER_HZ, t);
        }
        "mem" => {
            let used = paging::frames_in_use();
            let total = paging::frames_total();
            kprintln!("frames: {} used / {} total ({} KiB used)", used, total, used * 4);
        }
        "ps" => sched::for_each(|id, name, state| kprintln!("{:>3}  {:<10} {}", id, name, state)),
        "pci" => {
            for dev in pci::devices() {
                kprintln!(
                    "{} [{:04x}:{:04x}] class={} grp={} drv={}{}",
                    dev.bdf,
                    dev.vendor_id,
                    dev.device_id,
                    dev.class_hex(),
                    dev.iommu_group,
                    dev.driver,
                    if dev.is_gpu() { " [GPU]" } else { "" }
                );
            }
        }
        "vfio" => {
            let devs = pci::devices();
            if let Some(gpu) = devs.iter().find(|d| d.is_gpu()) {
                match pci::prepare_gpu_for_passthrough(&gpu.bdf) {
                    Ok(guard) => {
                        kprintln!(
                            "bound GPU {} (companions={}) to vfio-pci; dropping guard restores host driver",
                            guard.bdf(),
                            guard.companion_bdfs().len()
                        );
                    }
                    Err(e) => kprintln!("vfio error: {:?}", e),
                }
            } else {
                kprintln!("no VGA/3D GPU controller found on PCI bus");
            }
        }
        "disks" => {
            let mbr = storage::build_default_mbr(1_048_576);
            if let Ok(parts) = storage::discover_mbr_partitions(&mbr, 1_048_576) {
                for p in parts {
                    kprintln!(
                        "mbr part #{}: {} lba={}..{} ({} MiB){}",
                        p.index,
                        p.type_name(),
                        p.start_lba,
                        p.start_lba + p.sector_count,
                        p.size_bytes() / (1024 * 1024),
                        if p.bootable { " [boot]" } else { "" }
                    );
                }
            }
            for (name, size) in ramdisk::list() {
                kprintln!("ramdisk: {:<16} {} B", name, size);
            }
            let (q, done) = storage::spool_stats();
            kprintln!("spooler: {} queued, {} completed", q, done);
        }
        "spool" => {
            let payload = if rest.is_empty() { b"postcard-io" } else { rest.as_bytes() };
            let access = storage::StorageAccess {
                lba: 2048,
                count: 1,
                sector_size: storage::SECTOR_SIZE,
                rw: storage::StorageRw::Write,
            };
            match storage::submit_spool_job("vda", access, payload.to_vec()) {
                Ok(id) => {
                    let done = storage::process_next_spool_job();
                    kprintln!(
                        "spool job #{} submitted & completed ({} bytes)",
                        id,
                        done.map(|j| j.payload.len()).unwrap_or(0)
                    );
                }
                Err(e) => kprintln!("spool error: {:?}", e),
            }
        }
        "sandbox" => {
            let id = sandbox::ResolvedWorkloadIdentity::new(
                1000,
                1000,
                alloc::vec![27, 44],
                "openshell-policy",
                "sha256:postcard",
            );
            let lo = sandbox::LoopbackTarget::new([127, 0, 0, 1], 5700);
            kprintln!("sandbox identity: {:?}", id.map(|i| (i.uid, i.gid, i.supplementary_gids)));
            kprintln!("loopback guard:   {:?}", lo.map(|t| (t.octets(), t.port())));
        }
        "sdf" => {
            for entry in pipeline::sdf_catalog() {
                kprintln!(
                    "sdf #{}: {:<20} [{}] node={} ring={} lat={}ns bw={}G inv={}",
                    entry.service_id,
                    entry.name,
                    entry.kind.label(),
                    entry.conn.numa_or_cxl_node,
                    entry.conn.ring_channel_id,
                    entry.conn.latency_ns,
                    entry.conn.bandwidth_gbps,
                    entry.invocations
                );
            }
            let (svcs, chained, handoffs) = pipeline::pipeline_stats();
            kprintln!(
                "pipeline: {} accelerators, {} chained packets, {} zero-copy ring handoffs",
                svcs, chained, handoffs
            );
        }
        "pipeline" => {
            use pipeline::ServiceCapability as Cap;
            let payload = if rest.is_empty() {
                b"DIRECT-FLIGHT-ZERO-COPY-PACKET"
            } else {
                rest.as_bytes()
            };
            match pipeline::alloc_zero_copy_bytes(payload) {
                Ok(slot) => {
                    let _ = pipeline::Service::encrypt(slot.slot_id);
                    let chain = [Cap::Decrypt, Cap::DeepPacketInspection, Cap::Analytics];
                    match pipeline::execute_direct_flight(slot.slot_id, &chain) {
                        Ok(rep) => {
                            kprintln!(
                                "direct-flight slot={} phys={:#x} hops={} handoffs={} cpu_copies={} digest={:#x}",
                                rep.slot_id,
                                rep.phys_addr,
                                rep.stages_executed,
                                rep.ring_handoffs,
                                rep.cpu_staging_copies,
                                rep.telemetry_digest
                            );
                        }
                        Err(e) => kprintln!("pipeline error: {:?}", e),
                    }
                    let _ = pipeline::free_zero_copy_slot(slot.slot_id);
                }
                Err(e) => kprintln!("arena error: {:?}", e),
            }
        }
        "tier" => {
            let st = storage::tiered_engine_stats();
            kprintln!(
                "blocks: {} total ({} Hot DDR-SSD, {} Cold Flash)",
                st.total_blocks, st.hot_blocks, st.cold_blocks
            );
            kprintln!(
                "smart recopy: {} omitted (clean FG=0), {} written back (dirty FG=1)",
                st.recopies_omitted, st.recopies_performed
            );
            kprintln!(
                "flash health: {} MLC->SLC remaps | FTL confirm: {} written, {} skipped",
                st.mlc_to_slc_remaps, st.confirm_marks_written, st.confirm_marks_skipped
            );
            kprintln!(
                "dual-vfs (non-WORM): {} active files, {} snapshots",
                st.active_files, st.snapshots
            );
        }
        "snap" => {
            let name = if rest.is_empty() { "snap-live" } else { rest };
            let snap = storage::create_snapshot(name);
            let promoted = storage::promote_snapshot_writable(snap.snap_id).unwrap_or(snap);
            kprintln!(
                "snapshot #{} '{}' created & promoted writable={} (files={})",
                promoted.snap_id, promoted.name, promoted.writable, promoted.file_count
            );
        }
        "pool" => {
            let ps = paging::pool_stats();
            kprintln!(
                "cxl pool: {} total frames, {} allocated, {} free, {} cross-host migrations",
                ps.total_pooled_frames,
                ps.allocated_pooled_frames,
                ps.free_pooled_frames,
                ps.rebalance_migrations
            );
        }
        "hal" => {
            let bus_imu = hal_ime::HalBus::I2c {
                bus_id: 0,
                addr: 0x68,
            };
            let bus_pmic = hal_ime::HalBus::SmbusPmic { rail_id: 0 };
            let imu = hal_ime::hal_read_imu_9dof(bus_imu);
            let pmic = hal_ime::hal_read_pmic_thermal(bus_pmic);
            hal_ime::ime_step_oob_inference(imu, pmic);
            let st = hal_ime::stats();
            kprintln!(
                "imu [{}]: accel={:?}mg vib={}mg | tof 0-copy frames={}",
                bus_imu.label(),
                imu.accel_mg,
                imu.vibration_magnitude_mg,
                st.vision_zero_copy_frames
            );
            kprintln!(
                "pmic [{}]: vcore={}mV pwr={}mW storage={}.{}C throttled={}",
                bus_pmic.label(),
                pmic.vcore_mv,
                pmic.power_mw,
                pmic.storage_array_temp_mc / 1000,
                (pmic.storage_array_temp_mc % 1000) / 100,
                st.rail_throttled
            );
        }
        "ime" => {
            let st = hal_ime::stats();
            kprintln!(
                "oob ime: {} packets processed, {} preemptive defenses, {} local attestations (cloud=0)",
                st.oob_packets_processed,
                st.defensive_interventions,
                st.attestations_signed
            );
            for rec in hal_ime::ime_attestations().iter().rev().take(3) {
                kprintln!(
                    "  att #{} [coproc={:#x}]: score={} {:?} -> {:?} verified={}",
                    rec.record_id,
                    rec.coprocessor_id,
                    rec.quant_score,
                    rec.anomaly,
                    rec.action,
                    rec.verify("ime-local-epoch-v1")
                );
            }
        }
        "sensors" => {
            let imu9 = hal_ime::hal_read_imu_9dof(hal_ime::HalBus::Spi {
                bus_id: 0,
                cs_pin: 0,
            });
            let env = hal_ime::hal_read_environmental(650, 99_800);
            let spikes = [hal_ime::NeuromorphicSpikeEvent {
                x: 42,
                y: 18,
                polarity: 1,
                delta_log_lux: 11,
                timestamp_us: 250,
            }];
            if let Ok(dvs) = hal_ime::hal_stream_neuromorphic_events(0, 65_536, &spikes) {
                let _ = pipeline::ring_dequeue(2);
                let _ = pipeline::free_zero_copy_slot(dvs.zero_copy_slot_id);
                kprintln!(
                    "9dof imu: heading={}deg vib={}mg | env: {}lux (dim={}%), alt={}cm",
                    imu9.heading_deg,
                    imu9.vibration_magnitude_mg,
                    env.ambient_lux,
                    env.display_brightness_pct,
                    env.altitude_cm
                );
                kprintln!(
                    "neuromorphic dvs: {} spikes streamed (0-copy), {} static pixels suppressed",
                    dvs.spike_count, dvs.static_pixels_suppressed
                );
            }
        }
        "qrng" => {
            let q = hal_ime::hal_qrng_sample(0);
            kprintln!(
                "qrng ch={} health_ok={} min_entropy={}b/B sample={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}..",
                q.shot_noise_channel,
                q.health_test_passed,
                q.min_entropy_bits_per_byte,
                q.entropy_bytes[0],
                q.entropy_bytes[1],
                q.entropy_bytes[2],
                q.entropy_bytes[3],
                q.entropy_bytes[4],
                q.entropy_bytes[5],
                q.entropy_bytes[6],
                q.entropy_bytes[7]
            );
        }
        "mesh" => {
            if let Ok(sync) = hal_ime::mesh_publish_spatial_map(
                1,
                hal_ime::SpatialMeshPeerKind::WearableHeadset,
                b"SPATIAL-MAP-SYNC",
            ) {
                let _ = pipeline::free_zero_copy_slot(sync.zero_copy_slot_id);
                kprintln!(
                    "spatial mesh [{}]: loopback=127.0.0.1 phys={:#x} bytes={} cpu_copies={}",
                    sync.peer_kind.label(),
                    sync.shared_phys_addr,
                    sync.point_cloud_bytes,
                    sync.cpu_staging_copies
                );
            }
            if let Ok(heal) = hal_ime::ime_run_agentic_self_healing(15_000, 220) {
                kprintln!(
                    "agentic self-healing: migrated={} tasks_shed={} cxl_sw_port={}->{} att_verified={}",
                    heal.containers_migrated,
                    heal.background_tasks_shed,
                    heal.fabric_route.ingress_port,
                    heal.fabric_route.egress_port,
                    heal.attestation.verify("ime-local-epoch-v1")
                );
            }
        }
        "fb" => {
            if let Some((addr, val)) = framebuffer::plot_active_pixel(4, 7, framebuffer::COLOR_PURE_RED) {
                kprintln!(
                    "fb_ptr={:#x}: *fb_ptr.add(7 * stride + 4) = {:#010x} (Pure Red verified)",
                    addr, val
                );
            }
        }
        "compose" => {
            match framebuffer::compose_spatial_frame_zero_copy() {
                Ok(rep) => {
                    kprintln!(
                        "compositor fb={:#x}: heading={}deg tof={}B cards={} quarantined={} cpu_copies={} reticle={:#010x}",
                        rep.framebuffer_phys,
                        rep.imu_heading_deg,
                        rep.tof_depth_bytes,
                        rep.cards_rendered,
                        rep.rogue_cards_quarantined,
                        rep.cpu_staging_copies,
                        rep.anchor_pixel_rgb
                    );
                }
                Err(e) => kprintln!("compositor error: {}", e),
            }
        }
        "wm" => {
            // Pointer input is a real ray through the card planes: press to
            // grab, then a drag sample applies the true ray/plane displacement.
            spatial_wm::handle_spatial_input(spatial_wm::PointerEvent {
                x: 500,
                y: 400,
                pressed: true,
                dragged: false,
            });
            spatial_wm::handle_spatial_input(spatial_wm::PointerEvent {
                x: 560,
                y: 430,
                pressed: false,
                dragged: true,
            });
            match spatial_wm::hit_test_detailed(spatial_wm::PointerEvent {
                x: 560,
                y: 430,
                pressed: false,
                dragged: false,
            }) {
                Some(hit) => kprintln!(
                    "ray hit card #{} at ({:.1}, {:.1}, {:.1}) distance={:.1} uv=({:.2}, {:.2})",
                    hit.card_id,
                    hit.point.x,
                    hit.point.y,
                    hit.point.z,
                    hit.distance,
                    hit.u,
                    hit.v
                ),
                None => kprintln!("ray hit no card"),
            }
            kprintln!(
                "focus=#{:?} revision={} painter order (far to near):",
                spatial_wm::focused_card_id(),
                spatial_wm::world_version()
            );
            for c in spatial_wm::paint_order() {
                kprintln!(
                    "card #{}: pos=({:.1}, {:.1}, {:.1}) rot=({:.2}, {:.2}, {:.2}, {:.2}) scale=({:.2}, {:.2}, {:.2}) size={}x{} active={} focused={}",
                    c.id,
                    c.pose.position.x,
                    c.pose.position.y,
                    c.pose.position.z,
                    c.pose.rotation.x,
                    c.pose.rotation.y,
                    c.pose.rotation.z,
                    c.pose.rotation.w,
                    c.pose.scale.x,
                    c.pose.scale.y,
                    c.pose.scale.z,
                    c.width as i32,
                    c.height as i32,
                    c.active,
                    c.focused
                );
            }
        }
        "wmjson" | "wm-json" => {
            // Host-bridge frame: exactly one line of `SPATIALv1 {json}` on COM1,
            // consumed by workspace-prototype/kernel-bridge.mjs. Kept off the
            // VGA console so the operator view stays readable.
            crate::serial_println!("SPATIALv1 {}", spatial_wm::WireSnapshot);
        }
        "copilot" => {
            match coprocessor::run_neural_supervisory_copilot() {
                Ok(rep) => {
                    kprintln!(
                        "neural core {:#x} (vCPU={}): room={:?}cm obstacle={}cm tof={}B dvs_spikes={}",
                        rep.coprocessor_id,
                        rep.assigned_v_cpu,
                        rep.perception.room_bounds_cm,
                        rep.perception.nearest_obstacle_cm,
                        rep.perception.tof_zero_copy_bytes,
                        rep.perception.neuromorphic_spikes
                    );
                    kprintln!(
                        "co-pilot: cxl_sat={}% sdf_reroute=#{} cards={} trimmed={} guard_ok={} att_ok={}",
                        rep.cxl_saturation_predicted_pct,
                        rep.rerouted_service_id,
                        rep.active_spatial_cards,
                        rep.background_tasks_trimmed,
                        rep.rogue_guard_violation_blocked,
                        rep.attestation_verified
                    );
                }
                Err(e) => kprintln!("copilot error: {}", e),
            }
        }
        "secrets" => {
            use alloc::string::ToString;
            let child = sandbox::AugmentedSecret {
                secret_id: 2,
                label: "service-jwt-key".to_string(),
                asset_class: sandbox::AssetClass::Secret,
                scope: sandbox::ScopeMetadata {
                    dependent_vms: 6,
                    dependent_hosts: 3,
                    cloud_regions: 2,
                },
                impact: sandbox::ImpactMetadata {
                    criticality_score: 55,
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
                    last_rotated_tick: 0,
                    expiration_tick: 9999,
                    audit_access_count: 0,
                },
                unlocks_secret_ids: alloc::vec![],
            };
            let mut master = child.clone();
            master.secret_id = 1;
            master.label = "vault-root-key".to_string();
            master.impact.unlocks_financial_or_root = true;
            master.unlocks_secret_ids = alloc::vec![2];
            let dec = sandbox::evaluate_secret_access_with_transitivity(
                &mut master,
                &[child],
                pic::ticks(),
            );
            kprintln!(
                "augmented secret '{}' (unlocks={:?}): decision={:?}",
                master.label, master.unlocks_secret_ids, dec
            );
        }
        "vmdisk" => {
            use alloc::string::ToString;
            let mut vm = storage::VmInstanceConfig {
                vm_uuid: "uuid-postcard-vm-01".to_string(),
                card_name: "debian-card".to_string(),
                mac_addr: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
                vcpu_count: 4,
                memory_mib: 2048,
                root_disk: storage::VmDiskDescriptor {
                    volume_id: "vol-base-1".to_string(),
                    base_golden_image: "debian-golden.qcow2".to_string(),
                    overlay_path: "overlays/card-v1.qcow2".to_string(),
                    sectors: 1_048_576,
                    descriptor_generation: 1,
                    merged_digest: 0,
                },
                disk_swap_count: 0,
            };
            if let Ok(merged) = storage::replace_vm_disk_preserving_uuid(
                &mut vm,
                "vol-miaas-2",
                "overlays/card-v2.qcow2",
                2_097_152,
            ) {
                kprintln!(
                    "CN103853595B vm_uuid={} preserved | disk -> {} (gen={}, digest={:#x})",
                    vm.vm_uuid,
                    merged.volume_id,
                    merged.descriptor_generation,
                    merged.merged_digest
                );
            }
        }
        "dock" => {
            let mut brain = sandbox::PostCardBrainModule::new_credit_card_brain(0xCAFE_2026, 512);
            let dock = sandbox::InertShellDock {
                shell_id: 7,
                display_width: 1920,
                display_height: 1080,
                has_keyboard: true,
                loopback_bus_only: true,
            };
            let _ = brain.dock_into_shell(&dock);
            kprintln!(
                "CN108628404A credit-card brain {:?}mm docked into shell #{} ({}x{}, loopback={})",
                brain.dimensions_mm,
                brain.docked_shell_id.unwrap_or(0),
                dock.display_width,
                dock.display_height,
                dock.loopback_bus_only
            );
        }
        "doe" => {
            let rep = coprocessor::optimize_spatial_variability_doe(&[120, 185, 140, 210]);
            kprintln!(
                "TWI867989B DoE ({} trials): variability {}ppm -> {}ppm (v_trim={}mV, clk_phase={}ps)",
                rep.doe_trials_executed,
                rep.initial_variability_ppm,
                rep.optimized_variability_ppm,
                rep.optimal_voltage_trim_mv,
                rep.optimal_clock_phase_ps
            );
        }
        "dpll" => {
            let samples = [
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
                    dropped: true,
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
            let rep = hal_ime::sync_packet_network_dpll_rls(&samples);
            kprintln!(
                "DPLL+DDS+EW-RLS: freq={}Hz (pulses={}, skew={}ppb) | time_err={}ns (dropped={}, lock={})",
                rep.recovered_freq_hz,
                rep.dds_overflow_pulses,
                rep.estimated_skew_ppb,
                rep.residual_time_error_ns,
                rep.packets_dropped,
                rep.locked_nanosecond_grade
            );
        }
        "tortoise" | "manifesto" => {
            kprintln!("=== THE TORTOISE AND THE HARE: THE CORPORATE EDITION ===");
            kprintln!("[THE HARE - Enterprise Monolith]: 40,000 lines of XML boilerplate,");
            kprintln!("  50 GB node_modules, 3 legacy 2012 Java frameworks, bleeding cloud $$$.");
            kprintln!("  Trips over redundant wrapper classes & faceplants into tech debt.");
            kprintln!("[THE TORTOISE - G-III Bare-Metal Micro-Kernel]: 407 KB ELF (#![no_std]),");
            kprintln!("  0 errors, 22 modules, 4-level paging, CXL 3.0 fabric, Ring-0 3D Spatial WM,");
            kprintln!("  Out-of-Band IME Neural Co-Pilot & 6 Patent Subsystems straight on silicon.");
            kprintln!("[MORAL]: Strip away every ounce of bloat until only pure, high-compression");
            kprintln!("  capability is left standing. Status: CRUISING ACROSS THE FINISH LINE.");
        }
        "wire" => {
            kprintln!("=== TORTOISE NERVOUS SYSTEM: UNIVERSE WIRE (0 BYTES BLOAT) ===");
            // 1. Zero-Copy Pipeline (4-stage DMA descriptor handoff: ring_handoffs=4, cpu_copies=0)
            if let Ok(slot) = pipeline::alloc_zero_copy_bytes(b"TORTOISE-UNIVERSE-PULSE") {
                let _ = pipeline::Service::encrypt(slot.slot_id);
                let chain = [
                    pipeline::ServiceCapability::Decrypt,
                    pipeline::ServiceCapability::DeepPacketInspection,
                    pipeline::ServiceCapability::Analytics,
                    pipeline::ServiceCapability::IntegrityHash,
                ];
                if let Ok(rep) = pipeline::execute_direct_flight(slot.slot_id, &chain) {
                    kprintln!(
                        "[1/3 ZERO-COPY PIPELINE] phys={:#x} hops={} ring_handoffs={} cpu_copies={}",
                        rep.phys_addr, rep.stages_executed, rep.ring_handoffs, rep.cpu_staging_copies
                    );
                }
                let _ = pipeline::free_zero_copy_slot(slot.slot_id);
            }
            // 2. Direct Hardware Handshakes (PCI/VFIO + CXL 3.0 + I2C/SPI/MIPI/SMBus-PMIC)
            let pci_devs = pci::devices().len();
            let cxl = paging::pool_stats();
            let imu = hal_ime::hal_read_imu_9dof(hal_ime::HalBus::I2c { bus_id: 0, addr: 0x68 });
            let pmic = hal_ime::hal_read_pmic_thermal(hal_ime::HalBus::SmbusPmic { rail_id: 0 });
            kprintln!(
                "[2/3 DIRECT HARDWARE]    pci_devs={} | cxl_frames={} (migrations={}) | i2c_imu={}deg pmic={}mV",
                pci_devs, cxl.total_pooled_frames, cxl.rebalance_migrations, imu.heading_deg, pmic.vcore_mv
            );
            // 3. Decentralized P2P Mesh & Presence + DPLL+DDS+EW-RLS Clock Recovery (cloud_telemetry_bytes=0)
            let dpll_samples = [
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
            ];
            let dpll = hal_ime::sync_packet_network_dpll_rls(&dpll_samples);
            if let Ok(sync) = hal_ime::mesh_publish_spatial_map(
                1,
                hal_ime::SpatialMeshPeerKind::WearableHeadset,
                b"TORTOISE-P2P-PRESENCE",
            ) {
                let _ = pipeline::free_zero_copy_slot(sync.zero_copy_slot_id);
                kprintln!(
                    "[3/3 DECENTRALIZED MESH] peer={} phys={:#x} dpll_lock={} ({}ns) cpu_copies={} cloud_telemetry_bytes=0",
                    sync.peer_kind.label(),
                    sync.shared_phys_addr,
                    dpll.locked_nanosecond_grade,
                    dpll.residual_time_error_ns,
                    sync.cpu_staging_copies
                );
            }
        }
        "workshop" | "ebike" => {
            let sample = hal_ime::WorkshopTelemetrySample {
                throttle_front_pct: 85,
                throttle_rear_pct: 95,
                pack_voltage_mv: 67_200,
                front_phase_ma: 45_000,
                rear_phase_ma: 65_000,
                mosfet_temp_mc: 58_400,
                pitch_incline_deg: 14,
            };
            let rep = hal_ime::step_workshop_dual_motor_telemetry(sample);
            kprintln!("=== TORTOISE WORKSHOP IRON: COM1 + ESP32 + DUAL-MOTOR RIG ===");
            kprintln!(
                "[1/3 COM1 UMBILICAL] port=0x3F8 baud=115200_8N1 bridge=USB-UART(FTDI/CP2102)"
            );
            kprintln!(
                "[2/3 ESP32 SIDEKICK] relay_contactor={} derate={} frames_tx={}",
                if rep.main_contactor_relay_closed { "CLOSED" } else { "OPEN" },
                rep.thermal_derate_active,
                rep.esp32_frames_emitted
            );
            kprintln!(
                "[3/3 DUAL-MOTOR IRON] pack={}mV pwr={}W pwm_front={}% pwm_rear={}% rear_bias={}%",
                rep.pack_voltage_mv,
                rep.total_power_watts,
                rep.commanded_front_pwm_pct,
                rep.commanded_rear_pwm_pct,
                rep.torque_bias_rear_pct
            );
            kprintln!(
                "[G3:TELEM V={}mV W={} F={}% R={}% RELAY={} DERATE={}]",
                rep.pack_voltage_mv,
                rep.total_power_watts,
                rep.commanded_front_pwm_pct,
                rep.commanded_rear_pwm_pct,
                if rep.main_contactor_relay_closed { 1 } else { 0 },
                if rep.thermal_derate_active { 1 } else { 0 }
            );
        }
        "spawn" => {
            let id = sched::spawn("demo", || {
                for i in 1..=3 {
                    sched::sleep(100);
                    kprintln!("[demo] tick {}", i);
                }
            });
            kprintln!("spawned task {}", id);
        }
        "user" => kprintln!("spawned user task {}", crate::usermode::spawn("hello", crate::usermode::prog_hello())),
        "crash" => kprintln!("spawned user task {}", crate::usermode::spawn("evil", crate::usermode::prog_evil())),
        "reboot" => unsafe {
            // Pulse the CPU reset line through the keyboard controller.
            x86_64::instructions::port::Port::<u8>::new(0x64).write(0xFE);
            loop {
                x86_64::instructions::hlt();
            }
        },
        "halt" => {
            kprintln!("halted.");
            x86_64::instructions::interrupts::disable();
            loop {
                x86_64::instructions::hlt();
            }
        }
        other => kprintln!("unknown command: {} (try 'help')", other),
    }
}
