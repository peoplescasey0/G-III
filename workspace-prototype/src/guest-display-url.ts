/**
 * Where the browser should open the guest's VNC WebSocket.
 *
 * QEMU keeps VNC on 127.0.0.1, so loopback pages dial the port directly. A LAN
 * client (phone in another room) cannot; it asks the same dev-server origin for
 * `/api/guest/display/<cardId>`, which proxies to the loopback port.
 */
export type GuestDisplayLocation = {
  hostname: string;
  protocol: string;
  host: string;
  cardId: string;
  websocketPort: number;
};

export function guestDisplaySocketUrl(location: GuestDisplayLocation): string {
  const protocol = location.protocol === "https:" ? "wss:" : "ws:";
  const hostname = location.hostname || "127.0.0.1";
  const loopback =
    hostname === "localhost" ||
    hostname === "::1" ||
    hostname === "[::1]" ||
    hostname.startsWith("127.");
  if (loopback) return `${protocol}//${hostname}:${location.websocketPort}`;
  return `${protocol}//${location.host}/api/guest/display/${encodeURIComponent(location.cardId)}`;
}
