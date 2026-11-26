import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import ts from "typescript";

async function load() {
  const source = await readFile(new URL("../src/throw-vector.ts", import.meta.url), "utf8");
  const js = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  return import(`data:text/javascript,${encodeURIComponent(js)}`);
}

const {
  THROW_MAX_SPEED,
  THROW_MIN_SPEED,
  computeThrowVector,
  pickThrowTarget,
  throwArcHeightForPower,
  throwDistanceForPower,
  throwDurationForPower,
  throwTrajectoryPoint,
} = await load();

function flick(dxPerSecond, dyPerSecond, startT = 1000) {
  // Three samples 16 ms apart, moving at the requested screen speed.
  return [0, 16, 32].map((offset) => ({
    x: (dxPerSecond * offset) / 1000,
    y: (dyPerSecond * offset) / 1000,
    t: startT + offset,
  }));
}

test("a fast release is a throw, with direction and 0..1 force", () => {
  const samples = flick(2400, -600);
  const vector = computeThrowVector(samples, 1032);
  assert.ok(vector);
  assert.ok(vector.speed > THROW_MIN_SPEED);
  assert.ok(vector.power > 0 && vector.power < 1);
  assert.ok(Math.abs(Math.hypot(vector.dx, vector.dy) - 1) < 1e-9);
  assert.ok(vector.dx > 0);
  assert.ok(vector.dy < 0);

  const full = computeThrowVector(flick(THROW_MAX_SPEED + 2000, 0), 1032);
  assert.equal(full.power, 1);
});

test("a slow drag, a hold before release, and a still pointer are not throws", () => {
  assert.equal(computeThrowVector(flick(120, 40), 1032), undefined);
  const samples = flick(2400, 0);
  assert.equal(computeThrowVector(samples, 1032 + 400), undefined, "paused before release");
  assert.equal(
    computeThrowVector([
      { x: 10, y: 10, t: 1000 },
      { x: 10, y: 10, t: 1016 },
    ], 1016),
    undefined,
  );
  assert.equal(computeThrowVector([{ x: 0, y: 0, t: 1000 }], 1000), undefined);
});

test("only the final stretch of pointer travel is measured", () => {
  // A long slow glide followed by a fast snap: the snap is the flick.
  const samples = [
    { x: 0, y: 0, t: 200 },
    { x: 40, y: 0, t: 400 },
    { x: 60, y: 0, t: 800 },
    { x: 160, y: 0, t: 816 },
    { x: 260, y: 0, t: 832 },
  ];
  const vector = computeThrowVector(samples, 832);
  assert.ok(vector);
  assert.ok(vector.speed > 1000, `expected the snap speed, got ${vector.speed}`);
});

test("a coalesced burst of pointer events still yields a throw", () => {
  const vector = computeThrowVector(
    [
      { x: 0, y: 0, t: 1000 },
      { x: 40, y: 4, t: 1000.4 },
      { x: 96, y: 8, t: 1000.8 },
    ],
    1000.8,
  );
  assert.ok(vector, "a sub-millisecond flick must not divide by a zero span");
  assert.ok(vector.speed > 0);
  assert.ok(Math.abs(Math.hypot(vector.dx, vector.dy) - 1) < 1e-9);
});

test("force maps monotonically onto distance, arc height and flight time", () => {
  const soft = throwDistanceForPower(0);
  const hard = throwDistanceForPower(1);
  assert.ok(hard > soft * 2);
  assert.ok(throwArcHeightForPower(0.9) > throwArcHeightForPower(0.1));
  assert.ok(throwDurationForPower(0.9) > throwDurationForPower(0.1));
  // Clamped so an out-of-range power can never build an absurd throw.
  assert.equal(throwDistanceForPower(-4), throwDistanceForPower(0));
  assert.equal(throwDistanceForPower(9), throwDistanceForPower(1));
});

test("the arc starts and ends exactly, and lifts above the straight line in between", () => {
  const from = { x: -2, y: 2.6, z: 2 };
  const to = { x: 3, y: 3.2, z: -3 };
  assert.deepEqual(throwTrajectoryPoint(from, to, 0, 1.2), from);
  assert.deepEqual(throwTrajectoryPoint(from, to, 1, 1.2), to);
  const middle = throwTrajectoryPoint(from, to, 0.5, 1.2);
  const straightY = (from.y + to.y) / 2;
  assert.ok(middle.y > straightY + 1, "the plane should arc over the room");
  assert.ok(middle.x > from.x && middle.x < to.x);
  assert.ok(middle.z < from.z && middle.z > to.z);
});

test("recipient magnetism picks the card ahead of the flick and ignores the ones behind", () => {
  const candidates = [
    { item: "behind", x: 100, y: 400 },
    { item: "ahead-far", x: 700, y: 300 },
    { item: "ahead-near", x: 480, y: 340 },
    { item: "out-of-range", x: 2000, y: 0 },
  ];
  const picked = pickThrowTarget(candidates, { x: 400, y: 400 }, { dx: 1, dy: 0 });
  assert.equal(picked, "ahead-near");

  assert.equal(
    pickThrowTarget(candidates, { x: 400, y: 400 }, { dx: -1, dy: 0 }, { radius: 400 }),
    "behind",
  );
  assert.equal(pickThrowTarget([], { x: 0, y: 0 }, { dx: 1, dy: 0 }), undefined);
  assert.equal(
    pickThrowTarget(candidates, { x: 400, y: 400 }, { dx: 0, dy: 0 }),
    undefined,
    "a still pointer has no direction to magnetise along",
  );
});
