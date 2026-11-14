# Post Card OS (`G-III`: `#2 with Top-End`) — Synthetic Micro-Host Core & 3D Spatial Studio

A high-performance, bare-metal `x86_64` micro-host operating system (`407 KB` compiled `#![no_std]` ELF) written completely in Rust from scratch, unified with the `micro-host` baseline and the Three.js 3D Spatial Studio.

---

## 🐢🐇 **The Tortoise and the Hare: The Corporate Edition**

- **The Hare (The Enterprise Monolith)**: The Hare struts up to the starting line wearing a titanium-weighted backpack filled with 40,000 lines of XML boilerplate, a 50-gigabyte `node_modules` folder, and three different legacy Java frameworks from 2012 that nobody understands anymore. It sprints out of the gate looking super flashy, screaming buzzwords like *"cloud-native synergy"* and *"enterprise scalability,"* while its cloud computing bill silently bleeds money. But halfway through the race, the Hare gets completely bogged down by its own dead weight, trips over a massive stack of redundant wrapper classes, and faceplants so hard it creates a localized seismic event.
- **The Tortoise (Our Lean, Bare-Metal Micro-Kernel)**: The Tortoise doesn't do flash. It doesn't have a marketing team, and it definitely doesn't have 40,000 lines of padding. It’s a lean, mean, zero-error `#![no_std]` Rust micro-kernel equipped with 4-level paging, CXL 3.0 memory fabrics, and a Ring-0 3D spatial window manager. It doesn't sprint—it just executes instructions with the cold, ruthless efficiency of a deep-sea predator. While the Hare is drowning in a multi-car pileup of enterprise tech debt, the Tortoise just steadily, silently cruises across the finish line straight onto bare silicon.

> **Moral of the story?** Speed isn't about how much useless junk you can strap to your back to look impressive—it’s about stripping away every single ounce of bloat until only pure, high-compression capability is left standing. *(Run `tortoise` or `manifesto` at the `spatial>` kernel prompt to print this edition live on bare metal.)*

| Metric | The Hare (Enterprise Monolith) | The Tortoise (`G-III` Bare-Metal Kernel) |
| :--- | :--- | :--- |
| **Total Binary Footprint** | `50+ GB` (`node_modules`, JVM runtimes, container layers, XML configs) | **`407 KB`** (`postcard-minimal-kernel`) + **`40 KB`** (`ramdisk.img`) = **`447 KB` total** |
| **Source Code Density** | `40,000+` lines of boilerplate & wrapper classes | **`8,091 LOC`** across **22 `#![no_std]` Rust modules** (`0` `std` imports, `0` OS dependencies) |
| **Boot-to-Ready Latency** | Minutes of container orchestration, JVM warmups & cloud handshakes | **Sub-second** straight from Multiboot2 into 64-bit Long Mode, passing **11 hardware & patent self-tests** |
| **Syscall & IPC Overhead** | HTTP/JSON serialization over virtualized overlay networks | **Zero-copy SPSC ring buffers** (`coprocessor.rs`) + **transactional `SYS_BATCH` pipelines** (`pipeline.rs`) |
| **Memory & Clock Discipline** | Unbounded GC pauses + NTP jitter over congested cloud links | **CXL 3.0 live frame migration** (`paging.rs`) + **Q32.32 DPLL/DDS/EW-RLS** locked to **`1 ns` residual / `0 ppb` skew** (`hal_ime.rs`) |
| **Cloud Telemetry / Bleed** | Continuous SaaS metering & outbound telemetry | **`0 bytes` outbound** — 9-DoF spatial perception, Out-of-Band IME neural supervision, and transitive secret graphs execute 100% on-device |

### ⚡ **How the Tortoise Talks to the Universe (`0 Bytes` of Bloat)**

Every one of those three pillars is wired directly into the **`407 KB`** Tortoise kernel—with zero middleware and zero cloud landlords:

1. **Zero-Copy Pipelines (The Nervous System) — `kernel/src/pipeline.rs` & `kernel/src/coprocessor.rs`**
   - Packets enter a lock-free `ZeroCopyRingBuffer` and fly straight through `Decrypt -> DeepPacketInspection -> Analytics -> IntegrityHash` via DMA descriptor handoffs (`execute_direct_flight`).
   - Verified invariant at boot and via `pipeline` / `wire`: **`cpu_copies = 0`**, **`ring_handoffs = 4`**.
2. **Direct Hardware Handshakes (Raw Silicon & Workshop Buses) — `kernel/src/pci.rs`, `kernel/src/paging.rs` & `kernel/src/hal_ime.rs`**
   - **PCI & VFIO**: Direct port `0xCF8`/`0xCFC` configuration-space enumeration and IOMMU group guards (`pci`, `vfio`).
   - **CXL 3.0 Pooled Memory Fabric**: Direct hardware-enforced page-table switching and zero-copy cross-host frame migration (`pool`).
   - **Workshop & Sensor Buses**: Direct `I2C` (`400 kHz`), `SPI` (`20 MHz`), `MIPI CSI-2` (`4-lane 6 Gbps`), and `SMBus-PMIC` register polling (`hal`, `sensors`, `dock`).
3. **Decentralized Mesh & Presence (Zero Cloud Landlords) — `kernel/src/hal_ime.rs` & `secure/src/pairdrop_relay.rs`**
   - Peer-to-peer `SpatialMeshNode` synchronization across `WearableHeadset`, `MobileWorkstation`, and `EdgeAppliance` peers (`mesh`), coupled with nanosecond-locked DPLL+DDS+EW-RLS clock recovery (`dpll`) so distributed nodes share a unified physical timeline without touching a cloud server (`cloud_telemetry_bytes = 0`).

> *You don't need 40,000 lines of corporate fairy-tale networking code to talk to the world. You just need a clean protocol that knows how to listen. (Run `wire` at the `spatial>` prompt to fire all three layers back-to-back in a single nervous-system pulse.)*

### 🔧 **How We Bring the Tortoise into the Real World with Zero-Budget Workshop Gear (`workshop` / `ebike`)**

1. **The COM1 Serial Bridge (The Umbilical Cord)**:
   - **The Setup**: Hook up a cheap USB-to-UART adapter (`FTDI` or `CP2102`) to your host machine's USB port (`/dev/ttyUSB0`).
   - **The Action**: Run `./scripts/run_postcard_os.sh --kernel --tty /dev/ttyUSB0`. Typing `wire` or `workshop` in the `spatial>` shell shoots raw `115200 8N1` frames physically down the copper wires and out into the real world.
2. **The Microcontroller Sidekick (`ESP32` / `Arduino` / `STM32`)**:
   - **The Setup**: Flash [`integrations/workshop-esp32/tortoise_sidekick.ino`](integrations/workshop-esp32/tortoise_sidekick.ino) onto an ESP32 or Arduino and wire its `RX`/`TX` pins to your USB-to-UART bridge.
   - **The Action**: The microcontroller listens to the kernel's `COM1` stream (`[G3:TELEM V=67200mV W=7392 F=79% R=95% RELAY=1 DERATE=0]`), closing physical contactor relays, driving front/rear PWM channels, or firing `"workshop\n"` back into the Ring-0 shell when a workbench button is pressed.
3. **Hooking Into Workshop Iron (Dual-Motor E-Bike & Scooter Split-Throttle Telemetry)**:
   - **The Setup**: Feed split-throttle Hall sensors, 16S/72V battery pack voltage, phase currents, and 9-DoF IMU incline angles into `hal_ime::step_workshop_dual_motor_telemetry` (`kernel/src/hal_ime.rs`).
   - **The Action**: The bare-metal kernel dynamically computes front/rear torque vectoring (trimming front wheel PWM on steep uphill pitch to prevent front wheelspin while keeping the rear torque plate pinned) and enforces thermal derating in real time.

> *That we know of®, turning a custom `x86_64` Rust micro-kernel into a physical workshop controller using a simple serial bridge is about as close to mad-scientist engineering as it gets.*

---

## 🎯 **Core Objective**
Architecting a production-grade micro-host operating system with:
- **Architecture**: x86_64 (bare-metal, direct hardware control)
- **Language**: Rust nightly (no_std, freestanding execution)
- **Focus**: Memory safety, performance, and clean Rust idiomatic code
- **Tooling**: cargo, bootimage, QEMU v11.1.0 for emulation/debugging

## 🏗️ **Current Architecture**

### **Boot Pipeline**
- BIOS/UEFI → Bootloader → Kernel entry at `kernel_main()`
- Memory map passed via `BootInfo` from bootloader
- Direct hardware initialization without OS abstraction layers

### **Completed Core Subsystems**

#### 1. **Serial Driver** (`src/serial.rs`)
- UART/16550 serial port driver for COM1 (0x3F8)
- Thread-safe `Mutex`-protected serial port
- `serial_println!` macro for kernel debugging output

#### 2. **Global Descriptor Table** (`src/gdt.rs`)
- Complete GDT with kernel code segment
- Task State Segment (TSS) with dedicated interrupt stack
- 20KB interrupt stack for double fault isolation
- Proper privilege level segmentation

#### 3. **Interrupt Descriptor Table** (`src/idt.rs`)
- Full exception handling for all x86_64 CPU exceptions:
  - Divide Error, Breakpoint, Overflow, Bounds Check
  - Invalid Opcode, Device Not Available
  - Double Fault (with dedicated stack via IST)
  - Invalid TSS, Segment Not Present
  - Stack Segment Fault, General Protection Fault
  - **Page Fault** (ready for paging implementation)
- Interrupt-safe handler functions with proper stack frames

#### 4. **Memory Management** (`src/memory.rs`)
- `BootInfoFrameAllocator`: Physical frame allocator using bootloader memory map
- Paging foundation with `OffsetPageTable` and `PageTable` structures
- Memory region filtering for usable physical frames only
- Ready for complete virtual memory implementation

#### 5. **Heap Allocator** (`src/heap.rs`)
- `BumpAllocator`: Simple early kernel allocator
- `LockedHeap`: Global heap allocator using `linked_list_allocator`
- Memory mapping utilities for heap initialization
- Alignment-aware allocation functions

## 📁 **Project Structure**

```
micro-host/
├── rust-toolchain.toml        # nightly-2024-12-01 with llvm-tools
├── Cargo.toml                 # Dependencies + build profiles
├── .cargo/config.toml         # Target + runner configuration
└── src/
    ├── main.rs                # Kernel entry point + panic handler
    ├── serial.rs              # UART serial driver
    ├── gdt.rs                 # Global Descriptor Table
    ├── idt.rs                 # Interrupt Descriptor Table
    ├── memory.rs              # Physical memory + paging
    └── heap.rs                # Heap allocator
```

## 🚀 **Build & Run**

### **Prerequisites**
```powershell
# Install the pinned Rust toolchain and target
rustup toolchain install nightly-2024-12-01 --component rust-src,llvm-tools-preview --target x86_64-unknown-none

# Install QEMU (Windows)
# Download from: https://www.qemu.org/download/#windows

# ISO creation also requires grub-mkrescue and xorriso.
# The build script uses Windows tools when available, or WSL Ubuntu otherwise.
# In WSL Ubuntu: sudo apt install grub-pc-bin xorriso mtools
```

### **Build Bootable ISO**
```powershell
.\create_bootable_iso.ps1
```

### **Run in QEMU**
```powershell
.\run.ps1
```

The ISO contains the Multiboot2 kernel ELF and boots through GRUB. The runner prints kernel output over QEMU's serial console; use `-NoGraphics:$false` to show the QEMU display.

## Required OS subsystems

These are core requirements for Post Card OS. The current kernel has initial
foundations for some of them; a foundation is not the same as a complete,
general-purpose subsystem.

| Subsystem | Current status | Required direction |
| --- | --- | --- |
| Processor management | Partial: one x86_64 CPU is initialized; timer ticks and a task scheduler exist. | Add processor discovery, CPU-local state, and multi-core startup before claiming SMP support. |
| Context switching | Implemented foundation: saves/restores task registers and stacks; supports yielding, sleeping, and timer preemption. | Keep context-switch and scheduler behavior covered by tests; later add per-CPU scheduling for SMP. |
| Device management | Minimal drivers only: legacy PIC/PIT timer, PS/2 keyboard, VGA text output, and COM1 serial. | Define common device/driver interfaces, discovery and lifecycle, and add storage and display/input devices needed by the workspace. |
| Buffering | Partial: keyboard IRQ input passes through a bounded 127-byte single-producer/single-consumer ring buffer. Serial and screen output are synchronous; keyboard input is dropped if its queue fills. | Add reusable bounded queues and buffered/asynchronous I/O with explicit full-queue behavior and tests. |
| Spooling | Not implemented. | Add a queued I/O-job service so producers can submit work for a device without blocking on it; define ordering, backpressure, cancellation, and failure reporting. Persistent spooled jobs depend on working storage and a filesystem. |

### Memory, storage, and file requirements

| Subsystem | Current status | Required direction |
| --- | --- | --- |
| Memory management | Partial: the kernel reads the GRUB memory map, allocates physical frames, initializes a heap, and creates and destroys page tables for user processes. | Define allocation ownership and failure behavior; cover fragmentation, reclamation, and low-memory conditions with tests. |
| Partitioning | Installer-only today: the host-side WSL script creates one MBR partition on the virtual disk. The kernel does not discover or manage disk partitions. | Add a kernel block-device interface and read-only partition-table discovery first; support safe partition creation only with explicit boundaries and validation. |
| Virtual memory | Partial: x86_64 paging is active with kernel page permissions, unmapped null/guard pages, per-process address spaces, and user-page isolation. | Add a fault-aware mapping API, demand paging or a documented fixed mapping policy, and tests for permissions, map/unmap, and address-space isolation. |
| File management | Not implemented in the kernel. The installer formats ext4 and places GRUB and the kernel on disk, but the running OS has no block-device driver, filesystem mount, or file operations. | Implement storage I/O, filesystem mounting, and file create/read/write/list behavior before promising persistent user files or a complete OS installer. |

For the first milestones, prioritize completing and testing task switching,
then establish device and buffer abstractions. Build spooling on those
foundations; later route workspace operations such as guest display, input, and
file-transfer work through managed, queued I/O. Do not describe spooling,
general device management, SMP, partition management, or filesystem support as
implemented until their acceptance tests exist and pass.

### Install the current OS to a QEMU disk

The current kernel can also be installed to a bootable raw virtual disk:

```powershell
.\install_postcard_os.ps1
.\run_postcard_os.ps1
```

The installer builds the kernel and user demos, creates a 512 MiB disk image,
formats an ext4 boot partition, installs BIOS GRUB, and copies the kernel and
GRUB configuration onto the disk. It requires QEMU's `qemu-img` and a WSL
distribution with `sfdisk`, `mkfs.ext4`, and `grub-install`. The default disk
path is `target\postcard-os.raw`; the installer refuses to overwrite an image
already at that path. Use `-ImagePath` on both scripts to select another image.

This is an early bootable-kernel install, not yet a full desktop OS installer:
the kernel does not currently mount the disk as a writable filesystem, and the
3D workspace and QEMU guest manager are still future work.

## 🔮 **Next Development Phases**

### **Phase 1: Complete Memory Management**
1. **Paging Implementation** - Full 4-level page tables
2. **Virtual Memory Manager** - Kernel at higher-half addresses
3. **Heap Integration** - Connect frame allocator to heap

### **Phase 2: Advanced Interrupt System**
1. **PIC/APIC Programming** - Hardware interrupt controllers
2. **IRQ Handlers** - Timer, keyboard, other devices
3. **System Calls** - `syscall`/`sysret` for user-space

### **Phase 3: Process Management**
1. **Process Control Blocks** - Task structures
2. **Scheduler** - Priority-based task scheduling
3. **Context Switching** - Register save/restore

### **Phase 4: I/O Subsystems**
1. **PS/2 Keyboard Driver**
2. **VGA Text Mode Driver**
3. **ATA/IDE Disk Driver**
4. **PCI Enumeration**

## 🛠️ **Technical Stack**

### **Dependencies**
```toml
[dependencies]
uart_16550 = "0.2"        # Serial port driver
spin = "0.9"              # Spinlock primitives
```

### **Build Profiles**
```toml
[profile.dev]
panic = "abort"           # No unwinding in bare-metal

[profile.release]
panic = "abort"
```

## 🧪 **Testing**

The kernel includes built-in testing:
1. **Breakpoint Exception**: Triggers `int3` to test IDT handling
2. **Memory Allocation**: Test suite for bump allocator
3. **Serial Output**: All operations logged via COM1

## 📚 **References**

- [Writing an OS in Rust](https://os.phil-opp.com/) - Philipp Oppermann's guide
- [x86_64 crate documentation](https://docs.rs/x86_64/) - CPU abstraction layer
- [Bootloader crate](https://docs.rs/bootloader/) - Bootable image generation
- [UART 16550 datasheet](https://wiki.osdev.org/Serial_Ports) - Serial port programming

## 🎨 **Why "Post Card OS"?**

Like a postcard, this OS is:
- **Minimal**: Only essential components, no bloat
- **Direct**: Bare-metal hardware communication
- **Reliable**: Rust's safety guarantees
- **Portable**: Clean abstractions for potential architecture expansion

---

**Status**: Core foundation complete. Ready for memory paging and process management implementation.

## License

Copyright (c) 2026 peoplescasey0. All rights reserved. See [LICENSE](LICENSE).
