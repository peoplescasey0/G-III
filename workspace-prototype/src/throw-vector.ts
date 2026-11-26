/**
 * Pointer-flick vectors for the Desk's paper-plane/card throw.
 *
 * A mouse or touch pointer has no pressure field, so "force" is inferred from
 * how the object moves: the last stretch of pointer travel gives a magnitude
 * and a direction, and that pair is the throw vector. Keeping the arithmetic
 * here (no three.js) lets the studio and its tests agree on one definition.
 */

export type PointerSample = { x: number; y: number; t: number };
export type ThrowVector = {
  /** Unit screen direction of the flick; +y points down, like clientY. */
  dx: number;
  dy: number;
  /** Pointer speed at release, in pixels per second. */
  speed: number;
  /** 0..1 force, mapped from speed between the throw thresholds. */
  power: number;
};
export type Point3 = { x: number; y: number; z: number };
export type ScreenPoint = { x: number; y: number };
export type ScreenDirection = { dx: number; dy: number };
export type ThrowCandidate<T> = ScreenPoint & { item: T };

/** Below this release speed the pointer let go of the card, it did not throw it. */
export const THROW_MIN_SPEED = 620;
/** At or above this speed the throw is at full force. */
export const THROW_MAX_SPEED = 3200;
/** Only the final stretch of the gesture counts as the flick. */
export const THROW_SAMPLE_WINDOW_MS = 120;
/** Releasing after holding still is a drop, not a throw. */
export const THROW_RELEASE_GRACE_MS = 160;

export function computeThrowVector(
  samples: readonly PointerSample[],
  now: number,
): ThrowVector | undefined {
  if (samples.length < 2) return undefined;
  const last = samples[samples.length - 1];
  if (now - last.t > THROW_RELEASE_GRACE_MS) return undefined;

  let firstIndex = samples.length - 1;
  for (let index = samples.length - 1; index >= 0; index -= 1) {
    if (last.t - samples[index].t > THROW_SAMPLE_WINDOW_MS) break;
    firstIndex = index;
  }
  // Sparse input can leave only the final sample inside the window; the last
  // two samples are still the best measure of the flick.
  if (firstIndex === samples.length - 1) firstIndex = samples.length - 2;
  // High-rate or coalesced pointer input can deliver the whole flick inside a
  // millisecond. Widen to earlier samples so the measured span stays meaningful
  // instead of dividing a long travel by an almost-zero duration.
  while (firstIndex > 0 && last.t - samples[firstIndex].t < 4) {
    firstIndex -= 1;
  }
  const first = samples[firstIndex];
  const seconds = (last.t - first.t) / 1000;
  // A sub-millisecond span is still a real burst on high-rate input; only a
  // zero-length span means there is nothing to measure.
  if (seconds <= 0.0005) return undefined;

  const dx = last.x - first.x;
  const dy = last.y - first.y;
  const travel = Math.hypot(dx, dy);
  if (travel < 0.001) return undefined;

  const speed = travel / seconds;
  if (speed < THROW_MIN_SPEED) return undefined;

  return {
    dx: dx / travel,
    dy: dy / travel,
    speed,
    power: Math.min(1, (speed - THROW_MIN_SPEED) / (THROW_MAX_SPEED - THROW_MIN_SPEED)),
  };
}

/** World distance a throw covers: a soft flick is a nudge, a hard one crosses the room. */
export function throwDistanceForPower(power: number): number {
  return 2.4 + clamp01(power) * 6.2;
}

/** Peel of the arc above the straight line; harder throws fly higher. */
export function throwArcHeightForPower(power: number): number {
  return 0.5 + clamp01(power) * 1.5;
}

export function throwDurationForPower(power: number): number {
  return 520 + clamp01(power) * 420;
}

/** Ballistic-looking arc: eased in x/z, lifted in y, both ends exact. */
export function throwTrajectoryPoint(
  from: Point3,
  to: Point3,
  progress: number,
  arcHeight: number,
): Point3 {
  const t = clamp01(progress);
  const eased = t * t * (3 - 2 * t);
  return {
    x: from.x + (to.x - from.x) * eased,
    y: from.y + (to.y - from.y) * eased + Math.sin(t * Math.PI) * arcHeight,
    z: from.z + (to.z - from.z) * eased,
  };
}

/**
 * Recipient magnetism. When the aim ray misses every card, the flick still
 * has a direction: pick the nearest card inside the radius that lies ahead of
 * it, scoring distance against how well it lines up. Cards behind the flick
 * are never candidates, so a throw cannot land backwards.
 */
export function pickThrowTarget<T>(
  candidates: readonly ThrowCandidate<T>[],
  pointer: ScreenPoint,
  direction: ScreenDirection,
  options: { radius?: number; minCosine?: number } = {},
): T | undefined {
  const radius = options.radius ?? 260;
  const minCosine = options.minCosine ?? 0.25;
  const length = Math.hypot(direction.dx, direction.dy);
  if (length < 0.001) return undefined;
  const dx = direction.dx / length;
  const dy = direction.dy / length;

  let best: T | undefined;
  let bestScore = Infinity;
  for (const candidate of candidates) {
    const offsetX = candidate.x - pointer.x;
    const offsetY = candidate.y - pointer.y;
    const distance = Math.hypot(offsetX, offsetY);
    if (distance > radius) continue;
    const cosine = distance < 1 ? 1 : (offsetX * dx + offsetY * dy) / distance;
    if (cosine < minCosine) continue;
    const score = distance - cosine * radius * 0.45;
    if (score < bestScore) {
      bestScore = score;
      best = candidate.item;
    }
  }
  return best;
}

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(1, Math.max(0, value));
}
