# Project: micro-host (Post Card Spatial OS)

## Architecture
Micro-Host is a hybrid bare-metal operating system and spatial hypervisor project structured strictly into five modular tiers:

```
G-III/
├── kernel/                 # 🟢 The Core Metal (Ring 0)
│   ├── src/                # Paging, scheduler, IDT, GDT, usermode dispatcher, shell, memory, heap, pic
│   ├── Cargo.toml          # Kernel dependencies (x86_64, spin, uart_16550, linked_list_allocator)
│   └── linker.ld           # Multiboot2 memory layout (1 MiB load, 32-bit entry, 64-bit bootstrap)
│
├── user/                   # 🟢 The Userland Gremlins (Ring 3)
│   ├── src/bin/            # hello.rs, evil.rs, poke.rs, peek.rs
│   ├── user.ld             # User virtual base (0x80_0000_0000)
│   └── Cargo.toml          # Standalone user crate
│
├── secure/                 # 🟢 The Vault (Security & Hardware Passthrough)
│   ├── src/                # pci.rs, storage.rs, sandbox.rs, vfio guards
│   └── Cargo.toml          # Security/VFIO crate
│
├── workspace-prototype/    # 🟢 The Sci-Fi Dashboard (Frontend & Hypervisor)
│   ├── src/                # Three.js 3D spatial studio, noVNC, UI logic
│   ├── guest-api.mjs       # QEMU/QMP/VNC orchestration backend
│   └── package.json        # Vite & frontend deps
│
├── scripts/                # 🟢 The Automation Engine (Build & Test)
│   ├── build_user.sh       # Compiles userland & packs ramdisk.img
│   ├── build_kernel.sh     # Compiles the bare-metal kernel
│   ├── create_iso.sh       # Generates the Multiboot2 ISO via grub-mkrescue
│   ├── run_postcard_os.sh  # QEMU launcher with serial console
│   └── boot_test.sh        # Headless QEMU verification runner
│
└── Makefile                # 🟢 The Master Command Center (Ties it all together)
```

---

## Feature Inventory
| # | Feature | Description | Milestone | Source |
|---|---------|-------------|-----------|--------|
| 1 | `rust-toolchain.toml` Syntax Fix | Merge duplicate `components` keys on lines 3 & 5 into single array | M1 | Survey (Explorers 1, 2, 3) |
| 2 | Toolchain & Target Verification | Verify `nightly-2024-12-01`, `x86_64-unknown-none`, and `CARGO_INCREMENTAL=0` on host | M1 | Survey & User Request |
| 3 | Linux Build Automation Infra | Canonical `Makefile` and POSIX Bash helper scripts for all build/run/test steps | M1 | Survey (Explorers 1, 3) |
| 4 | Userland Binary Compilation | Compile `hello`, `evil`, `poke`, `peek` at `0x80_0000_0000` with `user.ld` | M2 | Survey (Explorer 2) & User Request |
| 5 | Ramdisk Image Packaging | Repack `"RDSK"` binary archive `ramdisk.img` with verified headers and offsets | M2 | Survey (Explorer 2) & User Request |
| 6 | Modular Architecture Migration | Reorganize project into `kernel/`, `user/`, `secure/`, `scripts/`, `Makefile` | M3 | User Layout Requirement |
| 7 | Secure Vault Integration | Package `pci.rs`, `storage.rs`, `sandbox.rs`, VFIO guards into `secure/` | M3 | User Layout Requirement |
| 8 | Kernel Build Automation (`build_kernel.sh`) | Compile `kernel/` to Multiboot2 ELF via `scripts/build_kernel.sh` | M3 | User Layout Requirement |
| 9 | Multiboot2 ISO Creation (`create_iso.sh`) | Create `target/postcard-spatial-os.iso` with GRUB serial console via `scripts/create_iso.sh` | M4 | Survey & User Request |
| 10 | Raw Disk Image Installation & Boot | Create/format 512 MiB raw MBR disk `target/postcard-os.raw` and install GRUB | M4 | Survey & User Request |
| 11 | Headless QEMU Boot Verification (`boot_test.sh`) | Automated boot test verifying all 13+ kernel selftests pass without crash | M4 | Survey & User Request |
| 12 | Headless QEMU Shell Verification (`shell_test.sh`) | Automated serial shell test verifying interactive commands (`help`, `uptime`, `disks`, `ps`) | M4 | Survey & User Request |
| 13 | Frontend WebSocket Remote Access Fix | Replace hardcoded `127.0.0.1` WebSocket in `main.ts` line 2927 with dynamic host | M5 | Survey (Explorer 3) & User Request |
| 14 | Post Card OS Card in Frontend | Add Post Card Spatial OS card to `initialCards` in `main.ts` with VNC/serial telemetry | M5 | Survey (Explorer 3) & User Request |
| 15 | Frontend Build & Verification | Verify `workspace-prototype` builds cleanly with Vite/TypeScript and runs | M5 | Survey (Explorer 3) |
| 16 | Root Workspace & Master Makefile | Unified root `Makefile` coordinating `kernel`, `user`, `secure`, `scripts`, `workspace-prototype` | M6 | User Layout Requirement |
| 17 | E2E Dual-Track Test Suite (Tiers 1-4) | Comprehensive opaque-box test suite verifying all features end-to-end | M7 (Phase 1) | Dual Track Requirement |
| 18 | Adversarial Coverage Hardening (Tier 5) | White-box stress tests, edge-case hardening, and security fault validation | M7 (Phase 2) | Project Pattern Requirement |

---

## Milestones
| # | Name | Scope | Dependencies | Status |
|---|------|-------|-------------|--------|
| M1 | Toolchain & Linux Automation Infra | Fix `rust-toolchain.toml`, verify rustup/targets, create canonical root `Makefile` and bash scripts | none | DONE |
| M2 | Userland Binaries & Ramdisk Packaging | Compile user programs (`hello`, `evil`, `poke`, `peek`), repack `ramdisk.img` | M1 | DONE |
| M3 | Modular Architecture & Kernel Reorganization | Reorganize into `kernel/` and `secure/`, author `build_kernel.sh`, integrate subsystems | M1, M2 | DONE |
| M4 | Multiboot2 ISO (`create_iso.sh`), Raw Disk & QEMU Tests | Author `create_iso.sh`, format raw disk, run headless QEMU boot and shell tests | M3 | DONE |
| M5 | Workspace-Prototype Frontend Fixes & Card | Fix `127.0.0.1` WebSocket bug, register `postcard-os` card, 7-stage tour, 60 FPS recording | none | DONE |
| M6 | Architecture Roadmap & Root Command Center | Master Makefile, Stripe Monetization Catalog, live sync, workspace pruning & hygiene | M3, M4, M5 | DONE |
| M7 | E2E Test Suite Pass & Adversarial Hardening | Pass 100% of Dual-Track E2E tests (Tiers 1-4), then execute Tier 5 adversarial hardening | M4, M5, M6 | IN_PROGRESS |

---

## Interface Contracts

### 1. Userland ↔ Kernel ABI
- Link base: `0x80_0000_0000` (PML4 slot 1).
- Stack: top at `0x80_0007_5000`, 16 KiB size with 4 KiB unmapped guard page at `0x80_0007_0000`.
- Calling Convention: x86_64 `syscall`/`sysretq`.
  - Registers: RAX = Syscall Number, RDI = Arg 1, RSI = Arg 2, RDX = Arg 3.
  - Return: RAX = signed 64-bit status / value. RCX and R11 clobbered by CPU.
- Syscall Table:
  - `0`: `SYS_WRITE(buf: *const u8, len: usize) -> i64`
  - `1`: `SYS_EXIT(code: i64) -> !`
  - `2`: `SYS_GETPID() -> u64`
  - `3`: `SYS_SLEEP(ticks: u64) -> i64`
  - `4`: `SYS_YIELD() -> i64`
  - `5`: `SYS_WAITPID(task: u64) -> i64` (poll; `-11` while still running)
  - `6`: `SYS_UPTIME_TICKS() -> i64`
  - `32`: `SYS_SPATIAL_CREATE_CARD(*const CardDesc) -> u32 card id`
  - `33`: `SYS_SPATIAL_DESTROY_CARD(card_id: u64) -> i64`
  - `34`: `SYS_SPATIAL_SET_POSE(card_id: u64, *const PoseDesc) -> i64`
  - `35`: `SYS_SPATIAL_FOCUS_CARD(card_id: u64) -> i64`
  - `36`: `SYS_SPATIAL_HIT_TEST(*const RayDesc, *mut HitDesc) -> i64 card id`
- Spatial descriptors are `#[repr(C)]` and owned by `kernel/src/spatial_wm.rs`:
  - `PoseDesc { position: [f32; 3], rotation: [f32; 4] /* xyzw */, scale: [f32; 3] }`
  - `CardDesc { width: f32, height: f32, pose: PoseDesc }`
  - `RayDesc { origin: [f32; 3], direction: [f32; 3] }`
  - `HitDesc { card_id: u32, reserved: u32, distance: f32, point: [f32; 3], local: [f32; 3], u: f32, v: f32 }`
- The dispatcher range-checks every descriptor against the caller's own USER
  pages (`user_range_ok` / `user_range_writable`) before copying it. Errors are
  `-EFAULT (-14)`, `-EINVAL (-22)`, `-ENOENT (-2)` and `-ENOSPC (-28)`.

### 2. Ramdisk Format ("RDSK") ↔ Kernel Embed
- Magic: `[0x52, 0x44, 0x53, 0x4B]` (`"RDSK"`).
- Count: Little-endian `u32`.
- Directory Entries: Array of 32-byte records:
  - `name`: 24 bytes (ASCII, NUL-padded).
  - `offset`: `u32` (byte offset from beginning of archive).
  - `size`: `u32` (byte length).
- Embedded into kernel via `static IMAGE: &[u8] = include_bytes!("../../ramdisk.img");` (or relative path from kernel crate).

### 3. Kernel ↔ Bootloader (Multiboot2)
- Header Magic: `0xe85250d6`, architecture `0` (i386 32-bit protected mode), length `24` or `64`.
- Location: First 32 KiB of ELF file, aligned to 64 bits (offset 4096).
- Load address: `0x00100000` (1 MiB).
- Entry Point: `0x00100018` (`entry32`).
- Kernel registers: EAX = `0x36d76289` (Multiboot2 magic), EBX = physical address of Multiboot2 information structure.

### 4. QEMU Serial Console ↔ Test Harness
- Port: COM1 (`0x3F8`), 115200 baud, 8N1.
- In headless test mode: `-serial stdio -display none -no-reboot` or `-serial file:<path>`.
- Output assertions: 13+ kernel self-tests and prompt `spatial> `.

### 5. Frontend ↔ Hypervisor VM
- WebSocket VNC: `ws://${window.location.hostname || "127.0.0.1"}:${websocketPort}`.
- QMP Control: TCP port `4444+`, JSON protocol for VM management and `send-key`.
- Serial Console: TCP port `4544+` for terminal stream.

### 6. Spatial VM-Control API + kernel bridge (protocol v2)
- Transport: HTTP + JSON under `/api/spatial/`, served by the Vite dev/preview
  middleware, loopback only (`403` otherwise). Every response carries
  `protocol: { name: "postcard.spatial", version: 2 }`; clients must reject an
  unknown name or version. v2 adds the kernel bridge routes and the `kernel`
  card source.
- World model: mirrors `kernel/src/spatial_wm.rs` — pose
  `position/rotation(xyzw)/scale`, camera `1024×768`, FOV `60°`, located at
  `(512, 384, -700)` looking down `+Z`, `+Y` up. One world unit ≈ one viewport
  pixel at `z = 0`; the studio (metres) uses 100 units per metre.
- Routes:
  - `GET  /api/spatial/state?since=<revision>` → snapshot, or `304` when current.
  - `POST /api/spatial/sync` `{ cards: [{ id, pose, width?, height? }], focused? }`
    → snapshot. Poses from the studio are `source: "studio"`.
  - `POST /api/spatial/pose` `{ id, pose, width?, height? }` → snapshot; the pose
    becomes `source: "hypervisor"` and studio syncs no longer overwrite it.
  - `POST /api/spatial/focus` `{ id }` → snapshot (raises the card to the top of
    the focus stack).
  - `POST /api/spatial/hit-test` `{ ray }` or `{ screen: { x, y } }` → `{ hit }`
    (`cardId`, `distance`, `point`, `local`, `u`, `v`) or `null`.
  - `POST /api/spatial/pointer` `{ x, y, pressed, dragged }` → `{ hit, snapshot }`,
    resolved through the camera ray; drags apply the true ray/plane delta.
  - `GET  /api/spatial/kernel` → `{ kernel }` bridge status (state, frame count,
    revision, resolved `qemuPath`/`kernelPath`, recent serial log).
  - `POST /api/spatial/kernel/start` / `/stop` → boot or kill the QEMU that hosts
    the kernel on COM1.
  - `POST /api/spatial/kernel/pull` → asks the live kernel for one frame, applies
    it, returns `{ kernel, snapshot }`. Cards become `source: "kernel"`.
  - `POST /api/spatial/kernel/map` `{ ids: { "1": "postcard-os" } }` → binds
    kernel card ids to studio card ids (default `card-<n>`), so a kernel demo
    card can drive a real card in the studio.
  - `POST /api/spatial/sync` also performs a best-effort background pull while a
    bridge is running, so the studio's existing 1 Hz sync is all it needs.
- Snapshot shape: `revision`, `camera`, `focusedCardId`, `focusOrder`, and
  `cards` in painter's order (farthest first) with `id`, `pose`, `width`,
  `height`, `active`, `focused`, `source`.
- Errors: `{ error: { code, message } }` with `400`/`403`/`404`/`409`/`413`/`415`,
  plus `501` (`kernel_bridge_unavailable`) and `503` (`qemu_not_found`,
  `kernel_not_found`, `kernel_timeout`, `qemu_exited`).
- Implementation: `workspace-prototype/spatial-protocol.mjs` (shared contract and
  3D math) and `workspace-prototype/spatial-hypervisor.mjs` (host world model and
  routes). The studio adopts `source: "hypervisor"` and `source: "kernel"` poses
  verbatim in `src/main.ts`.

#### 6.1 Kernel bridge (live serial link, no longer a mirror only)
- `shell.rs` gained `wmjson`: it prints exactly one line on COM1,
  `SPATIALv1 {"v":1,"rev":N,"focus":M,"cards":[{id,px,py,pz,qx,qy,qz,qw,sx,sy,sz,w,h,active,focused}]}`,
  produced by `World::write_json` (allocation-free, `core::fmt` only). Cards are
  listed in painter's order.
- `workspace-prototype/kernel-bridge.mjs` spawns
  `qemu-system-x86_64 -kernel <elf> -serial stdio -monitor none -display none`,
  writes `wmjson\r`, and parses the answered line; ids map `1` → `card-1`.
- `POST /api/spatial/kernel/start` is required once (the bridge is lazy); after
  that the studio's sync drives the pull. A stopped/absent bridge never affects
  the studio: it falls back to the Node mirror of the same model.
- Because the kernel owns the poses it reports, the studio's 1 Hz `sync` cannot
  drag those cards back to the carousel, and a card the kernel stops reporting
  is handed back to the studio.

---

## Code Layout (User Specified Architecture)
- `rust-toolchain.toml`: Nightly toolchain (`nightly-2024-12-01`) and targets (`x86_64-unknown-none`).
- `kernel/`: Ring 0 bare-metal kernel crate:
  - `kernel/Cargo.toml`, `kernel/linker.ld`, `kernel/src/`.
- `user/`: Ring 3 userland crate:
  - `user/Cargo.toml`, `user/user.ld`, `user/src/lib.rs`, `user/src/bin/{hello,evil,poke,peek}.rs`.
- `secure/`: Security and hardware passthrough crate:
  - `secure/Cargo.toml`, `secure/src/{pci.rs,storage.rs,sandbox.rs}`.
- `workspace-prototype/`: Spatial studio hypervisor frontend:
  - `package.json`, `vite.config.mjs`, `index.html`, `src/main.ts`, `guest-api.mjs`.
- `scripts/`: Build and test automation:
  - `build_user.sh`: Builds userland & packs ramdisk.img.
  - `build_kernel.sh`: Compiles bare-metal kernel.
  - `create_iso.sh`: Generates Multiboot2 ISO via grub-mkrescue.
  - `boot_test.sh`: Headless QEMU verification runner.
  - `shell_test.sh`: Headless QEMU serial shell test runner.
  - `run_postcard_os.sh`: QEMU interactive runner.
- `Makefile`: Master Command Center.
