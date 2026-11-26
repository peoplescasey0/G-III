/**
 * LAN mode rules for the Desk.
 *
 * The web UI and the guest API may be opened to the local network for a phone
 * in another room. QEMU's VNC, QMP and swtpm ports must never leave loopback,
 * so "LAN access" means private addresses only, and only when the LAN script
 * turned it on explicitly.
 */
import { networkInterfaces } from "node:os";
import { hostname as osHostname } from "node:os";

export const LAN_ACCESS_ENV = "POSTCARD_LAN_ACCESS";
/** Vite's default dev port; `PORT` can override it. */
export const DEFAULT_LAN_PORT = 5173;

function ipv4(address) {
  if (!address) return undefined;
  if (address.startsWith("::ffff:")) address = address.slice(7);
  const parts = address.split(".");
  if (parts.length !== 4) return undefined;
  if (!parts.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255)) return undefined;
  return parts.map(Number);
}

export function isLoopbackAddress(address) {
  if (!address) return false;
  const v4 = ipv4(address);
  if (v4) return v4[0] === 127;
  return address === "::1" || address === "[::1]";
}

/** RFC 1918 private space plus link-local; anything routable is refused. */
export function isPrivateLanAddress(address) {
  if (!address) return false;
  const v4 = ipv4(address);
  if (v4) {
    const [a, b] = v4;
    return (
      a === 10 ||
      (a === 172 && b >= 16 && b <= 31) ||
      (a === 192 && b === 168) ||
      (a === 169 && b === 254)
    );
  }
  const value = address.toLowerCase();
  if (value.startsWith("[")) return value.startsWith("[fc") || value.startsWith("[fd") || value.startsWith("[fe80:");
  return value.startsWith("fc") || value.startsWith("fd") || value.startsWith("fe80:");
}

/**
 * One rule for every guest route: loopback always works, private LAN clients
 * only when `POSTCARD_LAN_ACCESS=1` (set by `npm run dev:lan`), never public.
 */
export function isLanClientAllowed(address, enabled = process.env[LAN_ACCESS_ENV] === "1") {
  return isLoopbackAddress(address) || (enabled && isPrivateLanAddress(address));
}

/** Non-internal IPv4 addresses the Desk can be reached on. */
export function lanAddresses(interfaces = networkInterfaces()) {
  const addresses = new Set();
  for (const entries of Object.values(interfaces ?? {})) {
    for (const entry of entries ?? []) {
      if (!entry || entry.internal) continue;
      if (entry.family !== "IPv4" && entry.family !== 4) continue;
      if (!isPrivateLanAddress(entry.address)) continue;
      addresses.add(entry.address);
    }
  }
  return [...addresses].sort();
}

export function lanUrls(port, addresses = lanAddresses()) {
  return addresses.map((address) => `http://${address}:${port}`);
}

/** Hostnames Vite must accept so the LAN IP and mDNS name are not blocked. */
export function lanHostAllowlist(addresses = lanAddresses(), hostname = osHostname()) {
  const hosts = [...addresses, hostname, hostname ? `${hostname}.local` : "", "localhost", "127.0.0.1"];
  return [...new Set(hosts.filter(Boolean))];
}
