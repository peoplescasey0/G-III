import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { createServer } from "node:http";
import { test } from "node:test";
import { bridgeArguments, createKernelBridge, resolveAccel, resolveQemuPath } from "../kernel-bridge.mjs";
import { createSpatialHypervisor } from "../spatial-hypervisor.mjs";
import { managedPoses, parseKernelFrame } from "../spatial-protocol.mjs";

const FRAME_LINE =
  'SPATIALv1 {"v":1,"rev":7,"focus":1,"cards":[{"id":1,"px":500.000,"py":400.000,"pz":0.000,' +
  '"qx":0.00000,"qy":0.00000,"qz":0.00000,"qw":1.00000,"sx":1.000,"sy":1.000,"sz":1.000,' +
  '"w":320.0,"h":200.0,"active":true,"focused":true}]}';

class FakeChild extends EventEmitter {
  constructor() {
    super();
    this.stdout = new EventEmitter();
    this.stderr = new EventEmitter();
    this.writes = [];
    this.killed = false;
    this.stdin = {
      write: (chunk) => {
        this.writes.push(chunk);
        return true;
      },
      end: () => {
        this.stdinEnded = true;
      },
    };
  }

  send(text) {
    this.stdout.emit("data", text);
  }

  kill() {
    this.killed = true;
    this.emit("exit", null, "SIGTERM");
  }
}

function harness(options = {}) {
  const child = new FakeChild();
  const spawned = [];
  const bridge = createKernelBridge({
    spawnImpl: (command, args) => {
      spawned.push({ command, args });
      return child;
    },
    kernelPath: "/kernels/postcard-minimal-kernel",
    qemuPath: "/usr/bin/qemu-system-x86_64",
    exists: () => true,
    accel: "tcg",
    ...options,
  });
  return { bridge, child, spawned };
}

/** A booted bridge whose fake guest has already printed its shell prompt. */
function bootedHarness(options = {}) {
  const harnessed = harness(options);
  harnessed.bridge.start();
  harnessed.child.send("spatial> ");
  return harnessed;
}

test("resolveQemuPath prefers POSTCARD_QEMU over the platform defaults", () => {
  const seen = [];
  const resolved = resolveQemuPath({
    env: { POSTCARD_QEMU: "/opt/qemu/bin/qemu-system-x86_64", PATH: "" },
    platform: "linux",
    exists: (candidate) => {
      seen.push(candidate);
      return candidate === "/opt/qemu/bin/qemu-system-x86_64";
    },
  });
  assert.equal(resolved, "/opt/qemu/bin/qemu-system-x86_64");
  assert.equal(seen[0], "/opt/qemu/bin/qemu-system-x86_64");
});

test("start boots QEMU headless with the kernel on COM1 stdio", () => {
  const { bridge, spawned } = harness();
  const status = bridge.start();
  assert.equal(status.state, "running");
  assert.equal(spawned.length, 1);
  assert.equal(spawned[0].command, "/usr/bin/qemu-system-x86_64");
  const args = spawned[0].args;
  assert.deepEqual(args.slice(args.indexOf("-kernel"), args.indexOf("-kernel") + 2), [
    "-kernel",
    "/kernels/postcard-minimal-kernel",
  ]);
  assert.deepEqual(args.slice(args.indexOf("-serial"), args.indexOf("-serial") + 2), ["-serial", "stdio"]);
  assert.equal(args[args.indexOf("-display") + 1], "none");
  assert.equal(args.includes("-accel"), false);
  assert.equal(bridge.status().state, "running");
  assert.equal(bridge.status().accel, "tcg");
});

test("KVM is detected from a usable /dev/kvm, and can be forced either way", () => {
  const kvm = { exists: () => true, access: () => {} };
  const unavailable = { exists: () => true, access: () => { throw new Error("EACCES"); } };
  assert.equal(resolveAccel({ env: {}, platform: "linux", ...kvm }), "kvm");
  assert.equal(resolveAccel({ env: {}, platform: "linux", ...unavailable }), "tcg");
  assert.equal(resolveAccel({ env: {}, platform: "darwin", ...kvm }), "tcg");
  assert.equal(resolveAccel({ env: { POSTCARD_QEMU_ACCEL: "tcg" }, platform: "linux", ...kvm }), "tcg");
  assert.equal(resolveAccel({ env: { POSTCARD_QEMU_ACCEL: "kvm" }, platform: "linux", ...unavailable }), "kvm");

  const kvmArgs = bridgeArguments({ kernelPath: "/k", accel: "kvm" });
  assert.deepEqual(kvmArgs.slice(kvmArgs.indexOf("-accel"), kvmArgs.indexOf("-accel") + 2), ["-accel", "kvm"]);
  assert.equal(bridgeArguments({ kernelPath: "/k", accel: "tcg" }).includes("-accel"), false);
});

test("a KVM-accelerated bridge passes -accel kvm to QEMU and reports it", () => {
  const { bridge, spawned } = harness({ accel: "kvm" });
  bridge.start();
  assert.deepEqual(spawned[0].args.slice(spawned[0].args.indexOf("-accel"), spawned[0].args.indexOf("-accel") + 2), [
    "-accel",
    "kvm",
  ]);
  assert.equal(bridge.status().accel, "kvm");
});

test("the first pull waits for the shell prompt instead of writing into a booting guest", async () => {
  const { bridge, child } = harness();
  bridge.start();
  const pending = bridge.requestSnapshot();
  assert.deepEqual(child.writes, [], "nothing is written before the prompt");
  assert.equal(bridge.status().booted, false);
  assert.equal(bridge.status().boot.phase, "booting");
  assert.equal(bridge.status().boot.stage, "spawned");

  child.send("G-III: booting IDT, paging, heap...\n");
  assert.deepEqual(child.writes, [], "boot chatter alone does not open the gate");

  child.send("POST CARD SPATIAL OS - BOOTED!\n");
  child.send("[ok] GDT + TSS loaded\n");
  child.send("[ok] IDT loaded\n");
  assert.equal(bridge.status().boot.stage, "interrupts");
  assert.equal(bridge.status().boot.checks, 2);
  assert.equal(bridge.status().boot.phase, "booting");
  assert.ok(bridge.status().boot.elapsedMs >= 0);

  child.send("spatial> ");
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(child.writes, ["wmjson\r"]);
  assert.equal(bridge.status().booted, true);
  assert.equal(bridge.status().boot.phase, "ready");
  assert.equal(bridge.status().boot.stage, "shell");

  child.send(`${FRAME_LINE}\n`);
  const frame = await pending;
  assert.equal(frame.revision, 7);
});

test("a guest that never reaches its prompt fails with kernel_boot_timeout", async () => {
  const { bridge, child } = harness({ bootTimeoutMs: 40 });
  bridge.start();
  await assert.rejects(bridge.requestSnapshot(), (error) => error.code === "kernel_boot_timeout");
  assert.deepEqual(child.writes, []);
});

test("a newer pull supersedes one that is still waiting for the prompt", async () => {
  const { bridge, child } = harness();
  bridge.start();
  const first = bridge.requestSnapshot();
  const second = bridge.requestSnapshot();
  child.send("spatial> ");
  await assert.rejects(first, (error) => error.code === "superseded");
  await new Promise((resolve) => setImmediate(resolve));
  child.send(`${FRAME_LINE}\n`);
  const frame = await second;
  assert.equal(frame.revision, 7);
  assert.equal(bridge.status().frames, 1);
});

test("a frame split across reads still parses and resolves the request", async () => {
  const { bridge, child } = bootedHarness();
  const pending = bridge.requestSnapshot();
  assert.deepEqual(child.writes, ["wmjson\r"]);

  const cut = Math.floor(FRAME_LINE.length / 2);
  child.send(`${FRAME_LINE.slice(0, cut)}`);
  child.send(`${FRAME_LINE.slice(cut)}\n`);

  const frame = await pending;
  assert.equal(frame.revision, 7);
  assert.equal(frame.cards.length, 1);
  assert.equal(frame.cards[0].id, "card-1");
  assert.equal(frame.focus, "card-1");
  assert.equal(bridge.status().frames, 1);
});

test("console noise is ignored and a malformed frame is counted, not applied", async () => {
  const { bridge, child } = bootedHarness();
  const pending = bridge.requestSnapshot({ timeout: 60 });
  child.send("spatial> wmjson\r\n");
  child.send("SPATIALv1 {not json}\n");
  await assert.rejects(pending, (error) => error.code === "kernel_timeout");
  assert.equal(bridge.status().rejectedFrames, 1);
  assert.equal(bridge.status().frames, 0);
});

test("an unanswered request times out with an actionable error", async () => {
  const { bridge } = bootedHarness();
  await assert.rejects(bridge.requestSnapshot({ timeout: 30 }), (error) => {
    assert.equal(error.code, "kernel_timeout");
    assert.match(error.message, /wmjson/);
    return true;
  });
});

test("QEMU exiting rejects the pending request and marks the bridge failed", async () => {
  const { bridge, child } = bootedHarness();
  const pending = bridge.requestSnapshot({ timeout: 1000 });
  child.emit("exit", 1, null);
  await assert.rejects(pending, (error) => error.code === "qemu_exited");
  assert.equal(bridge.status().state, "failed");
  assert.equal(bridge.status().booted, false, "a dead guest is not reported as booted");
});

test("stop ends the serial channel, kills QEMU and settles the request", async () => {
  const { bridge, child } = bootedHarness();
  const pending = bridge.requestSnapshot({ timeout: 1000 });
  bridge.stop();
  await assert.rejects(pending, (error) => error.code === "bridge_stopped");
  assert.equal(child.killed, true);
  assert.equal(bridge.status().state, "stopped");
});

test("a missing kernel image fails with kernel_not_found instead of spawning QEMU", () => {
  const { bridge, spawned } = harness({ kernelPath: null, repoRoot: "/no/such/repo", exists: () => false });
  assert.throws(
    () => bridge.start(),
    (error) => {
      assert.equal(error.code, "kernel_not_found");
      assert.match(error.message, /make kernel/);
      return true;
    },
  );
  assert.equal(spawned.length, 0);
});

test("parseKernelFrame rejects frames from a different wire version", () => {
  const parsed = parseKernelFrame('SPATIALv1 {"v":2,"rev":1,"cards":[]}');
  assert.equal(parsed.ok, false);
  assert.equal(parsed.error.code, "unsupported_kernel_frame");
  assert.equal(parseKernelFrame("hello").error.code, "invalid_kernel_frame");
});

test("a live kernel frame becomes kernel-owned state the studio adopts", () => {
  const hypervisor = createSpatialHypervisor();
  const parsed = parseKernelFrame(FRAME_LINE);
  assert.equal(parsed.ok, true);

  let snapshot = hypervisor.applyKernelFrame(parsed.value);
  const card = snapshot.cards.find((entry) => entry.id === "card-1");
  assert.equal(card.source, "kernel");
  assert.deepEqual(card.pose.position, { x: 500, y: 400, z: 0 });
  assert.equal(snapshot.focusedCardId, "card-1");
  assert.equal(managedPoses(snapshot).filter((pose) => pose.id === "card-1").length, 1);

  // The studio's 1 Hz pose push must not drag a kernel-driven card back.
  snapshot = hypervisor.applySync({
    cards: [
      {
        id: "card-1",
        pose: {
          position: { x: 0, y: 0, z: 0 },
          rotation: { x: 0, y: 0, z: 0, w: 1 },
          scale: { x: 1, y: 1, z: 1 },
        },
        width: 10,
        height: 10,
      },
    ],
  });
  assert.deepEqual(snapshot.cards.find((entry) => entry.id === "card-1").pose.position, {
    x: 500,
    y: 400,
    z: 0,
  });

  // A card the kernel stops reporting (destroyed there) returns to the studio
  // instead of staying pinned to its last kernel pose.
  snapshot = hypervisor.applyKernelFrame({ version: 1, revision: 8, focus: null, cards: [] });
  assert.equal(snapshot.cards.find((entry) => entry.id === "card-1").source, "studio");
});

test("the kernel control routes start, pull and stop a bridge", async () => {
  const calls = [];
  const frame = parseKernelFrame(FRAME_LINE).value;
  const fakeKernel = {
    status: () => ({ state: "stopped", frames: calls.length, error: null }),
    start: () => {
      calls.push("start");
      return { state: "running" };
    },
    stop: () => {
      calls.push("stop");
      return { state: "stopped" };
    },
    requestSnapshot: async () => {
      calls.push("pull");
      return frame;
    },
  };

  const hypervisor = createSpatialHypervisor({ kernel: fakeKernel });
  const server = createServer((request, response) => {
    hypervisor.route(request, response).catch(() => response.writeHead(500).end());
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${server.address().port}`;

  try {
    const start = await fetch(`${base}/api/spatial/kernel/start`, { method: "POST" });
    assert.equal(start.status, 200);
    assert.equal((await start.json()).kernel.state, "running");

    const pull = await fetch(`${base}/api/spatial/kernel/pull`, { method: "POST" });
    assert.equal(pull.status, 200);
    const pulled = await pull.json();
    assert.equal(pulled.snapshot.cards.find((card) => card.id === "card-1").source, "kernel");

    const status = await fetch(`${base}/api/spatial/kernel`);
    assert.equal(status.status, 200);
    assert.equal((await status.json()).kernel.frames >= 1, true);

    const stop = await fetch(`${base}/api/spatial/kernel/stop`, { method: "POST" });
    assert.equal(stop.status, 200);
    assert.deepEqual(calls, ["start", "pull", "stop"]);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});

test("kernel card ids can be bound to real studio card ids", async () => {
  const frame = parseKernelFrame(FRAME_LINE).value;
  const hypervisor = createSpatialHypervisor({
    kernel: {
      status: () => ({ state: "stopped", frames: 0 }),
      start: () => ({ state: "running" }),
      stop: () => ({ state: "stopped" }),
      requestSnapshot: async () => frame,
    },
  });
  const server = createServer((request, response) => {
    hypervisor.route(request, response).catch(() => response.writeHead(500).end());
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${server.address().port}`;

  try {
    const map = await fetch(`${base}/api/spatial/kernel/map`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ ids: { 1: "postcard-os" } }),
    });
    assert.equal(map.status, 200);
    assert.equal((await map.json()).kernelIds["1"], "postcard-os");

    const pull = await fetch(`${base}/api/spatial/kernel/pull`, { method: "POST" });
    assert.equal(pull.status, 200);
    const snapshot = (await pull.json()).snapshot;
    const card = snapshot.cards.find((entry) => entry.id === "postcard-os");
    assert.ok(card, "the kernel card is bound to the studio card");
    assert.equal(card.source, "kernel");
    assert.equal(snapshot.focusedCardId, "postcard-os");

    const bad = await fetch(`${base}/api/spatial/kernel/map`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ ids: { zero: "postcard-os" } }),
    });
    assert.equal(bad.status, 400);
    assert.equal((await bad.json()).error.code, "invalid_kernel_ids");
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});

test("kernel routes answer 501 when no bridge is attached", async () => {
  const hypervisor = createSpatialHypervisor();
  const server = createServer((request, response) => {
    hypervisor.route(request, response).catch(() => response.writeHead(500).end());
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  try {
    const response = await fetch(`${base}/api/spatial/kernel/pull`, { method: "POST" });
    assert.equal(response.status, 501);
    assert.equal((await response.json()).error.code, "kernel_bridge_unavailable");
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});
