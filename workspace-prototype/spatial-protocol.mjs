/**
 * Post Card OS — spatial VM-control API (protocol v2).
 *
 * The versioned contract between the Three.js studio / `guest-api.mjs` and the
 * Ring-0 spatial hypervisor (`kernel/src/spatial_wm.rs`). The kernel is the
 * authority for card poses, z-order and pointer hit testing; this module mirrors
 * its 3D math in JavaScript so the host side can answer the same questions the
 * kernel does and speak the same wire format.
 *
 * v2 adds the kernel bridge: `GET/POST /api/spatial/kernel*` in
 * `spatial-hypervisor.mjs`, the `kernel` card source, and the `SPATIALv1` serial
 * frame format that `kernel-bridge.mjs` parses straight off COM1. Cards whose
 * pose came from the live kernel are owned by source `"kernel"`.
 *
 * Wire format (JSON, loopback only, mounted at `/api/spatial/`):
 *
 *   GET  /api/spatial/state?since=<revision>
 *        -> 200 world snapshot, or 304 when `since` is current
 *   POST /api/spatial/sync         { cards: [{id, pose, width?, height?}], focused? }
 *        -> 200 world snapshot (the studio's poses are authoritative for
 *           `source: "studio"` cards)
 *   POST /api/spatial/pose         { id, pose, source?: "hypervisor" }
 *        -> 200 snapshot (hypervisor/agent override for one card)
 *   POST /api/spatial/focus        { id } -> 200 snapshot
 *   POST /api/spatial/hit-test     { ray } | { screen: {x, y} } -> 200 { hit }
 *   POST /api/spatial/pointer      { x, y, pressed, dragged }
 *        -> 200 { hit, snapshot }
 *   GET  /api/spatial/kernel       -> 200 { kernel } bridge status
 *   POST /api/spatial/kernel/start -> 200 { kernel } boot QEMU on COM1
 *   POST /api/spatial/kernel/stop  -> 200 { kernel }
 *   POST /api/spatial/kernel/pull  -> 200 { kernel, snapshot } one kernel frame
 *        applied to the world (cards become source: "kernel")
 *   POST /api/spatial/kernel/map   { ids: { "1": "postcard-os" } }
 *        -> 200 { kernel, kernelIds } bind kernel card ids to studio card ids
 *           (defaults to `card-<n>`)
 *
 * Every response body includes `protocol: { name, version }`. A client MUST
 * reject a response whose protocol name or version it does not know.
 */

export const SPATIAL_PROTOCOL_NAME = "postcard.spatial";
export const SPATIAL_PROTOCOL_VERSION = 2;

/** Who owns a card's pose. `studio` animates it, the others override it. */
export const SPATIAL_SOURCES = Object.freeze({
  STUDIO: "studio",
  HYPERVISOR: "hypervisor",
  KERNEL: "kernel",
});

/** Mirrors `MAX_CARDS` in `kernel/src/spatial_wm.rs`. */
export const MAX_SPATIAL_CARDS = 32;
/** Mirrors `MIN_SCALE`; a smaller |scale| axis is a degenerate pose. */
export const MIN_SCALE = 1e-4;
const PARALLEL_EPSILON = 1e-6;
const TIE_EPSILON = 1e-4;
const MAX_CARD_EXTENT = 16384;

/** Mirrors `Camera::workspace()` in the kernel. */
export const WORKSPACE_CAMERA = Object.freeze({
  viewport: Object.freeze({ width: 1024, height: 768 }),
  fovY: Math.PI / 3,
  position: Object.freeze({ x: 512, y: 384, z: -700 }),
  rotation: Object.freeze({ x: 0, y: 0, z: 0, w: 1 }),
});

const fail = (code, message) => ({ ok: false, error: { code, message } });

export const isFiniteNumber = (value) => typeof value === "number" && Number.isFinite(value);

function readVec3(value, what) {
  if (!value || typeof value !== "object") return fail("invalid_pose", `${what} must be an object`);
  const { x, y, z } = value;
  if (!isFiniteNumber(x) || !isFiniteNumber(y) || !isFiniteNumber(z)) {
    return fail("invalid_pose", `${what} must have finite x, y and z`);
  }
  return { ok: true, value: { x, y, z } };
}

function readQuat(value) {
  if (!value || typeof value !== "object") {
    return fail("invalid_pose", "pose.rotation must be an object");
  }
  const { x, y, z, w } = value;
  if (![x, y, z, w].every(isFiniteNumber)) {
    return fail("invalid_pose", "pose.rotation must have finite x, y, z and w");
  }
  const length = Math.hypot(x, y, z, w);
  if (!(length > 0.5)) return fail("invalid_pose", "pose.rotation must be a non-zero quaternion");
  return { ok: true, value: { x: x / length, y: y / length, z: z / length, w: w / length } };
}

/**
 * Validates and normalises a pose. Returns `{ ok: true, value }` or
 * `{ ok: false, error: { code, message } }`. Rejects exactly what the kernel
 * rejects: non-finite values and collapsed scale axes.
 */
export function validatePose(pose) {
  if (!pose || typeof pose !== "object") return fail("invalid_pose", "pose is required");
  const position = readVec3(pose.position, "pose.position");
  if (!position.ok) return position;
  const rotation = readQuat(pose.rotation);
  if (!rotation.ok) return rotation;
  const scale = readVec3(pose.scale, "pose.scale");
  if (!scale.ok) return scale;
  for (const axis of ["x", "y", "z"]) {
    if (Math.abs(scale.value[axis]) < MIN_SCALE) {
      return fail("invalid_pose", `pose.scale.${axis} collapses the card`);
    }
  }
  return { ok: true, value: { position: position.value, rotation: rotation.value, scale: scale.value } };
}

/** Validates a card extent. */
export function validateExtent(width, height) {
  if (!isFiniteNumber(width) || !isFiniteNumber(height)) {
    return fail("invalid_extent", "width and height must be finite numbers");
  }
  if (!(width > 0) || !(height > 0) || width > MAX_CARD_EXTENT || height > MAX_CARD_EXTENT) {
    return fail("invalid_extent", `width and height must be within (0, ${MAX_CARD_EXTENT}]`);
  }
  return { ok: true, value: { width, height } };
}

export function quatRotateVec3(quaternion, vector) {
  const axis = { x: quaternion.x, y: quaternion.y, z: quaternion.z };
  const t = {
    x: 2 * (axis.y * vector.z - axis.z * vector.y),
    y: 2 * (axis.z * vector.x - axis.x * vector.z),
    z: 2 * (axis.x * vector.y - axis.y * vector.x),
  };
  return {
    x: vector.x + quaternion.w * t.x + (axis.y * t.z - axis.z * t.y),
    y: vector.y + quaternion.w * t.y + (axis.z * t.x - axis.x * t.z),
    z: vector.z + quaternion.w * t.z + (axis.x * t.y - axis.y * t.x),
  };
}

export function quatConjugate(quaternion) {
  return { x: -quaternion.x, y: -quaternion.y, z: -quaternion.z, w: quaternion.w };
}

const dot = (a, b) => a.x * b.x + a.y * b.y + a.z * b.z;
const subtract = (a, b) => ({ x: a.x - b.x, y: a.y - b.y, z: a.z - b.z });
const add = (a, b) => ({ x: a.x + b.x, y: a.y + b.y, z: a.z + b.z });
const scaleVec = (a, k) => ({ x: a.x * k, y: a.y * k, z: a.z * k });

function normalize(vector) {
  const length = Math.hypot(vector.x, vector.y, vector.z);
  if (!(length > 0) || !Number.isFinite(length)) return null;
  return { x: vector.x / length, y: vector.y / length, z: vector.z / length };
}

/** Local -> world for a pose: `position + rotation * (scale ⊙ local)`. */
export function poseToWorld(pose, local) {
  const scaled = {
    x: local.x * pose.scale.x,
    y: local.y * pose.scale.y,
    z: local.z * pose.scale.z,
  };
  return add(pose.position, quatRotateVec3(pose.rotation, scaled));
}

/** World -> local; `null` for a degenerate scale. */
export function poseToLocal(pose, world) {
  if (
    Math.abs(pose.scale.x) < MIN_SCALE ||
    Math.abs(pose.scale.y) < MIN_SCALE ||
    Math.abs(pose.scale.z) < MIN_SCALE
  ) {
    return null;
  }
  const rotated = quatRotateVec3(quatConjugate(pose.rotation), subtract(world, pose.position));
  return {
    x: rotated.x / pose.scale.x,
    y: rotated.y / pose.scale.y,
    z: rotated.z / pose.scale.z,
  };
}

/** Unit normal of a card plane in world space. */
export function planeNormal(pose) {
  return quatRotateVec3(pose.rotation, { x: 0, y: 0, z: 1 });
}

/**
 * Ray through a viewport pixel. Mirrors `Camera::ray_for_screen`: the origin is
 * at the top-left with `y` growing downward, `+Y` is up in world space.
 */
export function screenPointToRay(camera, x, y) {
  const view = { ...WORKSPACE_CAMERA, ...camera };
  if (!isFiniteNumber(x) || !isFiniteNumber(y)) return null;
  if (!(view.viewport.width > 0) || !(view.viewport.height > 0)) return null;
  const tanHalf = Math.tan(view.fovY / 2);
  if (!Number.isFinite(tanHalf)) return null;
  const aspect = view.viewport.width / view.viewport.height;
  const ndcX = (x / view.viewport.width) * 2 - 1;
  const ndcY = 1 - (y / view.viewport.height) * 2;
  const forward = quatRotateVec3(view.rotation, { x: 0, y: 0, z: 1 });
  const right = quatRotateVec3(view.rotation, { x: 1, y: 0, z: 0 });
  const up = quatRotateVec3(view.rotation, { x: 0, y: 1, z: 0 });
  const direction = normalize(
    add(
      add(forward, scaleVec(right, ndcX * tanHalf * aspect)),
      scaleVec(up, ndcY * tanHalf),
    ),
  );
  if (!direction) return null;
  return { origin: { ...view.position }, direction };
}

/** Ray/plane intersection distance, or `null`. */
export function rayPlaneDistance(ray, planePoint, normal) {
  const denominator = dot(normal, ray.direction);
  if (Math.abs(denominator) < PARALLEL_EPSILON) return null;
  const t = dot(normal, subtract(planePoint, ray.origin)) / denominator;
  if (!Number.isFinite(t) || t < 0) return null;
  return t;
}

/** Ray/rectangle intersection in 3D (two-sided). */
export function rayCardIntersection(ray, card) {
  if (!ray || !card || card.active === false) return null;
  if (!(card.width > 0) || !(card.height > 0)) return null;
  const t = rayPlaneDistance(ray, card.pose.position, planeNormal(card.pose));
  if (t === null) return null;
  const point = add(ray.origin, scaleVec(ray.direction, t));
  const local = poseToLocal(card.pose, point);
  if (!local) return null;
  const halfWidth = card.width / 2;
  const halfHeight = card.height / 2;
  if (Math.abs(local.x) > halfWidth || Math.abs(local.y) > halfHeight) return null;
  return {
    cardId: card.id,
    distance: t,
    point,
    local,
    u: (local.x + halfWidth) / card.width,
    v: (local.y + halfHeight) / card.height,
  };
}

/** Nearest hit; coplanar ties go to the card higher in the focus stack. */
export function hitTest(cards, ray, focusOrder = []) {
  let best = null;
  for (const card of cards) {
    const hit = rayCardIntersection(ray, card);
    if (!hit) continue;
    if (!best) {
      best = hit;
      continue;
    }
    if (Math.abs(hit.distance - best.distance) <= TIE_EPSILON) {
      if (focusOrder.indexOf(hit.cardId) > focusOrder.indexOf(best.cardId)) best = hit;
      continue;
    }
    if (hit.distance < best.distance) best = hit;
  }
  return best;
}

/** Painter's order: farthest from the eye first. */
export function paintOrder(cards, eye) {
  return [...cards].sort(
    (a, b) =>
      Math.hypot(b.pose.position.x - eye.x, b.pose.position.y - eye.y, b.pose.position.z - eye.z) -
      Math.hypot(a.pose.position.x - eye.x, a.pose.position.y - eye.y, a.pose.position.z - eye.z),
  );
}

export function protocolHeader() {
  return { name: SPATIAL_PROTOCOL_NAME, version: SPATIAL_PROTOCOL_VERSION };
}

/**
 * Throws when a response does not speak this protocol version. The studio calls
 * this on every payload so a future kernel/host mismatch degrades loudly rather
 * than misplacing cards.
 */
export function assertProtocol(payload) {
  const protocol = payload?.protocol;
  if (protocol?.name !== SPATIAL_PROTOCOL_NAME) {
    throw new Error(`unexpected spatial protocol: ${protocol?.name ?? "missing"}`);
  }
  if (protocol.version !== SPATIAL_PROTOCOL_VERSION) {
    throw new Error(
      `unsupported spatial protocol version ${protocol.version} (expected ${SPATIAL_PROTOCOL_VERSION})`,
    );
  }
  return payload;
}

/**
 * Poses the studio should apply because the hypervisor owns them
 * (`source: "hypervisor"`). Cards the studio owns (`source: "studio"`) are
 * returned untouched so the carousel keeps animating them.
 */
export function managedPoses(snapshot) {
  if (!snapshot || !Array.isArray(snapshot.cards)) return [];
  return snapshot.cards
    .filter(
      (card) =>
        (card.source === SPATIAL_SOURCES.HYPERVISOR || card.source === SPATIAL_SOURCES.KERNEL) &&
        card.active !== false,
    )
    .map((card) => ({
      id: card.id,
      position: { ...card.pose.position },
      rotation: { ...card.pose.rotation },
      scale: { ...card.pose.scale },
    }));
}

/**
 * Kernel wire frame: the single line `World::write_json` prints when the shell
 * runs `wmjson`. `kernel-bridge.mjs` reads it off COM1 and hands it here.
 */
export const KERNEL_FRAME_PREFIX = "SPATIALv1 ";
export const KERNEL_FRAME_VERSION = 1;
/** Kernel ids are 1-based u32; studio ids are strings. */
export const kernelCardId = (id) => `card-${id}`;

/**
 * Parses one `SPATIALv1 {...}` line into the host's card shape.
 * Returns `{ ok: true, value }` or `{ ok: false, error: { code, message } }`;
 * malformed frames are rejected whole rather than partially applied.
 */
export function parseKernelFrame(line) {
  if (typeof line !== "string") return fail("invalid_kernel_frame", "kernel frame must be a string");
  const at = line.indexOf(KERNEL_FRAME_PREFIX);
  if (at === -1) return fail("invalid_kernel_frame", `expected a ${KERNEL_FRAME_PREFIX.trim()} frame`);

  let payload;
  try {
    payload = JSON.parse(line.slice(at + KERNEL_FRAME_PREFIX.length));
  } catch (error) {
    return fail("invalid_kernel_frame", `kernel frame is not JSON: ${error.message}`);
  }
  if (payload?.v !== KERNEL_FRAME_VERSION) {
    return fail(
      "unsupported_kernel_frame",
      `kernel frame version ${payload?.v ?? "missing"} (expected ${KERNEL_FRAME_VERSION})`,
    );
  }
  if (!Number.isInteger(payload.rev) || payload.rev < 0) {
    return fail("invalid_kernel_frame", "kernel frame needs a non-negative integer rev");
  }
  if (!Array.isArray(payload.cards)) return fail("invalid_kernel_frame", "kernel frame needs a cards array");

  const cards = [];
  for (const raw of payload.cards) {
    if (!Number.isInteger(raw?.id) || raw.id <= 0) {
      return fail("invalid_kernel_frame", "each kernel card needs a positive integer id");
    }
    const pose = validatePose({
      position: { x: raw.px, y: raw.py, z: raw.pz },
      rotation: { x: raw.qx, y: raw.qy, z: raw.qz, w: raw.qw },
      scale: { x: raw.sx, y: raw.sy, z: raw.sz },
    });
    if (!pose.ok) return pose;
    const extent = validateExtent(raw.w, raw.h);
    if (!extent.ok) return extent;
    cards.push({
      id: kernelCardId(raw.id),
      kernelId: raw.id,
      pose: pose.value,
      width: extent.value.width,
      height: extent.value.height,
      active: raw.active !== false,
      focused: raw.focused === true,
    });
  }

  const focusKernelId = Number.isInteger(payload.focus) && payload.focus > 0 ? payload.focus : null;
  return {
    ok: true,
    value: {
      version: KERNEL_FRAME_VERSION,
      revision: payload.rev,
      focus: focusKernelId === null ? null : kernelCardId(focusKernelId),
      focusKernelId,
      cards,
    },
  };
}

/** Small fetch client for the control plane. */
export function createSpatialClient({ baseUrl = "", fetchImpl = null } = {}) {
  const request = fetchImpl ?? (typeof fetch === "function" ? fetch : null);
  if (!request) throw new Error("no fetch implementation available");

  const call = async (method, route, body, query = "") => {
    const response = await request(`${baseUrl}${route}${query}`, {
      method,
      headers: body === undefined ? undefined : { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (response.status === 304) return { unchanged: true };
    const payload = await response.json().catch(() => null);
    if (!response.ok) {
      const error = new Error(payload?.error?.message ?? `spatial request failed (${response.status})`);
      error.code = payload?.error?.code;
      error.status = response.status;
      throw error;
    }
    return assertProtocol(payload);
  };

  return {
    protocol: protocolHeader(),
    state: ({ since } = {}) =>
      call("GET", "/api/spatial/state", undefined, since === undefined ? "" : `?since=${since}`),
    sync: (body) => call("POST", "/api/spatial/sync", body),
    setPose: (body) => call("POST", "/api/spatial/pose", body),
    focus: (id) => call("POST", "/api/spatial/focus", { id }),
    hitTest: (body) => call("POST", "/api/spatial/hit-test", body),
    pointer: (body) => call("POST", "/api/spatial/pointer", body),
  };
}
