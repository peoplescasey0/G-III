/**
 * Kernel bridge — the live COM1 serial link to the Ring-0 spatial hypervisor.
 *
 * The host control plane (`spatial-hypervisor.mjs`) and the studio previously
 * talked to a pure JavaScript mirror of `kernel/src/spatial_wm.rs`. This module
 * replaces that mirror as the source of truth when it is running: it boots
 * `qemu-system-x86_64 -kernel <elf> -serial stdio`, writes the `wmjson` shell
 * command into the guest's COM1, and parses the single line the kernel answers
 * with:
 *
 *   SPATIALv1 {"v":1,"rev":12,"focus":2,"cards":[{"id":1,"px":500.000,...}]}
 *
 * The line format is owned by `World::write_json` in the kernel; `parseKernelFrame`
 * in `spatial-protocol.mjs` validates it and `kernelCardId` maps kernel ids
 * (u32, 1-based) onto studio card ids (`card-1`).
 *
 * The bridge is lazy: nothing is spawned until `start()` or `requestSnapshot()`
 * is called, so the studio keeps working with no QEMU installed.
 *
 * Design notes:
 *  - `spawnImpl` and `exists` are injectable so the whole state machine is
 *    testable on a host with no QEMU (`tests/kernel-bridge.test.mjs`).
 *  - Only one request is in flight; a newer request supersedes an older one
 *    instead of queueing, because a stale pose frame is worth nothing.
 *  - Child output is buffered into whole lines, so a frame split across reads
 *    still parses and shell echo noise is ignored.
 */

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { KERNEL_FRAME_PREFIX, parseKernelFrame } from "./spatial-protocol.mjs";

/** Shell command that makes the kernel print one wire frame. Mirrors `shell.rs`. */
export const KERNEL_REQUEST_COMMAND = "wmjson\r";
export const DEFAULT_TIMEOUT_MS = 2500;
const LOG_LIMIT = 200;
const MAX_BUFFER_BYTES = 64 * 1024;

export const BRIDGE_STATES = Object.freeze({
  STOPPED: "stopped",
  STARTING: "starting",
  RUNNING: "running",
  FAILED: "failed",
});

export class KernelBridgeError extends Error {
  constructor(message, { code = "kernel_bridge_error", status = 503 } = {}) {
    super(message);
    this.name = "KernelBridgeError";
    this.code = code;
    this.status = status;
  }
}

/** Every place a qemu-system-x86_64 binary is worth looking for, best first. */
export function qemuCandidates({ env = process.env, platform = process.platform } = {}) {
  const candidates = [];
  if (env?.POSTCARD_QEMU) candidates.push(env.POSTCARD_QEMU);
  if (platform === "win32") {
    candidates.push(
      path.join(env?.ProgramFiles ?? "C:\\Program Files", "qemu", "qemu-system-x86_64.exe"),
      path.join(env?.["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "qemu", "qemu-system-x86_64.exe"),
    );
  }
  const pathDirs = (env?.PATH ?? "").split(path.delimiter).filter(Boolean);
  for (const name of platform === "win32" ? ["qemu-system-x86_64.exe", "qemu-system-x86_64"] : ["qemu-system-x86_64"]) {
    for (const dir of pathDirs) candidates.push(path.join(dir, name));
  }
  return candidates;
}

export function resolveQemuPath({ env = process.env, platform = process.platform, exists = existsSync } = {}) {
  return qemuCandidates({ env, platform }).find((candidate) => candidate && exists(candidate)) ?? null;
}

/**
 * Kernel image candidates: `POSTCARD_KERNEL`, then the release build that
 * `scripts/build_kernel.sh` writes to the repository's `target/`.
 */
export function kernelCandidates({ repoRoot, env = process.env } = {}) {
  const candidates = [];
  if (env?.POSTCARD_KERNEL) candidates.push(path.resolve(env.POSTCARD_KERNEL));
  if (repoRoot) {
    candidates.push(
      path.join(repoRoot, "target", "x86_64-unknown-none", "release", "postcard-minimal-kernel"),
    );
  }
  return candidates;
}

export function resolveKernelPath({ repoRoot, env = process.env, exists = existsSync } = {}) {
  return kernelCandidates({ repoRoot, env }).find((candidate) => candidate && exists(candidate)) ?? null;
}

/** QEMU arguments for a headless kernel boot with COM1 on this process's stdio. */
export function bridgeArguments({ kernelPath, memoryMb = 256, extraArgs = [] }) {
  return [
    "-name",
    "Post Card Spatial OS (spatial bridge)",
    "-machine",
    "pc",
    "-m",
    `${memoryMb}M`,
    "-kernel",
    kernelPath,
    "-serial",
    "stdio",
    "-monitor",
    "none",
    "-display",
    "none",
    "-no-reboot",
    ...extraArgs,
  ];
}

export function createKernelBridge({
  spawnImpl = spawn,
  kernelPath = null,
  qemuPath = null,
  repoRoot = null,
  command = KERNEL_REQUEST_COMMAND,
  timeoutMs = DEFAULT_TIMEOUT_MS,
  memoryMb = 256,
  extraArgs = [],
  exists = existsSync,
  env = process.env,
  platform = process.platform,
} = {}) {
  const root = repoRoot ?? path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

  const state = {
    state: BRIDGE_STATES.STOPPED,
    error: null,
    child: null,
    startedAt: null,
    lastFrame: null,
    lastFrameAt: null,
    frames: 0,
    rejectedFrames: 0,
    log: [],
    kernelPath: kernelPath ?? resolveKernelPath({ repoRoot: root, env, exists }),
    qemuPath: qemuPath ?? resolveQemuPath({ env, platform, exists }),
  };

  let buffer = "";
  /** @type {{ resolve: Function, reject: Function, timer: any } | null} */
  let pending = null;

  const pushLog = (line) => {
    state.log.push(line);
    if (state.log.length > LOG_LIMIT) state.log.splice(0, state.log.length - LOG_LIMIT);
  };

  const settle = (error, frame) => {
    if (!pending) return;
    const { resolve, reject, timer } = pending;
    pending = null;
    clearTimeout(timer);
    if (error) reject(error);
    else resolve(frame);
  };

  const ingestLine = (line) => {
    pushLog(line);
    if (!line.includes(KERNEL_FRAME_PREFIX)) return;
    const parsed = parseKernelFrame(line);
    if (!parsed.ok) {
      state.rejectedFrames += 1;
      pushLog(`[bridge] rejected frame: ${parsed.error.message}`);
      return;
    }
    state.lastFrame = parsed.value;
    state.lastFrameAt = Date.now();
    state.frames += 1;
    settle(null, parsed.value);
  };

  const ingestChunk = (chunk) => {
    buffer += String(chunk);
    let index = buffer.search(/[\r\n]/);
    while (index !== -1) {
      const line = buffer.slice(0, index);
      buffer = buffer.slice(index + 1);
      if (line.length > 0) ingestLine(line);
      index = buffer.search(/[\r\n]/);
    }
    // A guest that spams the console without newlines must not grow this buffer.
    if (buffer.length > MAX_BUFFER_BYTES) buffer = "";
  };

  const status = () => ({
    protocol: { name: "postcard.spatial.kernel", version: 1 },
    state: state.state,
    error: state.error,
    frames: state.frames,
    rejectedFrames: state.rejectedFrames,
    lastFrameAt: state.lastFrameAt,
    revision: state.lastFrame?.revision ?? null,
    cards: state.lastFrame?.cards?.length ?? 0,
    kernelPath: state.kernelPath,
    qemuPath: state.qemuPath,
    log: [...state.log],
  });

  const start = () => {
    if (state.state === BRIDGE_STATES.RUNNING && state.child) return status();
    // Re-resolve on each start: the kernel image may have been built (or QEMU
    // installed) after this bridge was constructed by `createGuestApi`.
    state.kernelPath = state.kernelPath ?? resolveKernelPath({ repoRoot: root, env, exists });
    state.qemuPath = state.qemuPath ?? resolveQemuPath({ env, platform, exists });
    if (!state.qemuPath) {
      throw new KernelBridgeError(
        "qemu-system-x86_64 was not found. Install it, or point POSTCARD_QEMU at the binary.",
        { code: "qemu_not_found" },
      );
    }
    if (!state.kernelPath) {
      throw new KernelBridgeError(
        "The kernel image was not found. Build it with `make kernel` (or set POSTCARD_KERNEL).",
        { code: "kernel_not_found" },
      );
    }

    buffer = "";
    state.state = BRIDGE_STATES.STARTING;
    state.error = null;
    state.startedAt = Date.now();

    const child = spawnImpl(state.qemuPath, bridgeArguments({ kernelPath: state.kernelPath, memoryMb, extraArgs }), {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    state.child = child;
    state.state = BRIDGE_STATES.RUNNING;

    child.stdout?.setEncoding?.("utf8");
    child.stderr?.setEncoding?.("utf8");
    child.stdout?.on("data", ingestChunk);
    child.stderr?.on("data", ingestChunk);
    child.on("error", (error) => {
      state.child = null;
      state.state = BRIDGE_STATES.FAILED;
      state.error = `Could not start QEMU: ${error.message}`;
      settle(new KernelBridgeError(state.error, { code: "qemu_spawn_failed" }));
    });
    child.on("exit", (code, signal) => {
      if (state.child !== child) return;
      state.child = null;
      if (state.state !== BRIDGE_STATES.STOPPED) {
        state.state = BRIDGE_STATES.FAILED;
        state.error = `QEMU exited (code ${code ?? "?"}${signal ? `, signal ${signal}` : ""})`;
      }
      settle(new KernelBridgeError(`QEMU exited before answering: ${state.error}`, { code: "qemu_exited" }));
    });

    return status();
  };

  const stop = () => {
    const child = state.child;
    state.child = null;
    state.state = BRIDGE_STATES.STOPPED;
    settle(new KernelBridgeError("The kernel bridge was stopped.", { code: "bridge_stopped" }));
    if (child) {
      try {
        child.stdin?.end?.();
      } catch {
        /* the pipe is already gone */
      }
      try {
        child.kill?.();
      } catch {
        /* already exited */
      }
    }
    return status();
  };

  /**
   * Asks the kernel for one fresh frame. Resolves with the parsed frame, or
   * rejects with `KernelBridgeError` (`kernel_timeout`, `qemu_exited`, ...).
   */
  const requestSnapshot = ({ timeout = timeoutMs } = {}) => {
    if (state.state !== BRIDGE_STATES.RUNNING || !state.child) {
      if (state.state === BRIDGE_STATES.FAILED) state.state = BRIDGE_STATES.STOPPED;
      start();
    }
    const child = state.child;
    if (!child?.stdin) {
      throw new KernelBridgeError("The kernel bridge has no serial channel.", { code: "serial_unavailable" });
    }

    // A newer request supersedes an older one: a stale pose frame is useless.
    settle(new KernelBridgeError("Superseded by a newer snapshot request.", { code: "superseded" }));

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (!pending) return;
        pending = null;
        reject(
          new KernelBridgeError(
            "The kernel did not answer on COM1 in time. Boot it once with `-serial stdio` and check that `wmjson` prints a SPATIALv1 line.",
            { code: "kernel_timeout" },
          ),
        );
      }, timeout);
      timer.unref?.();
      pending = { resolve, reject, timer };
      try {
        child.stdin.write(command);
      } catch (error) {
        settle(new KernelBridgeError(`Writing to the serial channel failed: ${error.message}`, { code: "serial_write_failed" }));
      }
    });
  };

  return {
    start,
    stop,
    status,
    requestSnapshot,
    /** Last frame received, without asking the kernel for a new one. */
    lastFrame: () => state.lastFrame,
    /** True when a frame arrived recently enough to be treated as live. */
    isFresh: (maxAgeMs = 10_000) => Boolean(state.lastFrameAt) && Date.now() - state.lastFrameAt <= maxAgeMs,
    log: () => [...state.log],
  };
}
