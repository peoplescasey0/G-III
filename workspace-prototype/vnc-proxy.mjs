/**
 * Same-origin WebSocket proxy for QEMU's VNC display.
 *
 * QEMU keeps its VNC WebSocket on 127.0.0.1, so a phone on the LAN cannot dial
 * it directly. The browser asks the dev server for
 * `/api/guest/display/<cardId>`, the proxy looks up that running guest's
 * loopback port itself (the client never supplies a port), rewrites the upgrade
 * request and pipes the raw WebSocket both ways. Nothing outside loopback is
 * exposed and no arbitrary local port can be reached.
 */
import { createConnection } from "node:net";

const DISPLAY_PATH = /^\/api\/guest\/display\/([a-z0-9][a-z0-9_-]{0,63})$/;

export function displayCardIdFromUrl(requestUrl) {
  const pathname = new URL(requestUrl ?? "/", "http://127.0.0.1").pathname;
  const match = DISPLAY_PATH.exec(pathname);
  return match ? match[1] : undefined;
}

export function buildUpgradeRequest(request) {
  const lines = [`${request.method} / HTTP/1.1`];
  for (let index = 0; index < request.rawHeaders.length; index += 2) {
    const name = request.rawHeaders[index];
    if (/^host$/i.test(name)) continue;
    lines.push(`${name}: ${request.rawHeaders[index + 1]}`);
  }
  return `${lines.join("\r\n")}\r\n\r\n`;
}

export function attachGuestDisplayProxy(server, resolvePort, isAllowed = () => true) {
  if (!server) return () => {};
  const onUpgrade = (request, socket, head) => {
    const cardId = displayCardIdFromUrl(request.url);
    if (!cardId) return;
    if (!isAllowed(request.socket.remoteAddress)) {
      socket.destroy();
      return;
    }
    const port = resolvePort(cardId);
    if (!Number.isInteger(port) || port <= 0 || port > 65535) {
      socket.destroy();
      return;
    }
    const upstream = createConnection({ host: "127.0.0.1", port }, () => {
      upstream.write(buildUpgradeRequest(request));
      if (head?.length) upstream.write(head);
      upstream.pipe(socket);
      socket.pipe(upstream);
    });
    upstream.on("error", () => socket.destroy());
    socket.on("error", () => upstream.destroy());
    socket.on("close", () => upstream.destroy());
  };
  server.on("upgrade", onUpgrade);
  return () => server.off("upgrade", onUpgrade);
}
