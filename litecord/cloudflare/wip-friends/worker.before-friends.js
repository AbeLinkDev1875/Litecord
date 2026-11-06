// LiteCord relay for Cloudflare Workers.
//
// This is only a matchmaker. It tells each browser who else is in the same server
// and passes small messages between them (mostly WebRTC connection setup). Chat,
// files, voice and video go directly between the browsers. If two browsers can't
// connect directly, their messages are passed along here instead.
//
// It never stores anything, and it never sends anyone's IP address to anyone.
import { DurableObject } from "cloudflare:workers";

const MAX_PEERS = 50;
const MAX_MESSAGE = 1 << 20;  // 1 MB, Cloudflare's WebSocket message limit
const enc = new TextEncoder(), dec = new TextDecoder();

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/relay") {
      if (request.headers.get("Upgrade") !== "websocket") return new Response("Expected a WebSocket", { status: 426 });
      const room = (url.searchParams.get("room") || "lobby").slice(0, 32);
      return env.ROOMS.get(env.ROOMS.idFromName(room)).fetch(request);
    }
    return env.ASSETS.fetch(request);
  },
};

// One of these runs per LiteCord server name. It uses WebSocket hibernation, so it costs
// nothing while everyone is just sitting in the chat.
export class Room extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  async fetch(request) {
    const url = new URL(request.url);
    const id = url.searchParams.get("id") || "";
    const name = (url.searchParams.get("name") || "").trim().slice(0, 32);
    if (!/^[a-z0-9]{4,64}$/.test(id) || !name) return new Response("id and name required", { status: 400 });

    for (const old of this.ctx.getWebSockets(id)) this.drop(old, 4000, "replaced");  // same tab reconnecting
    if (this.live().length >= MAX_PEERS) return new Response("This server is full", { status: 429 });

    const [client, server] = Object.values(new WebSocketPair());
    this.ctx.acceptWebSocket(server, [id]);
    server.serializeAttachment({ id, name, joined: Date.now() });
    this.announce();
    return new Response(null, { status: 101, webSocket: client });
  }

  live(except) {
    return this.ctx.getWebSockets().filter(ws => ws !== except && !ws.deserializeAttachment()?.gone);
  }

  find(id) {
    return this.ctx.getWebSockets(id).find(ws => !ws.deserializeAttachment()?.gone);
  }

  // Everyone gets the list of who's here (names only). The one online longest is the host.
  announce(except) {
    const peers = this.live(except).map(ws => ws.deserializeAttachment())
      .sort((a, b) => a.joined - b.joined || (a.id < b.id ? -1 : 1));
    const msg = JSON.stringify({ type: "peers", peers: peers.map(({ id, name }) => ({ id, name })), leader: peers[0]?.id || null });
    for (const ws of this.live(except)) {
      try { ws.send(msg); } catch {}
    }
  }

  drop(ws, code, reason) {
    try { ws.serializeAttachment({ ...ws.deserializeAttachment(), gone: true }); } catch {}
    try { ws.close(code, reason); } catch {}
  }

  async webSocketMessage(ws, msg) {
    const me = ws.deserializeAttachment();
    if (!me || me.gone) return;
    if (typeof msg === "string") {
      // {"to": "<id>", "data": ...}  ->  {"type": "msg", "from": "<id>", "data": ...}
      if (msg.length > MAX_MESSAGE) return;
      let m;
      try { m = JSON.parse(msg); } catch { return; }
      const to = this.find(String(m?.to || ""));
      if (to && to !== ws) to.send(JSON.stringify({ type: "msg", from: me.id, data: m.data }));
      return;
    }
    // Binary: [to length][to id][payload]  ->  [from length][from id][payload]
    const u = new Uint8Array(msg);
    if (!u.length || u.length > MAX_MESSAGE) return;
    const n = u[0];
    const to = this.find(dec.decode(u.subarray(1, 1 + n)));
    if (!to || to === ws) return;
    const from = enc.encode(me.id), body = u.subarray(1 + n);
    const out = new Uint8Array(1 + from.length + body.length);
    out[0] = from.length;
    out.set(from, 1);
    out.set(body, 1 + from.length);
    to.send(out);
  }

  async webSocketClose(ws, code) {
    const was = ws.deserializeAttachment();
    this.drop(ws, code === 1005 || code === 1006 ? 1000 : code, "bye");
    if (!was?.gone) this.announce(ws);
  }

  async webSocketError(ws) {
    const was = ws.deserializeAttachment();
    this.drop(ws, 1011, "error");
    if (!was?.gone) this.announce(ws);
  }
}
