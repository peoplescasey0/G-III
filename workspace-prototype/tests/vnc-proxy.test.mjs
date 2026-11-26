import assert from "node:assert/strict";
import test from "node:test";
import { createServer as createHttpServer } from "node:http";
import { createConnection, createServer as createNetServer } from "node:net";
import {
  attachGuestDisplayProxy,
  buildUpgradeRequest,
  displayCardIdFromUrl,
} from "../vnc-proxy.mjs";

const UPGRADE =
  "Upgrade: websocket\r\nConnection: Upgrade\r\n" +
  "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";

function listen(server) {
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
}

function trackSockets(server) {
  const sockets = new Set();
  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
  });
  return () => {
    for (const socket of sockets) socket.destroy();
  };
}

function closeServer(server, destroy) {
  destroy();
  return new Promise((resolve) => server.close(() => resolve()));
}

test("only exact card display paths are proxied", () => {
  assert.equal(displayCardIdFromUrl("/api/guest/display/linux"), "linux");
  assert.equal(displayCardIdFromUrl("/api/guest/display/kali-red?x=1"), "kali-red");
  assert.equal(displayCardIdFromUrl("/api/guest/display/..%2Fetc"), undefined);
  assert.equal(displayCardIdFromUrl("/api/guest/display/"), undefined);
  assert.equal(displayCardIdFromUrl("/api/guest/status"), undefined);
  assert.equal(displayCardIdFromUrl("/api/guest/display/linux/extra"), undefined);
});

test("the forwarded upgrade request keeps WebSocket headers but rewrites host and path", () => {
  const request = {
    method: "GET",
    rawHeaders: [
      "Host", "192.168.1.50:5173",
      "Upgrade", "websocket",
      "Connection", "Upgrade",
      "Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==",
      "Sec-WebSocket-Version", "13",
    ],
  };
  const forwarded = buildUpgradeRequest(request);
  assert.match(forwarded, /^GET \/ HTTP\/1\.1\r\n/);
  assert.doesNotMatch(forwarded, /Host:/i);
  assert.match(forwarded, /Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==/);
  assert.match(forwarded, /\r\n\r\n$/);
});

test("a WebSocket upgrade is piped to the guest's loopback port and back", { timeout: 5000 }, async () => {
  let upstreamRequestLine = "";
  const upstream = createNetServer((socket) => {
    socket.once("data", (chunk) => {
      upstreamRequestLine = chunk.toString("utf8").split("\r\n")[0];
      socket.write("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n");
      socket.on("data", (payload) => socket.write(payload));
    });
  });
  const upstreamSockets = trackSockets(upstream);
  const upstreamPort = await listen(upstream);

  const http = createHttpServer((_req, res) => res.end("ok"));
  const httpSockets = trackSockets(http);
  attachGuestDisplayProxy(http, (cardId) => (cardId === "linux" ? upstreamPort : undefined), () => true);
  const proxyPort = await listen(http);

  const client = createConnection({ host: "127.0.0.1", port: proxyPort });
  const received = [];
  client.on("data", (chunk) => received.push(chunk.toString("utf8")));
  await new Promise((resolve) => client.on("connect", resolve));
  client.write(`GET /api/guest/display/linux HTTP/1.1\r\nHost: 127.0.0.1:${proxyPort}\r\n${UPGRADE}`);
  await new Promise((resolve) => setTimeout(resolve, 150));
  assert.match(received.join(""), /101 Switching Protocols/);
  assert.equal(upstreamRequestLine, "GET / HTTP/1.1");

  client.write("ping-frame");
  await new Promise((resolve) => setTimeout(resolve, 150));
  assert.match(received.join(""), /ping-frame/);

  client.destroy();
  await closeServer(http, httpSockets);
  await closeServer(upstream, upstreamSockets);
});

test("an unknown card id gets no upgrade", { timeout: 5000 }, async () => {
  const upstream = createNetServer(() => {});
  const upstreamSockets = trackSockets(upstream);
  const upstreamPort = await listen(upstream);
  const http = createHttpServer((_req, res) => res.end("ok"));
  const httpSockets = trackSockets(http);
  attachGuestDisplayProxy(
    http,
    (cardId) => (cardId === "linux" ? upstreamPort : undefined),
    () => true,
  );
  const proxyPort = await listen(http);

  const answered = await new Promise((resolve) => {
    const socket = createConnection({ host: "127.0.0.1", port: proxyPort });
    let gotData = false;
    socket.on("data", () => {
      gotData = true;
    });
    socket.on("close", () => resolve(gotData));
    socket.on("connect", () => {
      socket.write(`GET /api/guest/display/ghost-card HTTP/1.1\r\nHost: 127.0.0.1:${proxyPort}\r\n${UPGRADE}`);
      setTimeout(() => socket.destroy(), 200);
    });
  });
  assert.equal(answered, false);

  await closeServer(http, httpSockets);
  await closeServer(upstream, upstreamSockets);
});
