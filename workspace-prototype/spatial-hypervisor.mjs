/**
 * Host-side spatial hypervisor for the workspace prototype.
 *
 * This is the host mirror of `kernel/src/spatial_wm.rs`: the same world model,
 * ray/plane hit testing, focus stack and z-order, exposed over the versioned
 * `/api/spatial/` control plane described in `spatial-protocol.mjs`.
 *
 * The studio owns the carousel poses it animates (`source: "studio"`). A pose
 * written through `/api/spatial/pose` or moved by `/api/spatial/pointer` is
 * owned by the hypervisor (`source: "hypervisor"`); the studio applies those.
 */

import {
  MAX_SPATIAL_CARDS,
  WORKSPACE_CAMERA,
  hitTest,
  kernelCardId,
  paintOrder,
  protocolHeader,
  rayPlaneDistance,
  screenPointToRay,
  validateExtent,
  validatePose,
} from "./spatial-protocol.mjs";

const MAX_BODY_BYTES = 64 * 1024;
const SOURCE_STUDIO = "studio";
const SOURCE_HYPERVISOR = "hypervisor";
const SOURCE_KERNEL = "kernel";

function isLoopback(address) {
  if (!address) return false;
  if (address.startsWith("127.")) return true;
  return address === "::1" || address === "::ffff:127.0.0.1";
}

function sendJson(response, status, body) {
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
  });
  response.end(status === 304 ? undefined : JSON.stringify(body));
}

async function readJson(request) {
  if (!request.headers["content-type"]?.includes("application/json")) {
    const error = new Error("Expected an application/json request.");
    error.status = 415;
    error.code = "unsupported_media_type";
    throw error;
  }
  let body = "";
  for await (const chunk of request) {
    body += chunk;
    if (body.length > MAX_BODY_BYTES) {
      const error = new Error("Request body is too large.");
      error.status = 413;
      error.code = "payload_too_large";
      throw error;
    }
  }
  try {
    return JSON.parse(body);
  } catch {
    const error = new Error("Request body must be valid JSON.");
    error.status = 400;
    error.code = "invalid_json";
    throw error;
  }
}

const clone = (value) => JSON.parse(JSON.stringify(value));

/**
 * @param {object} [options]
 * @param {object} [options.camera] Workspace camera override.
 * @param {object|null} [options.kernel] Optional `createKernelBridge()` handle.
 *   When present, the host can boot the real kernel over COM1 and adopt the
 *   poses it reports (`source: "kernel"`). Without it the hypervisor stays a
 *   pure mirror and every route behaves exactly as before.
 */
export function createSpatialHypervisor({ camera = WORKSPACE_CAMERA, kernel = null } = {}) {
  const world = {
    /** @type {Map<string, object>} */
    cards: new Map(),
    focusOrder: [],
    revision: 0,
    camera: { ...WORKSPACE_CAMERA, ...camera },
    grab: null,
    /** Kernel card id (`"1"`) -> studio card id (`"postcard-os"`). */
    kernelIds: {},
  };

  const bump = () => {
    world.revision += 1;
  };

  const focusedId = () => (world.focusOrder.length ? world.focusOrder[world.focusOrder.length - 1] : null);

  /** Unit normal of a card plane, mirroring `Pose::plane_normal` in the kernel. */
  const planeNormalOf = (card) => {
    const { x, y, z, w } = card.pose.rotation;
    return {
      x: 2 * (x * z + w * y),
      y: 2 * (y * z - w * x),
      z: 1 - 2 * (x * x + y * y),
    };
  };

  const pushFocus = (id) => {
    world.focusOrder = world.focusOrder.filter((entry) => entry !== id);
    if (world.focusOrder.length >= MAX_SPATIAL_CARDS) world.focusOrder.shift();
    world.focusOrder.push(id);
  };

  const refreshFocusFlags = () => {
    const top = focusedId();
    for (const card of world.cards.values()) card.focused = card.id === top;
  };

  const cards = () => [...world.cards.values()];

  const snapshot = () => {
    const ordered = paintOrder(cards(), world.camera.position);
    return {
      protocol: protocolHeader(),
      revision: world.revision,
      camera: clone(world.camera),
      focusedCardId: focusedId(),
      focusOrder: [...world.focusOrder],
      cards: ordered.map((card) => ({
        id: card.id,
        pose: clone(card.pose),
        width: card.width,
        height: card.height,
        active: card.active !== false,
        focused: card.focused === true,
        source: card.source,
      })),
    };
  };

  const ensureCard = (id, { width = 320, height = 200, source = SOURCE_STUDIO } = {}) => {
    const existing = world.cards.get(id);
    if (existing) return existing;
    if (world.cards.size >= MAX_SPATIAL_CARDS) {
      const error = new Error(`at most ${MAX_SPATIAL_CARDS} spatial cards are supported`);
      error.status = 409;
      error.code = "capacity_exhausted";
      throw error;
    }
    const card = {
      id,
      pose: {
        position: { ...WORKSPACE_CAMERA.position, z: 0 },
        rotation: { x: 0, y: 0, z: 0, w: 1 },
        scale: { x: 1, y: 1, z: 1 },
      },
      width,
      height,
      active: true,
      focused: false,
      source,
    };
    world.cards.set(id, card);
    pushFocus(id);
    refreshFocusFlags();
    bump();
    return card;
  };

  const setPoseInternal = (id, pose, source) => {
    const card = ensureCard(id);
    card.pose = clone(pose);
    card.source = source;
    bump();
    refreshFocusFlags();
    return card;
  };

  const focusCard = (id) => {
    if (!world.cards.has(id)) {
      const error = new Error(`unknown spatial card: ${id}`);
      error.status = 404;
      error.code = "unknown_card";
      throw error;
    }
    pushFocus(id);
    refreshFocusFlags();
    bump();
  };

  const applySync = (body) => {
    const incoming = Array.isArray(body?.cards) ? body.cards : [];
    for (const entry of incoming) {
      if (!entry || typeof entry.id !== "string" || entry.id.length === 0) {
        const error = new Error("each card needs a string id");
        error.status = 400;
        error.code = "invalid_card";
        throw error;
      }
      const pose = validatePose(entry.pose);
      if (!pose.ok) {
        const error = new Error(pose.error.message);
        error.status = 400;
        error.code = pose.error.code;
        throw error;
      }
      const existing = world.cards.get(entry.id);
      // A hypervisor- or kernel-owned card stays authoritative until the studio
      // claims it explicitly. Otherwise the studio's 1 Hz pose push would drag a
      // card the kernel is driving straight back to its carousel slot.
      if (existing && existing.source !== SOURCE_STUDIO) continue;
      const extent = validateExtent(
        entry.width ?? existing?.width ?? 320,
        entry.height ?? existing?.height ?? 200,
      );
      if (!extent.ok) {
        const error = new Error(extent.error.message);
        error.status = 400;
        error.code = extent.error.code;
        throw error;
      }
      const card = ensureCard(entry.id, extent.value);
      card.width = extent.value.width;
      card.height = extent.value.height;
      card.pose = clone(pose.value);
      card.source = SOURCE_STUDIO;
      bump();
    }
    if (typeof body?.focused === "string" && body.focused.length > 0 && !world.cards.has(body.focused)) {
      const error = new Error(`unknown spatial card: ${body.focused}`);
      error.status = 404;
      error.code = "unknown_card";
      throw error;
    }
    if (typeof body?.focused === "string" && body.focused.length > 0 && focusedId() !== body.focused) {
      pushFocus(body.focused);
      bump();
    }
    refreshFocusFlags();
    return snapshot();
  };

  const pointer = (body) => {
    const ray = screenPointToRay(world.camera, body?.x, body?.y);
    if (!ray) {
      const error = new Error("pointer x and y must be finite numbers");
      error.status = 400;
      error.code = "invalid_pointer";
      throw error;
    }
    const hit = hitTest(cards(), ray, world.focusOrder);
    if (body.pressed === true) {
      if (hit) {
        focusCard(hit.cardId);
        const card = world.cards.get(hit.cardId);
        world.grab = {
          cardId: hit.cardId,
          anchor: { ...hit.point },
          normal: planeNormalOf(card),
          startPosition: { ...card.pose.position },
        };
      } else {
        world.grab = null;
      }
    }
    if (body.dragged === true && world.grab) {
      const { anchor, normal, cardId, startPosition } = world.grab;
      const t = rayPlaneDistance(ray, anchor, normal);
      if (t !== null) {
        const destination = {
          x: ray.origin.x + ray.direction.x * t,
          y: ray.origin.y + ray.direction.y * t,
          z: ray.origin.z + ray.direction.z * t,
        };
        const card = world.cards.get(cardId);
        if (card) {
          card.pose = {
            ...card.pose,
            position: {
              x: startPosition.x + (destination.x - anchor.x),
              y: startPosition.y + (destination.y - anchor.y),
              z: startPosition.z + (destination.z - anchor.z),
            },
          };
          card.source = SOURCE_HYPERVISOR;
          bump();
        }
      }
    }
    if (body.pressed !== true && body.dragged !== true) world.grab = null;
    return { hit, snapshot: snapshot() };
  };

  const hitTestRequest = (body) => {
    if (body?.ray) {
      const origin = body.ray.origin;
      const direction = body.ray.direction;
      const length = Math.hypot(direction?.x ?? 0, direction?.y ?? 0, direction?.z ?? 0);
      if (!(length > 0) || ![origin?.x, origin?.y, origin?.z, direction?.x, direction?.y, direction?.z].every(Number.isFinite)) {
        const error = new Error("ray needs a finite origin and a non-zero direction");
        error.status = 400;
        error.code = "invalid_ray";
        throw error;
      }
      const ray = {
        origin: { x: origin.x, y: origin.y, z: origin.z },
        direction: { x: direction.x / length, y: direction.y / length, z: direction.z / length },
      };
      return { hit: hitTest(cards(), ray, world.focusOrder), snapshot: snapshot() };
    }
    const ray = screenPointToRay(world.camera, body?.screen?.x, body?.screen?.y);
    if (!ray) {
      const error = new Error("provide either a ray or a screen point");
      error.status = 400;
      error.code = "invalid_ray";
      throw error;
    }
    return { hit: hitTest(cards(), ray, world.focusOrder), snapshot: snapshot() };
  };

  /**
   * Adopts one `SPATIALv1` frame from the live kernel. Cards become
   * `source: "kernel"` so the studio applies the pose instead of the carousel.
   */
  /** Kernel id -> studio card id. Defaults to `card-<n>`. */
  const idForKernelCard = (kernelId) => world.kernelIds[String(kernelId)] ?? kernelCardId(kernelId);

  const applyKernelFrame = (frame) => {
    const reported = new Set();
    for (const entry of frame?.cards ?? []) {
      const cardId = idForKernelCard(entry.kernelId ?? entry.id);
      reported.add(cardId);
      const card = ensureCard(cardId, {
        width: entry.width,
        height: entry.height,
        source: SOURCE_KERNEL,
      });
      card.pose = clone(entry.pose);
      card.width = entry.width;
      card.height = entry.height;
      card.active = entry.active !== false;
      card.source = SOURCE_KERNEL;
      bump();
    }
    // A card the kernel no longer reports (destroyed there) must not stay pinned
    // to a stale pose: hand it back to the studio's carousel.
    for (const card of world.cards.values()) {
      if (card.source === SOURCE_KERNEL && !reported.has(card.id)) {
        card.source = SOURCE_STUDIO;
        card.active = true;
        bump();
      }
    }
    const focusId = idForKernelCard(frame?.focusKernelId ?? frame?.focus);
    if (frame?.focus && world.cards.has(focusId)) {
      pushFocus(focusId);
      bump();
    }
    refreshFocusFlags();
    return snapshot();
  };

  let kernelPullInFlight = false;

  /**
   * Best-effort background pull, driven by the studio's existing 1 Hz `sync`.
   * Never throws: a bridge that is stopped, booting or broken must not affect
   * the studio's own carousel behaviour.
   */
  const maybePullKernel = () => {
    if (!kernel || kernelPullInFlight) return;
    let bridgeStatus;
    try {
      bridgeStatus = kernel.status();
    } catch {
      return;
    }
    if (bridgeStatus.state !== "running") return;
    kernelPullInFlight = true;
    Promise.resolve()
      .then(() => kernel.requestSnapshot())
      .then((frame) => applyKernelFrame(frame))
      .catch(() => {
        // The kernel bridge is optional for the prototype studio.
      })
      .finally(() => {
        kernelPullInFlight = false;
      });
  };

  const kernelUnavailable = () =>
    Object.assign(new Error("No kernel bridge is attached to this spatial hypervisor."), {
      status: 501,
      code: "kernel_bridge_unavailable",
    });

  const route = async (request, response) => {
    const url = new URL(request.url ?? "/", "http://localhost");
    if (!url.pathname.startsWith("/api/spatial/")) return false;

    if (!isLoopback(request.socket?.remoteAddress)) {
      sendJson(response, 403, {
        protocol: protocolHeader(),
        error: { code: "forbidden", message: "Spatial control is available only from this computer." },
      });
      return true;
    }

    try {
      if (request.method === "GET" && url.pathname === "/api/spatial/state") {
        // A missing `since` is not revision 0: `Number(null)` would be 0 and a
        // fresh world (revision 0) would wrongly answer 304.
        const sinceParam = url.searchParams.get("since");
        const since =
          sinceParam === null || sinceParam.trim() === "" ? null : Number(sinceParam);
        if (since !== null && Number.isFinite(since) && since === world.revision) {
          sendJson(response, 304, null);
          return true;
        }
        sendJson(response, 200, snapshot());
        return true;
      }
      if (request.method === "POST" && url.pathname === "/api/spatial/sync") {
        maybePullKernel();
        sendJson(response, 200, applySync(await readJson(request)));
        return true;
      }
      if (request.method === "GET" && url.pathname === "/api/spatial/kernel") {
        if (!kernel) throw kernelUnavailable();
        sendJson(response, 200, { protocol: protocolHeader(), kernel: kernel.status() });
        return true;
      }
      if (url.pathname.startsWith("/api/spatial/kernel/")) {
        if (!kernel) throw kernelUnavailable();
        if (request.method !== "POST") {
          throw Object.assign(new Error("kernel control is POST only"), {
            status: 405,
            code: "method_not_allowed",
          });
        }
        const action = url.pathname.slice("/api/spatial/kernel/".length);
        if (action === "start") {
          sendJson(response, 200, { protocol: protocolHeader(), kernel: kernel.start() });
          return true;
        }
        if (action === "stop") {
          sendJson(response, 200, { protocol: protocolHeader(), kernel: kernel.stop() });
          return true;
        }
        if (action === "map") {
          const body = await readJson(request);
          const ids = body?.ids;
          if (!ids || typeof ids !== "object" || Array.isArray(ids)) {
            throw Object.assign(new Error("ids must be an object of kernel id -> studio card id"), {
              status: 400,
              code: "invalid_kernel_ids",
            });
          }
          for (const [kernelId, studioId] of Object.entries(ids)) {
            if (!/^[1-9][0-9]*$/.test(kernelId)) {
              throw Object.assign(new Error(`kernel id must be a positive integer: ${kernelId}`), {
                status: 400,
                code: "invalid_kernel_ids",
              });
            }
            if (typeof studioId !== "string" || studioId.length === 0 || studioId.length > 64) {
              throw Object.assign(new Error(`studio card id is invalid for kernel card ${kernelId}`), {
                status: 400,
                code: "invalid_card",
              });
            }
            world.kernelIds[kernelId] = studioId;
          }
          sendJson(response, 200, {
            protocol: protocolHeader(),
            kernel: kernel.status(),
            kernelIds: { ...world.kernelIds },
          });
          return true;
        }
        if (action === "pull") {
          const frame = await kernel.requestSnapshot();
          sendJson(response, 200, {
            protocol: protocolHeader(),
            kernel: kernel.status(),
            snapshot: applyKernelFrame(frame),
          });
          return true;
        }
        throw Object.assign(new Error(`no kernel action: ${action}`), {
          status: 404,
          code: "unknown_route",
        });
      }
      if (request.method === "POST" && url.pathname === "/api/spatial/pose") {
        const body = await readJson(request);
        if (typeof body?.id !== "string" || body.id.length === 0) {
          throw Object.assign(new Error("id must be a non-empty string"), {
            status: 400,
            code: "invalid_card",
          });
        }
        const pose = validatePose(body.pose);
        if (!pose.ok) throw Object.assign(new Error(pose.error.message), { status: 400, code: pose.error.code });
        const extent = validateExtent(
          body.width ?? world.cards.get(body.id)?.width ?? 320,
          body.height ?? world.cards.get(body.id)?.height ?? 200,
        );
        if (!extent.ok) throw Object.assign(new Error(extent.error.message), { status: 400, code: extent.error.code });
        const card = ensureCard(body.id, extent.value);
        card.width = extent.value.width;
        card.height = extent.value.height;
        // A pose written through the control plane belongs to the hypervisor, so
        // the studio applies it instead of animating over it.
        setPoseInternal(body.id, pose.value, SOURCE_HYPERVISOR);
        sendJson(response, 200, snapshot());
        return true;
      }
      if (request.method === "POST" && url.pathname === "/api/spatial/focus") {
        const body = await readJson(request);
        focusCard(body?.id);
        sendJson(response, 200, snapshot());
        return true;
      }
      if (request.method === "POST" && url.pathname === "/api/spatial/hit-test") {
        sendJson(response, 200, hitTestRequest(await readJson(request)));
        return true;
      }
      if (request.method === "POST" && url.pathname === "/api/spatial/pointer") {
        sendJson(response, 200, pointer(await readJson(request)));
        return true;
      }
      sendJson(response, 404, {
        protocol: protocolHeader(),
        error: { code: "unknown_route", message: `no spatial route for ${request.method} ${url.pathname}` },
      });
      return true;
    } catch (error) {
      sendJson(response, error.status ?? 500, {
        protocol: protocolHeader(),
        error: { code: error.code ?? "internal_error", message: error.message },
      });
      return true;
    }
  };

  return {
    route,
    snapshot,
    applySync,
    applyKernelFrame,
    setPose: setPoseInternal,
    focus: focusCard,
    hitTest: hitTestRequest,
    pointer,
    camera: () => clone(world.camera),
    revision: () => world.revision,
    reset: () => {
      world.cards.clear();
      world.focusOrder = [];
      world.grab = null;
      world.revision += 1;
    },
  };
}
