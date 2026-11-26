import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";
import ts from "typescript";

async function load() {
  const source = await readFile(new URL("../src/guest-display-url.ts", import.meta.url), "utf8");
  const js = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  return import(`data:text/javascript,${encodeURIComponent(js)}`);
}

const { guestDisplaySocketUrl } = await load();

test("loopback pages still dial QEMU's own display port", () => {
  for (const hostname of ["localhost", "127.0.0.1", "127.10.0.2", "::1", ""]) {
    assert.equal(
      guestDisplaySocketUrl({ hostname, protocol: "http:", host: "localhost:5173", cardId: "linux", websocketPort: 5700 }),
      `ws://${hostname || "127.0.0.1"}:5700`,
      hostname,
    );
  }
  assert.equal(
    guestDisplaySocketUrl({ hostname: "localhost", protocol: "https:", host: "localhost:5173", cardId: "linux", websocketPort: 5701 }),
    "wss://localhost:5701",
  );
});

test("LAN clients use the same-origin display proxy, not the loopback port", () => {
  assert.equal(
    guestDisplaySocketUrl({
      hostname: "192.168.1.50",
      protocol: "http:",
      host: "192.168.1.50:5173",
      cardId: "linux",
      websocketPort: 5700,
    }),
    "ws://192.168.1.50:5173/api/guest/display/linux",
  );
  assert.equal(
    guestDisplaySocketUrl({
      hostname: "casey-pc.local",
      protocol: "https:",
      host: "casey-pc.local:8443",
      cardId: "kali-red",
      websocketPort: 5703,
    }),
    "wss://casey-pc.local:8443/api/guest/display/kali-red",
  );
  assert.match(
    guestDisplaySocketUrl({
      hostname: "10.0.0.7",
      protocol: "http:",
      host: "10.0.0.7:5173",
      cardId: "a b/c",
      websocketPort: 5700,
    }),
    /\/api\/guest\/display\/a%20b%2Fc$/,
  );
});
