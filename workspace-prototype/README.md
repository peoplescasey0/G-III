# Post Card OS — Workspace Prototype

An early host-side prototype of the spatial workspace concept. It is
intentionally separate from the bare-metal kernel so the interaction model can
be explored while the OS foundation continues to develop.

## Run

```powershell
cd workspace-prototype
npm install
npm run dev
```

Open the local URL printed by Vite. To create a production bundle, run
`npm run build`.

## Prototype interactions

- Drag cards to arrange them in the 3D space. A quick flick instead throws the
  card: the flick's direction and speed are the throw vector, a mint aim ray and
  reticle mark the valid landing card, and the card flies an arc to it (or into
  the room when nothing valid is in the way). It stops on contact with another
  card rather than passing through, and opens where it lands.
- Scroll over the studio or focus the canvas and use the arrow keys to browse
  cards; **Home** and **End** jump to the first and last card.
- Double-click a card, press **Enter** while the canvas is focused, or use
  the active card's **Settings** action to see its settings side. The selected
  card's compact action strip follows it through the 3D carousel; use it to
  install, launch, resume, or open a guest, request shutdown while it is running,
  or move the card to the paper bin.
- Moving a card to the paper bin folds it into a paper airplane and removes it
  from the active room without touching its guest disk, checkpoints, TPM/UEFI
  state, or private data. Open **The Paper Bin** in the room corner to restore a
  card; binned cards remain there across reloads.
- A quick Shift/right-drag flick into the bin is still the separate
  **permanent-delete** action. It requires the existing confirmation and removes
  that card’s private guest state. Use **Move to bin** for reversible hiding.
- Use **Send a paper plane** to send a local demo note to another card; flip the
  recipient card to read it.
- Use **+** to add another system card.
- Open **Performance** to select a per-card virtual CPU profile (Eco, Balanced,
  or Gaming) for the next guest launch, or tune the local 3D studio's render
  quality. The render setting affects the studio only. This QEMU/VNC prototype
  does not support host CPU/GPU overclocking or guest GPU passthrough.

The Windows and Linux cards can launch live QEMU guests, show their VNC display
in the workspace, and forward keyboard and pointer input through noVNC. Linux
cards share a Debian golden base with persistent per-card overlays. Windows
cards use a persistent 64 GiB disk and their own software TPM 2.0 state. Up to
four guests and 8 GiB of guest RAM can be active at once. Android remains an
illustration, and paper-plane notes are still local prototype behavior.

## Spatial hypervisor control plane

The Ring-0 spatial hypervisor (`kernel/src/spatial_wm.rs`) models the workspace
in 3D: every card has a pose (position, rotation quaternion, scale), cards are
depth ordered, and pointer input is resolved by casting a ray from the camera
through the card planes. That model is exposed to this prototype through a
small, versioned VM-control API (`protocol v2`, `postcard.spatial`), documented
in `PROJECT.md` §6 and implemented by `spatial-protocol.mjs` and
`spatial-hypervisor.mjs`.

- `GET /api/spatial/state` — world snapshot: revision, camera, focus stack and
  cards in painter's order.
- `POST /api/spatial/sync` — the studio pushes its card poses once a second.
- `POST /api/spatial/pose` — a controller claims a card; the studio then pins it
  to that pose (`source: "hypervisor"`) until the studio claims it back.
- `POST /api/spatial/focus`, `/hit-test`, `/pointer` — focus stack, ray-plane hit
  testing and true camera-ray dragging.

All routes are loopback only. Without a running kernel the control plane answers
from a Node mirror of the same math, so the studio behaves exactly as before —
the sync is best-effort.

## Live kernel bridge

`kernel-bridge.mjs` replaces that mirror with the real thing: the kernel prints
one `SPATIALv1 {json}` line on COM1 when the shell runs `wmjson`, and the bridge
boots it under QEMU, asks for that line, and applies it. Cards the kernel reports
become `source: "kernel"` and are pinned to the kernel's pose.

```bash
# once, with the dev server running (or use the same routes from any loopback client)
curl -X POST http://127.0.0.1:5173/api/spatial/kernel/start
curl -X POST http://127.0.0.1:5173/api/spatial/kernel/pull
curl        http://127.0.0.1:5173/api/spatial/kernel
```

Kernel cards are addressed as `card-1`, `card-2`, … Bind them to real studio cards
(the `postcard-os`, `android`, … cards) before pulling:

```bash
curl -X POST http://127.0.0.1:5173/api/spatial/kernel/map \
  -H 'content-type: application/json' -d '{"ids":{"1":"postcard-os"}}'
```

`make kernel` stages the image and `ramdisk.img` into
`workspace-prototype/data/master-images/` automatically (run `make stage-vm` to
restage without rebuilding), which is where the guest API loads them from by
default; `POSTCARD_KERNEL`/`POSTCARD_RAMDISK` override those paths. `make
check-vm` verifies the staged images, the Debian base and its ready marker, the
Windows media/firmware/TPM directories and the QEMU binaries, and reports the
exact path or environment variable for anything missing. QEMU
is found at `C:\Program Files\qemu\qemu-system-x86_64.exe` on Windows, on `PATH`
elsewhere, or via `POSTCARD_QEMU`. The bridge is lazy — nothing spawns until you
ask — and `POST /api/spatial/kernel/stop` shuts it down. Once started, the studio's
existing 1 Hz sync pulls a frame in the background, so the 3D cards follow the
kernel's own pose state.

The first pull after `start` waits for the kernel's `spatial>` prompt before it
writes `wmjson`; a command sent into a COM1 that nobody is reading yet would be
dropped, which is why a cold start used to end in `kernel_timeout`. A boot that
never reaches the prompt now fails with the distinct `kernel_boot_timeout`.

On Linux the bridge adds `-accel kvm` whenever `/dev/kvm` is readable and
writable, so a boot takes a fraction of a second instead of the roughly ten
seconds TCG needs. Force either mode with `POSTCARD_QEMU_ACCEL=tcg` or
`POSTCARD_QEMU_ACCEL=kvm`; `GET /api/spatial/kernel` reports the accelerator in
use alongside `booted`.

The same status carries `boot`: `phase` (`booting`, `ready`, `failed` or
`stopped`), the last milestone the kernel printed (`banner`, `interrupts`,
`memory`, `alive`, `shell`), how many `[ok]` self-checks passed, and the elapsed
milliseconds. The studio polls that route once a second and shows it as a
`KERNEL` chip in the topbar, so a slow boot reads as progress instead of a hung
pull.

If QEMU or the kernel image is missing the routes return `503` with the exact
path they looked for instead of failing silently.

## Connect the Linux guest

The guest service is part of the Vite development and preview servers; it
listens only for requests from this computer. Install QEMU and keep the Debian
netinst ISO at `..\archive\guest-vm-setup\debian-13.7.0-amd64-netinst.iso`, or
set `POSTCARD_DEBIAN_ISO` to its location. On Windows the default QEMU paths are
`C:\Program Files\qemu\qemu-system-x86_64.exe` and `qemu-img.exe`; override them
with `POSTCARD_QEMU` and `POSTCARD_QEMU_IMG` if they are installed elsewhere.

1. Run `npm run dev` and open the local Vite URL.
2. Select a Linux card, then choose **Install Debian base**. The first launch
   creates `workspace-prototype\data\master-images\debian-golden.qcow2` and
   boots the installer. Complete Debian installation in the live display.
3. Shut down the installer guest. Choose **Finish base installation** only
   after Debian is installed; this marks the image as the shared backing image.
4. Choose **Boot this card**. The host creates a persistent
   `workspace-prototype\data\overlays\<card-id>.qcow2` overlay and boots it.
   Each Linux card receives a separate overlay backed by the same golden image.
5. Other Linux cards can be launched while a card is running, up to four at
   once. Existing overlays persist across guest restarts. The finalized base is
   locked against further installer launches to protect existing card overlays.

The new base and overlays are stored under the ignored `workspace-prototype\data`
directory; archived sample images are not used or modified. Closing the display
only disconnects noVNC; it does not stop QEMU. Use **Request shutdown** to ask
the guest OS to power down gracefully. The guest uses QEMU user-mode networking.
Guests include a VirtIO balloon device; host memory page merging is disabled
because it is unsupported by some QEMU host configurations.
VNC and QMP listen on loopback ports 5700 and 4444; do not expose those ports
to a network. QEMU guests use unique local display/control ports (the first
guest uses WebSocket port 5700 and QMP port 4444; later guests use subsequent
ports). `POSTCARD_VM_DATA`, `POSTCARD_LINUX_BASE`, and `POSTCARD_DEBIAN_ISO`
can select the data directory, golden image, and installer ISO. The installer
may ignore ACPI shutdown requests; in that case the display offers a clearly
labeled force-stop action that can discard unsaved changes. Running multiple
guests increases host CPU and memory use.

## Connect a Windows guest (bring your own ISO)

Windows install media is **not included**: you must supply your own licensed ISO. The guest runs through the TPM-enabled QEMU build in WSL 2, not the native
Windows QEMU installation. Install Ubuntu (or another configured WSL
distribution) and its QEMU, software TPM, and OVMF packages:

```bash
sudo apt update
sudo apt install qemu-system-x86 swtpm ovmf
```

Set `POSTCARD_WINDOWS_ISO` to the ISO's Windows path. A local `.env` file is
supported; WSL UNC paths such as
`\\wsl.localhost\Ubuntu\home\user\Windows.iso` are converted to Linux paths
inside the configured `POSTCARD_WSL_DISTRO` (default `Ubuntu`), because QEMU
runs inside WSL. Start the workspace with `npm run dev`, select **Windows ·
Studio**, then choose **Install Windows (your ISO)**. The service creates a persistent
per-card 64 GiB QCOW2 disk, per-card UEFI variables, and private TPM 2.0 state
under `workspace-prototype\data\windows`.
After installation, use **Boot Windows** or **Resume Windows** to start it from
the card disk. The TPM state is part of the guest identity and must not be
deleted if you want to preserve the installation's TPM-bound data.

Configuration options are `POSTCARD_WSL` (defaults to `wsl.exe`),
`POSTCARD_WSL_DISTRO` (defaults to `Ubuntu`), `POSTCARD_WSL_QEMU`,
`POSTCARD_WSL_QEMU_IMG`, `POSTCARD_WSL_SWTPM`, `POSTCARD_WSL_OVMF_CODE`,
`POSTCARD_WSL_OVMF_VARS`, and `POSTCARD_WINDOWS_ISO`. The default OVMF firmware
paths are the Microsoft Secure Boot 4 MiB code and vars templates installed by
Ubuntu's `ovmf` package.

**Performance note:** this machine's WSL environment has no `/dev/kvm`, so the
Windows guest uses QEMU software emulation (TCG). Windows installation and
desktop use may be very slow. TPM 2.0 is provided by `swtpm`; VNC, QMP, and TPM
ports bind to loopback only. Do not expose them to a network.

The workspace is a quiet, night-time spatial studio: layered architectural
surfaces, a softly lit circular dais, and tactile paper cards with metallic
edges. Cards sit on a curved arc; scrolling or keyboard navigation changes the
focused card. Each card's guest display preview occupies about one quarter of
its face. Only the selected card shows its live guest framebuffer; inactive
cards align edge-on to the camera and replace cached framebuffer or console
output with a private-display indicator. Mouse, generated navigation, and
keyboard selection all update this state. With reduced motion enabled, card
selection and zoom settle immediately, keyboard navigation still works as
direct card selection, carousel dragging is disabled, settings flips snap to
their destination, and ambient scene loops pause. Card movement and delivery
animations also respect the system reduced-motion preference. Moving a card to
the bin honors reduced motion and provides the same immediate state change and
status announcement without animation.
Rendering resolution is capped to keep the scene responsive, especially on
smaller screens.
