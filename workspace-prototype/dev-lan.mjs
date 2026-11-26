#!/usr/bin/env node
/**
 * One-command LAN mode: `npm run dev:lan`.
 *
 * Binds the dev server to 0.0.0.0, lets Vite accept the PC's LAN hostnames, and
 * prints the exact URL to open on a phone. Guest VNC, QMP and swtpm stay on
 * 127.0.0.1; only the web UI and guest API are reachable from the LAN.
 */
import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  DEFAULT_LAN_PORT,
  LAN_ACCESS_ENV,
  lanAddresses,
  lanHostAllowlist,
  lanUrls,
} from "./lan-access.mjs";

const workspaceDir = path.dirname(fileURLToPath(import.meta.url));
process.chdir(workspaceDir);

const port = Number(process.env.PORT || DEFAULT_LAN_PORT);
const addresses = lanAddresses();
const urls = lanUrls(port, addresses);

process.env[LAN_ACCESS_ENV] = "1";
process.env.__VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS = lanHostAllowlist(addresses).join(",");

console.log("Post Card OS · LAN mode");
console.log("  web UI + guest API: this local network only");
console.log("  guest VNC, QMP and swtpm: 127.0.0.1 only, never on the LAN");
if (urls.length === 0) {
  console.log("  no private LAN address found — connect Wi-Fi or Ethernet, then rerun");
} else {
  for (const url of urls) console.log(`  open on your phone: ${url}`);
}
console.log(`  firewall: allow TCP ${port} on this PC (Windows prompt or: sudo ufw allow ${port}/tcp)`);
console.log("  Ctrl+C stops the studio\n");

const child = spawn(
  process.execPath,
  [
    path.join(workspaceDir, "node_modules", "vite", "bin", "vite.js"),
    "--host",
    "0.0.0.0",
    "--port",
    String(port),
    "--strictPort",
  ],
  { cwd: workspaceDir, stdio: "inherit", env: process.env },
);

child.on("exit", (code) => process.exit(code ?? 0));
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    child.kill(signal);
  });
}
