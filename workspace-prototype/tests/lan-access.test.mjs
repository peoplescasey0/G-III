import assert from "node:assert/strict";
import test from "node:test";
import {
  isLanClientAllowed,
  isLoopbackAddress,
  isPrivateLanAddress,
  lanAddresses,
  lanHostAllowlist,
  lanUrls,
} from "../lan-access.mjs";

test("loopback is recognised in every form the dev server sees", () => {
  for (const address of ["127.0.0.1", "127.10.0.4", "::1", "::ffff:127.0.0.1", "[::1]"]) {
    assert.equal(isLoopbackAddress(address), true, address);
  }
  for (const address of ["192.168.1.20", "10.0.0.5", "8.8.8.8", "", undefined]) {
    assert.equal(isLoopbackAddress(address), false, String(address));
  }
});

test("only RFC 1918 and link-local addresses count as LAN", () => {
  for (const address of ["10.0.0.5", "10.255.255.254", "172.16.0.1", "172.31.255.1", "192.168.1.50", "169.254.10.2", "fd00::1", "fe80::1"]) {
    assert.equal(isPrivateLanAddress(address), true, address);
  }
  for (const address of ["172.32.0.1", "8.8.8.8", "100.64.0.1", "192.169.0.1", "::1", "", undefined, "example.com"]) {
    assert.equal(isPrivateLanAddress(address), false, String(address));
  }
});

test("LAN access is opt-in and never covers public addresses", () => {
  assert.equal(isLanClientAllowed("127.0.0.1", false), true);
  assert.equal(isLanClientAllowed("::1", false), true);
  assert.equal(isLanClientAllowed("192.168.1.50", false), false, "default stays loopback-only");
  assert.equal(isLanClientAllowed("192.168.1.50", true), true);
  assert.equal(isLanClientAllowed("10.1.2.3", true), true);
  assert.equal(isLanClientAllowed("8.8.8.8", true), false, "a routable client is never allowed");
  assert.equal(isLanClientAllowed(undefined, true), false);
});

test("LAN addresses skip internal and public interfaces", () => {
  const interfaces = {
    lo: [{ address: "127.0.0.1", family: "IPv4", internal: true }],
    eth0: [
      { address: "192.168.1.50", family: "IPv4", internal: false },
      { address: "fe80::1", family: "IPv6", internal: false },
    ],
    wlan0: [{ address: "10.0.0.7", family: 4, internal: false }],
    wan: [{ address: "203.0.113.9", family: "IPv4", internal: false }],
  };
  assert.deepEqual(lanAddresses(interfaces), ["10.0.0.7", "192.168.1.50"]);
});

test("printed URLs use the real LAN address, never 0.0.0.0 or localhost", () => {
  const urls = lanUrls(5173, ["192.168.1.50", "10.0.0.7"]);
  assert.deepEqual(urls, ["http://192.168.1.50:5173", "http://10.0.0.7:5173"]);
  for (const url of urls) {
    assert.doesNotMatch(url, /0\.0\.0\.0|localhost|127\.0\.0\.1/);
  }
  assert.deepEqual(lanUrls(5173, []), []);
});

test("the Vite allowlist covers the IP, the hostname and mDNS, without duplicates", () => {
  const hosts = lanHostAllowlist(["192.168.1.50"], "casey-pc");
  assert.deepEqual([...hosts].sort(), [
    "127.0.0.1",
    "192.168.1.50",
    "casey-pc",
    "casey-pc.local",
    "localhost",
  ].sort());
  assert.equal(new Set(hosts).size, hosts.length);
  assert.deepEqual(lanHostAllowlist(["192.168.1.50"], ""), [
    "192.168.1.50",
    "localhost",
    "127.0.0.1",
  ]);
});
