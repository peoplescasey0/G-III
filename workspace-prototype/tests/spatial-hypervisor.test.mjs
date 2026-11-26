import assert from "node:assert/strict";
import { createServer } from "node:http";
import { after, before, test } from "node:test";
import { createSpatialHypervisor } from "../spatial-hypervisor.mjs";
import {
  SPATIAL_PROTOCOL_NAME,
  SPATIAL_PROTOCOL_VERSION,
  WORKSPACE_CAMERA,
  assertProtocol,
  hitTest,
  managedPoses,
  poseToLocal,
  poseToWorld,
  screenPointToRay,
  validatePose,
} from "../spatial-protocol.mjs";

let server;
let baseUrl;

const POSE_AT_ORIGIN = {
  position: { x: 0, y: 0, z: 0 },
  rotation: { x: 0, y: 0, z: 0, w: 1 },
  scale: { x: 1, y: 1, z: 1 },
};

const poseAt = (x, y, z) => ({
  ...POSE_AT_ORIGIN,
  position: { x, y, z },
});

async function post(route, body) {
  return fetch(`${baseUrl}${route}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

before(async () => {
  const hypervisor = createSpatialHypervisor();
  server = createServer((request, response) => {
    hypervisor
      .route(request, response)
      .then((handled) => {
        if (!handled) {
          response.writeHead(404).end();
        }
      })
      .catch((error) => response.writeHead(500).end(String(error)));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  baseUrl = `http://127.0.0.1:${server.address().port}`;

  const res = await post("/api/spatial/sync", {
    cards: [
      { id: "near", pose: poseAt(500, 400, 0), width: 320, height: 200 },
      { id: "far", pose: poseAt(500, 400, 200), width: 320, height: 200 },
    ],
    focused: "near",
  });
  assert.equal(res.status, 200);
});

after(async () => {
  await new Promise((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()));
  });
});

test("state advertises the protocol version and z-order", async () => {
  const response = await fetch(`${baseUrl}/api/spatial/state`);
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("cache-control"), "no-store");
  const state = assertProtocol(await response.json());
  assert.equal(state.protocol.name, SPATIAL_PROTOCOL_NAME);
  assert.equal(state.protocol.version, SPATIAL_PROTOCOL_VERSION);
  // Painter's order: the far card is listed first.
  assert.deepEqual(
    state.cards.map((card) => card.id),
    ["far", "near"],
  );
  assert.equal(state.focusedCardId, "near");
  assert.equal(state.cards.find((card) => card.id === "near").focused, true);
});

test("a fresh hypervisor answers the first state request with 200", async () => {
  // A missing `since` must not be coerced to revision 0 and answered with 304.
  const fresh = createSpatialHypervisor();
  const freshServer = createServer((request, response) => {
    fresh.route(request, response).catch(() => response.writeHead(500).end());
  });
  await new Promise((resolve) => freshServer.listen(0, "127.0.0.1", resolve));
  try {
    const base = `http://127.0.0.1:${freshServer.address().port}`;
    const response = await fetch(`${base}/api/spatial/state`);
    assert.equal(response.status, 200);
    const state = await response.json();
    assert.equal(state.revision, 0);
    assert.deepEqual(state.cards, []);
    assert.equal(state.focusedCardId, null);
  } finally {
    await new Promise((resolve) => freshServer.close(resolve));
  }
});

test("state is unchanged for a matching revision", async () => {
  const first = await (await fetch(`${baseUrl}/api/spatial/state`)).json();
  const second = await fetch(`${baseUrl}/api/spatial/state?since=${first.revision}`);
  assert.equal(second.status, 304);
  const stale = await fetch(`${baseUrl}/api/spatial/state?since=0`);
  assert.equal(stale.status, 200, "an older revision still returns a snapshot");
});

test("hit-testing a ray returns the nearest card", async () => {
  const response = await post("/api/spatial/hit-test", {
    ray: { origin: { x: 500, y: 400, z: -500 }, direction: { x: 0, y: 0, z: 1 } },
  });
  assert.equal(response.status, 200);
  const { hit } = await response.json();
  assert.equal(hit.cardId, "near");
  assert.equal(Math.round(hit.distance), 500);
  assert.equal(hit.u, 0.5);
  assert.equal(hit.v, 0.5);
});

test("hit-testing a screen point uses the workspace camera", async () => {
  const response = await post("/api/spatial/hit-test", { screen: { x: 60, y: 700 } });
  assert.equal(response.status, 200);
  const body = await response.json();
  assert.equal(body.hit, null, "bottom-left of the viewport is empty");
});

test("focus moves the input target to the top of the stack", async () => {
  const response = await post("/api/spatial/focus", { id: "far" });
  assert.equal(response.status, 200);
  const state = await response.json();
  assert.equal(state.focusedCardId, "far");
  assert.deepEqual(state.focusOrder.slice(-2), ["near", "far"]);
  await post("/api/spatial/focus", { id: "near" });
});

test("pointer presses focus the hit card and drags with the real ray delta", async () => {
  const press = await post("/api/spatial/pointer", { x: 500, y: 400, pressed: true, dragged: false });
  assert.equal(press.status, 200);
  const pressed = await press.json();
  assert.equal(pressed.hit.cardId, "near");
  assert.equal(pressed.snapshot.focusedCardId, "near");

  const drag = await post("/api/spatial/pointer", { x: 560, y: 430, pressed: false, dragged: true });
  const dragged = await drag.json();
  const near = dragged.snapshot.cards.find((card) => card.id === "near");
  const scale = 1.0524; // world units per screen pixel at z = 0
  assert.ok(Math.abs(near.pose.position.x - (500 + 60 * scale)) < 2, `x=${near.pose.position.x}`);
  assert.ok(Math.abs(near.pose.position.y - (400 - 30 * scale)) < 2, `y=${near.pose.position.y}`);
  assert.equal(near.pose.position.z, 0);
  assert.equal(near.source, "hypervisor", "a pointer drag is hypervisor-owned");
});

test("a hypervisor-owned pose survives the studio's next sync", async () => {
  const set = await post("/api/spatial/pose", {
    id: "near",
    pose: poseAt(40, 50, 60),
    width: 100,
    height: 100,
  });
  assert.equal(set.status, 200);

  const sync = await post("/api/spatial/sync", {
    cards: [{ id: "near", pose: poseAt(0, 0, 0), width: 100, height: 100 }],
  });
  const state = await sync.json();
  const near = state.cards.find((card) => card.id === "near");
  assert.deepEqual(near.pose.position, { x: 40, y: 50, z: 60 });
  assert.equal(near.source, "hypervisor");
});

test("managedPoses only yields hypervisor-owned cards", async () => {
  const state = await (await fetch(`${baseUrl}/api/spatial/state`)).json();
  const managed = managedPoses(state);
  assert.ok(managed.every((pose) => pose.id === "near"));
  assert.equal(managed.length, 1);
});

test("invalid payloads are rejected with protocol errors", async () => {
  const badPose = await post("/api/spatial/pose", {
    id: "near",
    pose: { ...POSE_AT_ORIGIN, position: { x: Number.NaN, y: 0, z: 0 } },
  });
  assert.equal(badPose.status, 400);
  assert.equal((await badPose.json()).error.code, "invalid_pose");

  const collapsed = await post("/api/spatial/pose", {
    id: "near",
    pose: { ...POSE_AT_ORIGIN, scale: { x: 0, y: 1, z: 1 } },
  });
  assert.equal(collapsed.status, 400);

  const unknown = await post("/api/spatial/focus", { id: "ghost" });
  assert.equal(unknown.status, 404);

  const badRay = await post("/api/spatial/hit-test", {
    ray: { origin: { x: 0, y: 0, z: 0 }, direction: { x: 0, y: 0, z: 0 } },
  });
  assert.equal(badRay.status, 400);
  assert.equal((await badRay.json()).error.code, "invalid_ray");

  const wrongType = await fetch(`${baseUrl}/api/spatial/sync`, {
    method: "POST",
    headers: { "content-type": "text/plain" },
    body: "{}",
  });
  assert.equal(wrongType.status, 415);

  const missing = await post("/api/spatial/nope", {});
  assert.equal(missing.status, 404);
  assert.equal((await missing.json()).error.code, "unknown_route");
});

test("unknown protocol versions are refused by the client guard", () => {
  assert.throws(() => assertProtocol({ protocol: { name: SPATIAL_PROTOCOL_NAME, version: 99 } }), /version/);
  assert.throws(() => assertProtocol({ protocol: { name: "other", version: 1 } }), /unexpected/);
});

test("protocol math mirrors the kernel world model", () => {
  const pose = {
    position: { x: 12, y: -4, z: 7 },
    rotation: { x: 0.318, y: 0.099, z: -0.143, w: 0.932 },
    scale: { x: 2, y: 0.5, z: 1.5 },
  };
  const normalized = validatePose(pose);
  assert.equal(normalized.ok, true);
  const world = poseToWorld(normalized.value, { x: -40, y: 25, z: 0 });
  const back = poseToLocal(normalized.value, world);
  assert.ok(Math.abs(back.x + 40) < 1e-6);
  assert.ok(Math.abs(back.y - 25) < 1e-6);

  const centerRay = screenPointToRay(WORKSPACE_CAMERA, 512, 384);
  assert.ok(centerRay);
  const card = { id: "c", pose: poseAt(512, 384, 0), width: 100, height: 100, active: true };
  const hit = hitTest([card], centerRay);
  assert.ok(hit);
  assert.equal(hit.cardId, "c");
  assert.ok(Math.abs(hit.point.x - 512) < 0.01);
  assert.ok(Math.abs(hit.point.y - 384) < 0.01);
});
