import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import ts from "typescript";

async function load(relative) {
  const source = await readFile(new URL(relative, import.meta.url), "utf8");
  const js = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  return import(`data:text/javascript,${encodeURIComponent(js)}`);
}

const { PLANE_NOTE_LIMIT, carriedNote, foldPose } = await load("../src/paper-plane.ts");
const mainTs = await readFile(new URL("../src/main.ts", import.meta.url), "utf8");

test("the sheet starts flat and reads as a folded dart by the end", () => {
  const flat = foldPose(0);
  const folded = foldPose(1);
  assert.equal(flat.dihedral, 0);
  assert.ok(flat.keelDrop > 0.4);
  assert.ok(flat.sheetScale > 1.2);
  assert.ok(folded.dihedral > 0.3);
  assert.equal(folded.keelDrop, 0);
  assert.equal(folded.sheetScale, 1);
  assert.ok(folded.dihedral > flat.dihedral);
  assert.ok(folded.keelDrop < flat.keelDrop);
  assert.ok(folded.sheetScale < flat.sheetScale);
});

test("the fold is monotonic and clamped, so the shape never pops backwards", () => {
  let previous = foldPose(0);
  for (let step = 1; step <= 20; step += 1) {
    const pose = foldPose(step / 20);
    assert.ok(pose.dihedral >= previous.dihedral);
    assert.ok(pose.keelDrop <= previous.keelDrop);
    assert.ok(pose.sheetScale <= previous.sheetScale);
    previous = pose;
  }
  assert.deepEqual(foldPose(-3), foldPose(0));
  assert.deepEqual(foldPose(9), foldPose(1));
  assert.deepEqual(foldPose(Number.NaN), foldPose(0));
});

test("a composed note rides with the plane, trimmed and within the API limit", () => {
  assert.equal(carriedNote("  meet me at the dock  ", "Linux", "Windows"), "meet me at the dock");
  assert.equal(carriedNote("line one\n\n line two", "Linux", "Windows"), "line one line two");
  const long = carriedNote("x".repeat(400), "Linux", "Windows");
  assert.equal(long.length, PLANE_NOTE_LIMIT);
});

test("without a draft the plane still carries an honest note, never an empty one", () => {
  const named = carriedNote("", "Spatial OS", "Pocket Lab");
  assert.match(named, /Spatial OS/);
  assert.match(named, /Pocket Lab/);
  assert.ok(named.length > 0 && named.length <= PLANE_NOTE_LIMIT);

  const room = carriedNote("   ", "Spatial OS", "  ");
  assert.match(room, /Spatial OS/);
  assert.ok(room.length > 0 && room.length <= PLANE_NOTE_LIMIT);
});

test("the plane, not the card, is the projectile and it carries the note", () => {
  const start = mainTs.indexOf("function beginPlaneThrow");
  const end = mainTs.indexOf("function spawnThrowBloom", start);
  assert.notEqual(start, -1);
  assert.notEqual(end, -1);
  const throwBody = mainTs.slice(start, end);

  assert.match(throwBody, /buildPaperPlane\(\)/);
  assert.match(throwBody, /scene\.add\(plane\.group\)/);
  assert.match(throwBody, /carriedNote\(/);
  assert.doesNotMatch(throwBody, /card\.group\.position\.copy/);
  assert.doesNotMatch(throwBody, /card\.group\.scale\.copy/);

  const finishStart = mainTs.indexOf("function finishPlaneThrow");
  const finishEnd = mainTs.indexOf("function updatePlaneFlight", finishStart);
  const finishBody = mainTs.slice(finishStart, finishEnd);
  assert.match(finishBody, /deliverPaperPlaneToBackend\(flight\.fromCard, target, flight\.note\)/);
  assert.match(finishBody, /flipCardTo\(target, "right"\)/);
  assert.match(finishBody, /spawnThrowBloom/);
});

test("the ffe5993 throw physics and targeting are still the ones flying", () => {
  for (const helper of [
    "computeThrowVector(",
    "throwDistanceForPower(",
    "throwArcHeightForPower(",
    "throwDurationForPower(",
    "throwTrajectoryPoint(",
    "pickThrowTarget(",
    "THROW_CONTACT_RADIUS",
  ]) {
    assert.ok(mainTs.includes(helper), `missing ${helper}`);
  }
  assert.match(mainTs, /function updatePlaneFlight/);
  assert.doesNotMatch(mainTs, /beginCardThrow/);
  assert.doesNotMatch(mainTs, /let throwFlight/);
});
