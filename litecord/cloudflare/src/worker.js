// LiteCord relay for Cloudflare Workers.
//
// This is only a matchmaker. It tells each browser who else is in the same server
// and passes small messages between them (mostly WebRTC connection setup). Chat,
// files, voice and video go directly between the browsers. If two browsers can't
// connect directly, their messages are passed along here instead.
//
// It never sends anyone's IP address to anyone. The relay part stores nothing.
//
// The Inbox part is a small mailbox per person, for friends and direct messages: it holds
// messages for friends who are offline (up to 30 days, deleted as soon as they're picked up)
// and tells friends who's online.
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
    if (url.pathname === "/inbox") {
      if (request.headers.get("Upgrade") !== "websocket") return new Response("Expected a WebSocket", { status: 426 });
      const uid = url.searchParams.get("uid") || "";
      if (!UID.test(uid)) return new Response("bad id", { status: 400 });
      return inbox(env, uid).fetch(request);
    }
    if (url.pathname === "/api/username" && request.method === "POST") return setUsername(request, env);
    if (url.pathname === "/api/lookup") {
      const name = (url.searchParams.get("name") || "").toLowerCase();
      if (!UNAME.test(name)) return Response.json({ uid: null });
      return Response.json(await call(nameBox(env, name), "/name/get"));
    }
    return env.ASSETS.fetch(request);
  },
};

/* ---------- friends and direct messages ---------- */
const UID = /^[0-9a-f]{16}$/;
const UNAME = /^[a-z0-9_.]{2,32}$/;
const MAX_MAIL = 2000;            // waiting messages per person
const MAX_BODY = 96 * 1024;       // one message (file pieces are sent as several)
const MAIL_DAYS = 30;
const MAX_FRIENDS = 200, FANOUT = 40;  // Cloudflare's free plan allows 50 sub-requests per event

const inbox = (env, uid) => env.INBOX.get(env.INBOX.idFromName("u:" + uid));
const nameBox = (env, name) => env.INBOX.get(env.INBOX.idFromName("n:" + name));
async function call(stub, path, body) {
  const r = await stub.fetch("https://inbox" + path, body === undefined ? {} : { method: "POST", body: JSON.stringify(body) });
  return r.json();
}
async function sha(s) {
  const d = await crypto.subtle.digest("SHA-256", enc.encode(s));
  return [...new Uint8Array(d)].map(b => b.toString(16).padStart(2, "0")).join("");
}

// Usernames are optional and first come, first served. They only map a name to a mailbox.
async function setUsername(request, env) {
  let m;
  try { m = await request.json(); } catch { return Response.json({ ok: false }, { status: 400 }); }
  const uid = String(m?.uid || ""), key = String(m?.key || ""), name = String(m?.name || "").toLowerCase();
  if (!UID.test(uid) || (name && !UNAME.test(name))) return Response.json({ ok: false, error: "bad name" }, { status: 400 });
  const me = inbox(env, uid);
  if (!(await call(me, "/verify", { key })).ok) return Response.json({ ok: false, error: "not yours" }, { status: 403 });
  const old = (await call(me, "/username")).name;
  if (name && name !== old && !(await call(nameBox(env, name), "/name/claim", { uid })).ok)
    return Response.json({ ok: false, error: "taken" });
  if (old && old !== name) await call(nameBox(env, old), "/name/release", { uid });
  await call(me, "/username", { name });
  return Response.json({ ok: true });
}

export class Inbox extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.env = env;
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    this.sql = ctx.storage.sql;
    this.sql.exec("CREATE TABLE IF NOT EXISTS mail (id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER, sender TEXT, body TEXT)");
    this.sql.exec("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT)");
  }
  get(k) {
    const r = this.sql.exec("SELECT v FROM kv WHERE k = ?", k).toArray()[0];
    return r ? JSON.parse(r.v) : undefined;
  }
  put(k, v) { this.sql.exec("INSERT OR REPLACE INTO kv (k, v) VALUES (?, ?)", k, JSON.stringify(v)); }
  del(k) { this.sql.exec("DELETE FROM kv WHERE k = ?", k); }
  live() { return this.ctx.getWebSockets().filter(ws => !ws.deserializeAttachment()?.gone); }
  broadcast(s) { for (const ws of this.live()) { try { ws.send(s); } catch {} } }

  async fetch(request) {
    const url = new URL(request.url);
    const body = request.method === "POST" ? await request.json().catch(() => ({})) : {};
    switch (url.pathname) {
      case "/inbox": return this.connect(request, url);
      case "/verify": {
        const have = this.get("keyhash");
        return Response.json({ ok: !!have && have === await sha(String(body.key || "")) });
      }
      case "/username":
        if (request.method === "POST") { body.name ? this.put("username", body.name) : this.del("username"); return Response.json({ ok: true }); }
        return Response.json({ name: this.get("username") || null });
      case "/name/get": return Response.json({ uid: this.get("owner") || null });
      case "/name/claim": {
        const owner = this.get("owner");
        if (owner && owner !== body.uid) return Response.json({ ok: false });
        this.put("owner", body.uid);
        return Response.json({ ok: true });
      }
      case "/name/release":
        if (this.get("owner") === body.uid) this.del("owner");
        return Response.json({ ok: true });
      case "/deliver": return Response.json(this.deliver(body));
      case "/presence": return Response.json(this.friendPresence(body));
    }
    return new Response("not found", { status: 404 });
  }

  // The owner connects with their secret key. The first key used becomes the mailbox's key.
  async connect(request, url) {
    const uid = url.searchParams.get("uid") || "", key = url.searchParams.get("key") || "";
    if (!UID.test(uid) || key.length < 16 || key.length > 128) return new Response("bad key", { status: 400 });
    const hash = await sha(key), have = this.get("keyhash");
    if (!have) { this.put("keyhash", hash); this.put("uid", uid); }
    else if (have !== hash) return new Response("wrong key", { status: 403 });
    const open = this.live();
    if (open.length >= 8) { try { open[0].close(4000, "too many tabs"); } catch {} }
    this.sql.exec("DELETE FROM mail WHERE ts < ?", Date.now() - MAIL_DAYS * 864e5);
    const [client, server] = Object.values(new WebSocketPair());
    this.ctx.acceptWebSocket(server);
    server.serializeAttachment({ win: Date.now(), n: 0 });
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws, raw) {
    if (typeof raw !== "string" || raw.length > MAX_BODY * 2) return;
    let m;
    try { m = JSON.parse(raw); } catch { return; }
    const att = ws.deserializeAttachment() || {};
    if (att.gone) return;
    const uid = this.get("uid");
    if (m.type === "hello" || m.type === "friends") {  // "friends": the friend list changed
      const friends = Array.isArray(m.friends) ? [...new Set(m.friends.filter(f => UID.test(f) && f !== uid))].slice(0, MAX_FRIENDS) : [];
      this.put("friends", friends);
      if (m.type === "hello") this.sendMail(ws);
      await this.setPresence(m.presence, ws);
    } else if (m.type === "presence") {
      await this.setPresence(m.presence, ws);
    } else if (m.type === "ack") {
      for (const id of (Array.isArray(m.ids) ? m.ids : []).slice(0, 1000)) this.sql.exec("DELETE FROM mail WHERE id = ?", Math.floor(+id) || 0);
    } else if (m.type === "send") {
      const now = Date.now();  // at most 120 messages per 10 seconds per tab
      if (now - att.win > 10000) { att.win = now; att.n = 0; }
      if (++att.n > 120) return;
      ws.serializeAttachment(att);
      const to = [...new Set((Array.isArray(m.to) ? m.to : []).filter(t => UID.test(t)))].slice(0, 12);
      const body = JSON.stringify(m.body ?? null);
      if (!to.length || body.length > MAX_BODY) return;
      const res = await Promise.all(to.map(t => call(inbox(this.env, t), "/deliver", { from: uid, body, eph: !!m.eph }).catch(() => ({ ok: false }))));
      if (m.ref) ws.send(JSON.stringify({ type: "sent", ref: String(m.ref).slice(0, 40), ok: res.map(r => !!r.ok) }));
    }
  }

  deliver({ from, body, eph }) {
    if (!UID.test(from || "") || typeof body !== "string") return { ok: false };
    const ts = Date.now();
    if (eph) {  // typing, ringing, presence pings: only if they're online right now
      this.broadcast(`{"type":"mail","items":[{"from":"${from}","ts":${ts},"body":${body}}]}`);
      return { ok: true };
    }
    const n = this.sql.exec("SELECT COUNT(*) AS n FROM mail").one().n;
    if (n >= MAX_MAIL) return { ok: false };
    const id = this.sql.exec("INSERT INTO mail (ts, sender, body) VALUES (?, ?, ?) RETURNING id", ts, from, body).one().id;
    this.broadcast(`{"type":"mail","items":[{"id":${id},"from":"${from}","ts":${ts},"body":${body}}]}`);
    return { ok: true };
  }

  sendMail(ws) {  // everything that arrived while they were away, in pieces under 1 MB
    let batch = [], size = 0;
    const flush = () => { if (batch.length) ws.send(`{"type":"mail","items":[${batch.join(",")}]}`); batch = []; size = 0; };
    for (const r of this.sql.exec("SELECT id, ts, sender, body FROM mail ORDER BY id")) {
      const s = `{"id":${r.id},"from":"${r.sender}","ts":${r.ts},"body":${r.body}}`;
      if (size + s.length > 800000) flush();
      batch.push(s); size += s.length;
    }
    flush();
  }

  // Presence is only shared between people who have each other as friends.
  async setPresence(p, ws) {
    p = cleanPresence(p);
    if (ws) { const a = ws.deserializeAttachment() || {}; a.p = p; ws.serializeAttachment(a); }
    this.put("presence", p);
    await this.fanout(p, true);
  }
  async fanout(p, collect) {
    const uid = this.get("uid"), friends = (this.get("friends") || []).slice(0, FANOUT);
    const res = await Promise.all(friends.map(f => call(inbox(this.env, f), "/presence", { from: uid, p }).catch(() => null)));
    if (!collect) return;
    const list = {};
    friends.forEach((f, i) => { list[f] = res[i]?.p ?? null; });
    this.broadcast(JSON.stringify({ type: "presence", list, all: true }));
  }
  friendPresence({ from, p }) {
    if (!(this.get("friends") || []).includes(from)) return { p: null };
    this.broadcast(JSON.stringify({ type: "presence", list: { [from]: cleanPresence(p) } }));
    return { p: this.live().length ? this.get("presence") ?? null : null };
  }

  async webSocketClose(ws) { await this.closed(ws); }
  async webSocketError(ws) { await this.closed(ws); }
  async closed(ws) {
    try { ws.serializeAttachment({ ...ws.deserializeAttachment(), gone: true }); } catch {}
    try { ws.close(1000, "bye"); } catch {}
    const rest = this.live();
    if (rest.length) return;
    this.put("presence", null);
    await this.fanout(null, false);  // tell friends we went offline
  }
}

function cleanPresence(p) {
  if (!p || typeof p !== "object") return null;
  const s = (v, n) => typeof v === "string" ? v.slice(0, n) : "";
  return {
    status: ["online", "idle", "dnd"].includes(p.status) ? p.status : "online",
    name: s(p.name, 32), custom: s(p.custom, 128), act: s(p.act, 100), live: !!p.live,
    pf: p.pf && typeof p.pf === "object" ? { about: s(p.pf.about, 190), pronouns: s(p.pf.pronouns, 40), color: Math.floor(+p.pf.color) % 360 || 0, banner: Math.floor(+p.pf.banner) % 360 || 0 } : null,
  };
}

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
