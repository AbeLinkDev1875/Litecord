/* ---------- friends, direct messages and group chats ---------- */
// No server needed for these: each person has a small mailbox on Cloudflare (the Inbox in worker.js)
// that holds messages while they're offline and tells friends who's online. DM calls use a private,
// unguessable room, so voice, video and screen share still go straight between browsers.
const myKey = localStorage.lcKey ||= rid(24);   // proves the mailbox is ours; never shown or sent to anyone else
const UIDRE = /^[0-9a-f]{16}$/, GIDRE = /^[0-9a-f]{24}$/;
const DM_FILE_MAX = 8 * 1024 * 1024, DM_CHUNK = 48 * 1024, GROUP_MAX = 10, DM_HISTORY = 2000;
const DM_VOICE = "default-lounge";  // the voice channel every DM call room has
function loadObj(k) { try { const v = JSON.parse(localStorage[k] || "{}"); return v && typeof v === "object" && !Array.isArray(v) ? v : {}; } catch { return {}; } }
const friends = loadObj("lcFriends");  // uid -> {name, state: "friend" | "in" | "out" | "blocked", since}
const dms = loadObj("lcDMs");          // "u:<uid>" or "g:<id>" -> {key, group, name, members: {uid: name}, owner, cid, last, closed, left}
const presence = new Map();            // uid -> what their mailbox says (null = offline)
let homeMode = false, friendsTab = "online", friendQuery = "", serverView = null, homeView = null;
let pendingCall = params.get("call"), pendingRing = params.get("ring") === "1";
const isDM = ch => typeof ch === "string" && ch.startsWith("dm:");
const isDMRoom = () => room.startsWith("dm-");
const dmOf = ch => isDM(ch) ? dms[ch.slice(3)] : null;
const curDM = () => isDMRoom() ? Object.values(dms).find(d => "dm-" + d.cid === room) || null : null;
const friendUids = () => Object.keys(friends).filter(u => friends[u].state === "friend");
const validCid = c => typeof c === "string" && GIDRE.test(c) ? c : null;
function saveSocial() { localStorage.lcFriends = JSON.stringify(friends); localStorage.lcDMs = JSON.stringify(dms); }

/* people */
function customText() {
  const c = prefs.custom;
  if (!c?.text) return "";
  if (c.until && Date.now() > c.until) { prefs.custom = null; savePrefs(); return ""; }
  return c.text;
}
function pfOut() {
  const p = prefs.profile || {};
  return {about: String(p.about || "").slice(0, 190), pronouns: String(p.pronouns || "").slice(0, 40), color: p.color ?? -1, banner: p.banner ?? -1};
}
function cleanPf(p) {
  if (!p || typeof p !== "object") return null;
  const n = v => { v = Math.floor(+v); return v >= 0 && v < 360 ? v : -1; };
  return {about: String(p.about ?? "").slice(0, 190), pronouns: String(p.pronouns ?? "").slice(0, 40), color: n(p.color), banner: n(p.banner)};
}
function selfPerson() { return {id: myId, uid: myUid, name: myName, status: myStatus(), custom: customText(), pf: pfOut(), state: myState()}; }
function memberName(d, uid) { return d?.members?.[uid] || friends[uid]?.name || "Someone"; }
function friendPerson(uid) {
  if (uid === myUid) return selfPerson();
  const p = presence.get(uid), f = friends[uid], here = [...people.values()].find(x => x.uid === uid);
  let name = p?.name || f?.name || here?.name;
  if (!name) for (const d of Object.values(dms)) if (d.members[uid]) { name = d.members[uid]; break; }
  return {id: "f:" + uid, uid, name: name || "Someone", status: p ? p.status : "invisible", custom: p?.custom || "",
          act: p?.act || "", live: !!p?.live, pf: p?.pf || here?.pf || null, friend: f?.state === "friend"};
}
function subline(p) {
  if (p.live) return "Sharing their screen";
  return p.custom || p.act || "";
}

/* the mailbox connection */
let ibx = null, ibxReady = false, ibxQueue = [], ibxFails = 0;
const seenMail = new Set();
function connectInbox() {
  if (!myName) return;
  const proto = location.protocol === "https:" ? "wss" : "ws";
  let s;
  try { s = ibx = new WebSocket(`${proto}://${location.host}/inbox?uid=${myUid}&key=${myKey}`); } catch { return; }
  s.onopen = () => {
    ibxReady = true; ibxFails = 0;
    lastPres = JSON.stringify(myPresence());
    s.send(JSON.stringify({type: "hello", friends: friendUids(), presence: myPresence()}));
    const q = ibxQueue; ibxQueue = [];
    q.forEach(x => s.send(x));
  };
  s.onmessage = e => {
    if (e.data === "pong") return;
    let m;
    try { m = JSON.parse(e.data); } catch { return; }
    if (m.type === "mail") onMail(Array.isArray(m.items) ? m.items : []);
    else if (m.type === "presence") onPresence(m);
  };
  s.onclose = e => {
    if (ibx !== s) return;
    ibxReady = false;
    if (e.code === 4000) return;  // too many tabs: this one gave way
    if (++ibxFails === 4) toast("Friends and DMs can't connect right now. Retrying…", 5000);
    setTimeout(connectInbox, Math.min(30000, 2000 * ibxFails) + Math.random() * 1000);
  };
}
setInterval(() => { if (ibx?.readyState === 1) ibx.send("ping"); }, 25000);
function ibxSend(obj) {
  const s = JSON.stringify(obj);
  if (ibxReady && ibx?.readyState === 1) ibx.send(s);
  else if (ibxQueue.length < 2000) ibxQueue.push(s);
}
function mail(to, body, eph = false) {
  to = [...new Set(to)].filter(u => UIDRE.test(u) && u !== myUid && friends[u]?.state !== "blocked");
  for (let i = 0; i < to.length; i += 12) ibxSend({type: "send", to: to.slice(i, i + 12), body, eph});
}
function myPresence() {
  const s = myStatus();
  if (s === "invisible") return null;
  const share = prefs.shareActivity !== false;
  return {status: s, name: myName, custom: customText(), pf: pfOut(), live: share && !!screenStream,
          act: share && voiceCh ? (isDMRoom() ? "In a call" : "In a voice channel") : ""};
}
let lastPres = "";
function pushPresence() {
  if (!ibxReady) return;
  const p = myPresence(), s = JSON.stringify(p);
  if (s === lastPres) return;
  lastPres = s;
  ibxSend({type: "presence", presence: p});
}
setInterval(pushPresence, 4000);
function updateFriendList() {
  saveSocial();
  if (ibxReady) ibxSend({type: "friends", friends: friendUids(), presence: myPresence()});
  renderHomeSoon();
  if (isDM(view?.ch)) refreshMessages();
}
function onPresence(m) {
  if (m.all) for (const u of [...presence.keys()]) if (!(u in (m.list || {}))) presence.delete(u);
  for (const [u, p] of Object.entries(m.list || {})) {
    if (!UIDRE.test(u)) continue;
    presence.set(u, p && typeof p === "object" ? {...p, pf: cleanPf(p.pf)} : null);
    const nm = p?.name && oneLine(p.name).slice(0, 32);
    if (nm && friends[u] && friends[u].name !== nm) { friends[u].name = nm; saveSocial(); }
  }
  renderHomeSoon();
}

/* incoming mail */
function onMail(items) {
  const ack = [];
  for (const it of items) {
    if (it.id) { ack.push(it.id); if (seenMail.has(it.id)) continue; seenMail.add(it.id); }
    const from = String(it.from ?? ""), b = it.body;
    if (!UIDRE.test(from) || !b || typeof b !== "object" || friends[from]?.state === "blocked") continue;
    try { handleMail(from, b, +it.ts || Date.now()); } catch (err) { console.warn(err); }
  }
  if (ack.length) ibxSend({type: "ack", ids: ack});
  saveSocial();
  renderHomeSoon();
}
const mailKey = (from, b) => GIDRE.test(String(b.k ?? "")) ? "g:" + b.k : "u:" + from;
const canDMFrom = uid => friends[uid]?.state === "friend" || !!dms["u:" + uid] || prefs.serverDMs !== false;
function handleMail(from, b, ts) {
  const nm = oneLine(b.n).slice(0, 32);
  switch (b.t) {
    case "freq": {
      const f = friends[from];
      if (f?.state === "friend" || f?.state === "out") {  // they lost track, or we both asked: just be friends
        if (f.state === "out") toast(`You and ${nm || f.name} are now friends!`);
        friends[from] = {name: nm || f.name, state: "friend", since: Date.now()};
        mail([from], {t: "facc", n: myName});
        return updateFriendList();
      }
      if (prefs.friendReqs === false || f?.state === "in") return;
      friends[from] = {name: nm || "Someone", state: "in", since: ts};
      sfx("friend");
      toast(`${nm || "Someone"} sent you a friend request.`, 5000);
      return renderHomeSoon();
    }
    case "facc": {
      const f = friends[from];
      if (!f || f.state === "blocked") return;
      if (f.state === "out") { toast(`${nm || f.name} accepted your friend request!`, 5000); sfx("friend"); }
      friends[from] = {name: nm || f.name, state: "friend", since: Date.now()};
      return updateFriendList();
    }
    case "fdel":
      if (friends[from] && friends[from].state !== "blocked") { delete friends[from]; presence.delete(from); updateFriendList(); }
      return;
    case "dm": return mailDM(from, b, nm, ts);
    case "op": return mailOp(from, b);
    case "typing": {
      const d = dms[mailKey(from, b)];
      if (!d || (d.group && !d.members[from])) return;
      typingNow.set("dm:" + from, {ch: "dm:" + d.key, name: nm || memberName(d, from), until: Date.now() + 6000});
      return renderTyping();
    }
    case "gmeta": return mailGroup(from, b);
    case "ring": return incomingCall(from, b, nm);
    case "ring-stop": if (ringing?.from === from) stopRinging(); return;
    case "file": return mailFile(from, b);
  }
}
function cleanGroup(g) {
  if (!g || typeof g !== "object") return null;
  const members = {};
  for (const [u, n] of Object.entries(g.members || {}).slice(0, GROUP_MAX)) if (UIDRE.test(u)) members[u] = oneLine(n).slice(0, 32) || "Someone";
  if (!Object.keys(members).length) return null;
  return {name: oneLine(g.name).slice(0, 100), members, owner: UIDRE.test(String(g.owner ?? "")) && members[g.owner] ? g.owner : Object.keys(members)[0]};
}
const groupOut = d => ({name: d.name || "", members: d.members, owner: d.owner});
const keyPart = d => d.group ? {k: d.key.slice(2)} : {};
function groupFromMail(key, b, from) {
  const g = cleanGroup(b.g);
  if (!g || !g.members[myUid] || !g.members[from]) return null;
  return dms[key] = {key, group: true, cid: key.slice(2), ...g, last: Date.now()};
}
function mailDM(from, b, nm, ts) {
  const key = mailKey(from, b), m = b.msg;
  if (!m || typeof m !== "object" || !/^[0-9a-f]{12}$/.test(String(m.mid ?? ""))) return;
  let d = dms[key];
  if (key.startsWith("g:")) {
    if (!d) { d = groupFromMail(key, b, from); if (!d) return; sysLine(d, `${nm || "Someone"} added you to the group.`, ts - 1); }
    if (!d.members[from] || d.left) return;
  } else {
    if (!d) { if (!canDMFrom(from)) return; d = dms[key] = {key, group: false, members: {[from]: nm || "Someone"}, cid: validCid(b.cid) || rid(12)}; }
    if (validCid(b.cid) && b.cid < d.cid) d.cid = b.cid;  // both started a DM at once: settle on one call room
  }
  if (nm) d.members[from] = nm;
  const ev = {type: "chat", ch: "dm:" + key, mid: m.mid, id: "f:" + from, uid: from, name: nm || memberName(d, from),
              text: String(m.text ?? "").slice(0, 2000), ts: Math.min(+m.ts || ts, Date.now())};
  if (m.reply && typeof m.reply === "object")
    ev.reply = {mid: String(m.reply.mid ?? "").slice(0, 24), name: oneLine(m.reply.name).slice(0, 32), text: String(m.reply.text ?? "").slice(0, 120)};
  if (m.file) ev.file = m.file;
  d.closed = false;
  onChat(ev);
}
function mailOp(from, b) {
  const d = dms[mailKey(from, b)], e = b.ev;
  if (!d || !e || typeof e !== "object" || (d.group && !d.members[from])) return;
  const ch = "dm:" + d.key, t = findMsg(ch, String(e.mid ?? ""));
  if (!t) return;
  const own = t.uid === from;
  if (e.type === "edit" && own) onMsgEvent({type: "edit", ch, mid: t.mid, text: String(e.text ?? "").slice(0, 2000), edited: Date.now()});
  else if (e.type === "del" && own) onMsgEvent({type: "del", ch, mid: t.mid});
  else if (e.type === "react") {
    const emoji = String(e.emoji ?? "");
    if (emoji && emoji.length <= 16 && !/\s/.test(emoji)) onMsgEvent({type: "react", ch, mid: t.mid, emoji, uid: from, on: !!e.on});
  } else if (e.type === "pin") onMsgEvent({type: "pin", ch, mid: t.mid, on: !!e.on});
}
function mailGroup(from, b) {
  const key = mailKey(from, b);
  if (!key.startsWith("g:")) return;
  const g = cleanGroup(b.g);
  if (!g) return;
  let d = dms[key];
  if (!d) {
    if (!g.members[myUid] || !g.members[from]) return;
    d = dms[key] = {key, group: true, cid: key.slice(2), ...g, last: Date.now()};
    sysLine(d, `${memberName(d, from)} added you to the group.`);
    return;
  }
  if (!d.members[from] || d.left) return;
  const before = d.members, who = memberName(d, from);
  const added = Object.keys(g.members).filter(u => !before[u]), gone = Object.keys(before).filter(u => !g.members[u]);
  if (b.why === "leave" ? gone.length !== 1 || gone[0] !== from || added.length : gone.length && from !== d.owner) return;
  if ((g.name || "") !== (d.name || "")) sysLine(d, g.name ? `${who} changed the group name to ${g.name}.` : `${who} removed the group name.`);
  added.forEach(u => sysLine(d, `${who} added ${g.members[u]} to the group.`));
  gone.forEach(u => sysLine(d, u === from ? `${who} left the group.` : `${who} removed ${before[u]} from the group.`));
  Object.assign(d, g);
  if (!d.members[myUid]) { d.left = true; sysLine(d, "You were removed from this group."); }
  saveSocial();
  if (view?.ch === "dm:" + key) renderView();
}
// System lines in DMs ("Ana added Ben to the group"). Only kept on this device.
function sysLine(d, text, ts = Date.now()) {
  onChat({type: "chat", ch: "dm:" + d.key, mid: rid(6), uid: "", id: "", name: "", text, ts, sys: true});
}

/* sending in DMs: send() hands anything for a "dm:" channel to this */
function dmTargets(d) { return Object.keys(d.members).filter(u => u !== myUid); }
function dmSend(m) {
  const d = dmOf(m.ch);
  if (!d) return;
  if (d.left) return toast("You're not in this group anymore.");
  if (!d.group && friends[dmOther(d)]?.state === "blocked") return toast("You blocked this person. Unblock them to send messages.");
  const to = dmTargets(d);
  if (m.type === "typing") return mail(to, {t: "typing", n: myName, ...keyPart(d)}, true);
  if (m.type === "chat") {
    const text = String(m.text ?? "").trim().slice(0, 2000);
    if (!text && !m.file) return;
    const msg = {mid: rid(6), text, ts: Date.now()};
    const r = m.reply ? findMsg(m.ch, String(m.reply)) : null;
    if (r) msg.reply = {mid: r.mid, name: r.name, text: (r.text || (r.file ? "📎 " + r.file.name : "")).slice(0, 120)};
    if (m.file) msg.file = m.file;
    mail(to, {t: "dm", n: myName, ...keyPart(d), g: d.group ? groupOut(d) : undefined, cid: d.group ? undefined : d.cid, msg});
    onChat({type: "chat", ch: m.ch, id: myId, uid: myUid, name: myName, ...msg});
    return;
  }
  const t = findMsg(m.ch, String(m.mid ?? ""));
  if (!t) return;
  const own = t.uid === myUid;
  let ev;
  if (m.type === "edit") {
    const text = String(m.text ?? "").trim().slice(0, 2000);
    if (!own || !text || text === t.text) return;
    ev = {type: "edit", mid: t.mid, text};
  } else if (m.type === "del") {
    if (!own) return;
    ev = {type: "del", mid: t.mid};
  } else if (m.type === "react") {
    const emoji = String(m.emoji ?? "");
    if (!emoji || emoji.length > 16) return;
    ev = {type: "react", mid: t.mid, emoji, on: !(t.reactions?.[emoji] || []).includes(myUid)};
  } else if (m.type === "pin") ev = {type: "pin", mid: t.mid, on: !t.pinned};
  else return;
  mail(to, {t: "op", ...keyPart(d), ev});
  onMsgEvent({...ev, ch: m.ch, uid: myUid, edited: Date.now()});
}
const EMOTICONS = {":)": "🙂", ":-)": "🙂", ":D": "😄", ";)": "😉", ":(": "🙁", ":P": "😛", ":p": "😛", "<3": "❤️", ":O": "😮", ":o": "😮", "xD": "😆", "XD": "😆", ":/": "😕", ":'(": "😢", "B)": "😎"};
function convertEmoticons(text) {
  if (text.includes("`")) return text;
  return text.replace(/(^|\s)(:-?\)|:D|;\)|:\(|:[Pp]|<3|:[Oo]|[xX]D|:\/|:'\(|B\))(?=\s|$)/g, (all, sp, e) => sp + (EMOTICONS[e] || e));
}

/* DM history and files: saved on this device only */
const dmSaveTimers = new Map();
function saveDM(key) {
  clearTimeout(dmSaveTimers.get(key));
  dmSaveTimers.set(key, setTimeout(() => {
    dmSaveTimers.delete(key);
    idb("rooms", "readwrite", st => st.put({key, msgs: hist.get("dm:" + key) || []}, "\u0001dm:" + key));
  }, 800));
}
function saveDMFile(fid) {
  if (!blobs.has(fid) || !fileMeta.has(fid)) return;
  idb("files", "readwrite", st => st.put({meta: fileMeta.get(fid), blob: blobs.get(fid)}, "\u0001dm/" + fid));
}
async function loadDMs() {
  const rows = await idb("rooms", "readonly", st => st.getAll(IDBKeyRange.bound("\u0001dm:", "\u0001dm:￿")));
  for (const r of rows || []) if (r?.key && dms[r.key] && Array.isArray(r.msgs)) hist.set("dm:" + r.key, r.msgs.slice(-DM_HISTORY));
  const files = await idb("files", "readonly", st => st.getAll(IDBKeyRange.bound("\u0001dm/", "\u0001dm/￿")));
  for (const f of files || []) if (f?.meta && FID.test(f.meta.id) && f.blob instanceof Blob) { fileMeta.set(f.meta.id, f.meta); blobs.set(f.meta.id, f.blob); }
}
function dmTouched(m) {
  const d = dmOf(m.ch);
  if (!d) return;
  d.last = Math.max(d.last || 0, m.ts);
  saveSocial(); saveDM(d.key);
  renderHomeSoon();
}
function dmFixFile(m) {
  const r = m.file;
  if (!r) return;
  if (!FID.test(String(r.id ?? ""))) { delete m.file; return; }
  if (!fileMeta.has(r.id)) {
    const size = Math.floor(+r.size);
    if (!(size > 0 && size <= DM_FILE_MAX)) { delete m.file; return; }
    fileMeta.set(r.id, {id: r.id, name: safeName(r.name), size, type: guessType(r.name)});
  }
  m.file = fileMeta.get(r.id);
}
function b64(u8) { let s = ""; for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode(...u8.subarray(i, i + 0x8000)); return btoa(s); }
function unb64(s) { const bin = atob(s), u = new Uint8Array(bin.length); for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i); return u; }
// Files in DMs go through the mailboxes in pieces, so friends get them even if they're offline.
async function dmUpload(file) {
  const ch = view.ch, d = dmOf(ch);
  if (!d || d.left) return;
  if (!file.size) return toast(`"${file.name}" is empty.`);
  if (file.size > DM_FILE_MAX) return toast(`"${file.name}" is too big for a DM. The limit is 8 MB.`);
  const fid = rid(12), meta = {id: fid, name: safeName(file.name), size: file.size, type: guessType(file.name)};
  fileMeta.set(fid, meta); blobs.set(fid, file); saveDMFile(fid);
  const to = dmTargets(d), n = Math.ceil(file.size / DM_CHUNK);
  const line = h("div", {}, `Sending ${meta.name}…`);
  $("#uploads").append(line);
  for (let i = 0; i < n; i++) {
    const buf = new Uint8Array(await file.slice(i * DM_CHUNK, (i + 1) * DM_CHUNK).arrayBuffer());
    mail(to, {t: "file", ...keyPart(d), fid, i, n, name: meta.name, size: meta.size, data: b64(buf)});
    line.textContent = `Sending ${meta.name}… ${Math.round((i + 1) / n * 100)}%`;
    if (n > 1) await new Promise(r => setTimeout(r, 110));  // the mailbox takes about 9 pieces a second
  }
  line.remove();
  dmSend({type: "chat", ch, text: "", file: meta});
}
const dmParts = new Map();  // file id -> pieces arriving
function mailFile(from, b) {
  const d = dms[mailKey(from, b)];
  if (d ? d.group && !d.members[from] : !canDMFrom(from)) return;
  const fid = String(b.fid ?? ""), n = Math.floor(+b.n), i = Math.floor(+b.i), size = Math.floor(+b.size);
  if (!FID.test(fid) || blobs.has(fid) || !(n > 0 && n <= Math.ceil(DM_FILE_MAX / DM_CHUNK)) || !(i >= 0 && i < n)
      || !(size > 0 && size <= DM_FILE_MAX) || typeof b.data !== "string") return;
  let f = dmParts.get(fid);
  if (!f) {
    if (dmParts.size >= 20) return;
    f = {from, n, size, parts: new Array(n), got: 0, meta: {id: fid, name: safeName(b.name), size, type: guessType(b.name)}};
    dmParts.set(fid, f);
  }
  if (f.from !== from || f.parts[i]) return;
  try { f.parts[i] = unb64(b.data); } catch { return; }
  if (++f.got < f.n) return;
  dmParts.delete(fid);
  const blob = new Blob(f.parts);
  if (blob.size !== f.size) return;
  if (!fileMeta.has(fid)) fileMeta.set(fid, f.meta);
  blobs.set(fid, blob);
  saveDMFile(fid);
  document.querySelectorAll(`[data-fid="${fid}"]`).forEach(el => el.replaceWith(attachment(fileMeta.get(fid))));
}

/* friends */
async function sendFriendRequest(input) {
  input = input.trim().replace(/^@/, "");
  let uid = null, name = null;
  if (UIDRE.test(input.toLowerCase())) uid = input.toLowerCase();
  else {
    const u = input.toLowerCase();
    if (!/^[a-z0-9_.]{2,32}$/.test(u)) return [false, "Hm, that doesn't look like a username or friend code."];
    const r = await fetch("/api/lookup?name=" + encodeURIComponent(u)).then(r => r.json()).catch(() => null);
    if (!r) return [false, "Couldn't check that right now. Try again in a moment."];
    uid = r.uid; name = u;
    if (!uid) return [false, "Hm, didn't work. Double check that the username is correct."];
  }
  if (uid === myUid) return [false, "That's you! You can't add yourself."];
  return requestFriend(uid, name);
}
function requestFriend(uid, name) {
  const f = friends[uid];
  if (f?.state === "friend") return [false, "You're already friends with them!"];
  if (f?.state === "in") { acceptFriend(uid); return [true, `You and ${f.name} are now friends!`]; }
  if (f?.state === "blocked") delete friends[uid];
  friends[uid] = {name: name || f?.name || friendPerson(uid).name, state: "out", since: Date.now()};
  mail([uid], {t: "freq", n: myName});
  saveSocial(); renderHomeSoon();
  if (isDM(view?.ch)) refreshMessages();
  return [true, `Success! Your friend request to ${friends[uid].name} was sent.`];
}
function acceptFriend(uid) {
  if (!friends[uid]) return;
  friends[uid].state = "friend"; friends[uid].since = Date.now();
  mail([uid], {t: "facc", n: myName});
  updateFriendList();
}
function removeFriend(uid) {
  if (!friends[uid]) return;
  delete friends[uid]; presence.delete(uid);
  mail([uid], {t: "fdel"});
  updateFriendList();
}
function confirmRemoveFriend(uid) {
  if (confirm(`Remove ${friendPerson(uid).name} from your friends?`)) removeFriend(uid);
}
function blockUser(uid, name) {
  if (!confirm(`Block ${name || friendPerson(uid).name}? You won't get their messages or friend requests.`)) return;
  if (friends[uid]?.state === "friend" || friends[uid]?.state === "out") mail([uid], {t: "fdel"});
  friends[uid] = {name: name || friendPerson(uid).name, state: "blocked", since: Date.now()};
  presence.delete(uid);
  updateFriendList();
}
function unblockUser(uid) { delete friends[uid]; updateFriendList(); }

/* DMs */
function dmOther(d) { return d && !d.group ? Object.keys(d.members)[0] : null; }
function dmTitle(d) {
  if (!d) return "Direct Message";
  if (!d.group) return friendPerson(dmOther(d)).name;
  if (d.name) return d.name;
  return Object.entries(d.members).filter(([u]) => u !== myUid).map(([, n]) => n).join(", ") || "Unnamed Group";
}
function dmIcon(d, dot = false) {
  if (!d.group) return avatar(friendPerson(dmOther(d)), dot);
  return h("span", {class: "av gav", style: `background:hsl(${hue(d.key)} 45% 42%)`}, icon("users"));
}
function dmFor(uid, name) {
  const key = "u:" + uid;
  dms[key] ||= {key, group: false, members: {[uid]: name || friendPerson(uid).name}, cid: rid(12), last: Date.now()};
  dms[key].closed = false;
  saveSocial();
  return dms[key];
}
function openDM(key) {
  const d = dms[key];
  if (!d) return;
  d.closed = false; saveSocial();
  if (!homeMode) { serverView = view; homeMode = true; }
  openView({ch: "dm:" + key, kind: "text"});
}
function messageUser(uid, name) { if (UIDRE.test(uid || "") && uid !== myUid) openDM(dmFor(uid, name).key); }
function closeDM(d) {
  d.closed = true; saveSocial();
  if (view?.ch === "dm:" + d.key) openView({ch: "@friends", kind: "friends"});
  else renderSidebar();
}
function createGroup(uids) {
  const id = rid(12), key = "g:" + id, members = {[myUid]: myName};
  for (const u of uids.slice(0, GROUP_MAX - 1)) members[u] = friendPerson(u).name;
  const d = dms[key] = {key, group: true, name: "", members, owner: myUid, cid: id, last: Date.now()};
  saveSocial();
  mail(dmTargets(d), {t: "gmeta", n: myName, k: id, g: groupOut(d), why: "create"});
  sysLine(d, "You started the group.");
  openDM(key);
}
function groupChange(d, members, name, why, lines) {
  const old = dmTargets(d);
  Object.assign(d, {members, name});
  if (!d.members[d.owner]) d.owner = Object.keys(d.members)[0];
  mail([...new Set([...old, ...dmTargets(d)])], {t: "gmeta", n: myName, k: d.key.slice(2), g: groupOut(d), why});
  lines.forEach(t => sysLine(d, t));
  saveSocial();
  if (view?.ch === "dm:" + d.key) renderView();
  else renderSidebar();
}
function addToGroup(d, uids) {
  const members = {...d.members};
  for (const u of uids) if (Object.keys(members).length < GROUP_MAX) members[u] = friendPerson(u).name;
  const added = Object.keys(members).filter(u => !d.members[u]);
  if (added.length) groupChange(d, members, d.name, "add", added.map(u => `You added ${members[u]} to the group.`));
}
function renameGroup(d) {
  const n = prompt("Group name (leave empty to use everyone's names):", d.name || "");
  if (n === null) return;
  const name = oneLine(n).slice(0, 100);
  if (name !== (d.name || "")) groupChange(d, d.members, name, "rename", [name ? `You changed the group name to ${name}.` : "You removed the group name."]);
}
function kickFromGroup(d, uid) {
  if (d.owner !== myUid || !confirm(`Remove ${memberName(d, uid)} from the group?`)) return;
  const members = {...d.members}, name = members[uid];
  delete members[uid];
  groupChange(d, members, d.name, "kick", [`You removed ${name} from the group.`]);
}
function leaveGroup(d) {
  if (!confirm(`Leave "${dmTitle(d)}"? You won't be able to rejoin unless someone adds you again.`)) return;
  const members = {...d.members};
  delete members[myUid];
  mail(dmTargets(d), {t: "gmeta", n: myName, k: d.key.slice(2), g: {...groupOut(d), members, owner: d.owner === myUid ? Object.keys(members)[0] : d.owner}, why: "leave"});
  delete dms[d.key];
  hist.delete("dm:" + d.key);
  idb("rooms", "readwrite", st => st.delete("\u0001dm:" + d.key));
  saveSocial();
  if (view?.ch === "dm:" + d.key) openView({ch: "@friends", kind: "friends"});
  else renderSidebar();
}

/* calls in DMs */
let ringing = null;  // an incoming call card
let calling = null;  // we're ringing them: {until, loop}
function startCall(d, video = false) {
  if (!d || d.left) return;
  if (!canMedia) return toast("Calls need HTTPS. Open the https:// link.");
  if (room === "dm-" + d.cid) { if (!voiceCh) joinDMCall(video, true); else if (video && !camStream) toggleCamera(); return; }
  if (voiceCh && !confirm("You're in a voice channel. Leave it and start this call?")) return;
  saveSocial();
  location.href = `?room=dm-${d.cid}&dm=${encodeURIComponent(d.key)}&call=${video ? "video" : "1"}&ring=1`;
}
async function joinDMCall(video, ring = false) {
  if (!chById(DM_VOICE)) { pendingCall = video ? "video" : "1"; pendingRing ||= ring; return; }
  await joinVoice(DM_VOICE);
  if (video && !camStream) toggleCamera();
  if (ring) ringMembers(video);
}
function afterSyncSocial() {
  if (pendingCall && chById(DM_VOICE)) {
    const v = pendingCall === "video", r = pendingRing;
    pendingCall = null; pendingRing = false;
    joinDMCall(v, r);
  }
}
function ringMembers(video) {
  const d = curDM();
  if (!d) return;
  const to = dmTargets(d).filter(u => !everyone().some(p => p.uid === u && p.state?.voice === DM_VOICE));
  if (!to.length) return;
  mail(to, {t: "ring", n: myName, ...keyPart(d), g: d.group ? groupOut(d) : undefined, cid: d.cid, video: !!video}, true);
  stopCalling();
  calling = {until: Date.now() + 30000, loop: setInterval(() => { if (!calling || Date.now() > calling.until) return stopCalling(); sfx("calling"); }, 3000)};
  sfx("calling");
  renderCallSoon();
}
function stopCalling() {
  if (!calling) return;
  clearInterval(calling.loop); calling = null;
  renderCallSoon();
}
function leftVoiceSocial() {
  const d = curDM();
  if (calling && d) mail(dmTargets(d), {t: "ring-stop", ...keyPart(d)}, true);
  stopCalling();
}
function incomingCall(from, b, nm) {
  const key = mailKey(from, b), cid = validCid(b.cid);
  if (!cid) return;
  let d = dms[key];
  if (!d) {
    if (key.startsWith("g:")) { d = groupFromMail(key, b, from); if (!d) return; }
    else { if (!canDMFrom(from)) return; d = dms[key] = {key, group: false, members: {[from]: nm || "Someone"}, cid}; }
    saveSocial(); renderHomeSoon();
  }
  if (d.left || (d.group && !d.members[from])) return;
  if (room === "dm-" + cid && voiceCh) return;  // already in it
  stopRinging();
  const who = d.group ? dmTitle(d) : nm || dmTitle(d);
  const go = () => {
    stopRinging();
    if (!d.group) d.cid = cid;  // use the caller's call room
    d.closed = false; saveSocial();
    location.href = `?room=dm-${cid}&dm=${encodeURIComponent(d.key)}&call=${b.video ? "video" : "1"}`;
  };
  const card = h("div", {class: "ringcard"},
    h("div", {class: "ringav"}, d.group ? dmIcon(d) : avatar(friendPerson(from))),
    h("b", {}, who),
    h("div", {class: "tiny"}, d.group ? `${nm || "Someone"} is calling the group…` : b.video ? "Incoming video call…" : "Incoming call…"),
    h("div", {class: "ringbtns"},
      h("button", {class: "rbtn no", "data-tip": "Decline", onclick: stopRinging}, icon("hangup")),
      h("button", {class: "rbtn yes", "data-tip": b.video ? "Join with Video" : "Join Call", onclick: go}, icon(b.video ? "camera" : "phone"))));
  document.body.append(card);
  ringing = {from, el: card, loop: setInterval(() => sfx("ring"), 2600), timer: setTimeout(stopRinging, 30000)};
  sfx("ring");
  if (document.hidden && "Notification" in window && Notification.permission === "granted" && !prefs.streamer?.noNotif) {
    try { const n = new Notification(`${who} is calling`, {body: "Click to answer in LiteCord", icon: "/icon-192.png", tag: "call"}); n.onclick = () => { focus(); n.close(); }; } catch {}
  }
}
function stopRinging() {
  if (!ringing) return;
  clearInterval(ringing.loop); clearTimeout(ringing.timer);
  ringing.el.remove(); ringing = null;
}

/* home: the friends page and the DM list */
function setHome(on) {
  if (!on && (!room || isDMRoom())) return;
  if (on === homeMode) { if (on && view?.kind !== "friends" && !isDM(view?.ch)) openView({ch: "@friends", kind: "friends"}); return; }
  homeMode = on;
  if (on) { serverView = view; openView(homeView && viewExists(homeView) ? homeView : {ch: "@friends", kind: "friends"}); }
  else { homeView = view; openView(serverView && viewExists(serverView) ? serverView : defaultView()); }
}
let homeTimer = 0;
function renderHomeSoon() {
  if (homeTimer) return;
  homeTimer = setTimeout(() => {
    homeTimer = 0;
    renderRail();
    if (!homeMode) return;
    renderHomeSide(); renderHomeMembers();
    if (view?.kind === "friends") fillFriendList();
    else if (isDM(view?.ch)) dmHeader();
  }, 60);
}
function renderSidebar() {
  renderRail();
  const bar = $("#roomBar");
  bar.classList.toggle("find", homeMode);
  if (homeMode) {
    bar.replaceChildren(h("span", {class: "findbox"}, "Find or start a conversation"));
    bar.title = "";
    renderHomeSide(); renderMemberList(); renderCallSoon();
    return;
  }
  bar.replaceChildren(h("span", {id: "roomName"}, room), icon("chevron"));
  bar.title = "";
  renderServerSide();
}
function renderRail() {
  const dmUnread = Object.values(dms).filter(d => (unread.get("dm:" + d.key) || 0) > 0 && !d.left);
  const reqs = Object.values(friends).filter(f => f.state === "in").length;
  const total = dmUnread.reduce((n, d) => n + unread.get("dm:" + d.key), 0) + reqs;
  const badge = n => n ? h("span", {class: "rbadge"}, n > 99 ? "99+" : n) : null;
  const serverUnread = !homeMode && false;
  $("#rail").replaceChildren(
    h("button", {class: "srv home" + (homeMode ? " active" : ""), "data-tip": "Direct Messages", "data-tip-side": "right", onclick: () => setHome(true)},
      h("img", {src: "/icon.svg", alt: ""}), badge(total)),
    ...dmUnread.map(d => h("button", {class: "srv dmi", "data-tip": dmTitle(d), "data-tip-side": "right", onclick: () => openDM(d.key)},
      dmIcon(d), badge(unread.get("dm:" + d.key)))),
    h("div", {class: "railsep"}),
    ...servers().map(s => h("button", {class: "srv" + (s === room && !homeMode ? " active" : "") + (s === room && homeMode && unread.size && [...unread.keys()].some(k => !isDM(k)) ? " unread" : ""),
      "data-tip": s, "data-tip-side": "right",
      onclick: () => { if (s !== room) goServer(s); else setHome(false); },
      oncontextmenu: e => { e.preventDefault(); serverMenu(s, e.clientX, e.clientY); }}, initials(s))),
    h("button", {class: "srv add", "data-tip": "Add a Server", "data-tip-side": "right", onclick: addServer}, icon("plus")),
    serverUnread ? null : null);
}
function renderHomeSide() {
  const list = $("#channels");
  const reqs = Object.values(friends).filter(f => f.state === "in").length;
  const rows = [
    h("div", {class: "ch home" + (view?.kind === "friends" ? " active" : ""), onclick: () => openView({ch: "@friends", kind: "friends"})},
      icon("friends"), h("span", {class: "cn"}, "Friends"), reqs ? h("span", {class: "badge"}, reqs) : null),
    h("div", {class: "cat dmcat"}, h("span", {}, "Direct Messages"),
      h("button", {class: "ib", "data-tip": "Create DM", onclick: e => pickFriends(e.currentTarget)}, icon("plus")))];
  const shown = Object.values(dms).filter(d => (!d.closed && !d.left) || view?.ch === "dm:" + d.key).sort((a, b) => (b.last || 0) - (a.last || 0));
  for (const d of shown) {
    const ch = "dm:" + d.key, n = unread.get(ch) || 0, u = dmOther(d), p = u ? friendPerson(u) : null;
    const sub = d.group ? `${Object.keys(d.members).length} Members` : subline(p);
    const inCall = room === "dm-" + d.cid && everyone().some(x => x.state?.voice === DM_VOICE);
    rows.push(h("div", {class: "ch dmrow" + (view?.ch === ch ? " active" : "") + (n && !isMuted(ch) ? " unread" : "") + (isMuted(ch) ? " muted" : ""),
                        onclick: () => openDM(d.key), oncontextmenu: e => { e.preventDefault(); dmMenu(d, e.clientX, e.clientY); }},
      dmIcon(d, true),
      h("div", {class: "dmn"}, h("div", {class: "cn"}, dmTitle(d)), sub ? h("div", {class: "dms"}, sub) : null),
      inCall ? h("span", {class: "ic incall", title: "Call in progress"}, icon("speaker")) : null,
      n && !isMuted(ch) ? h("span", {class: "badge"}, n > 99 ? "99+" : n) : null,
      h("button", {class: "ib gear", title: d.group ? "Leave Group" : "Close DM", onclick: e => { e.stopPropagation(); d.group ? leaveGroup(d) : closeDM(d); }}, icon("x"))));
  }
  if (!shown.length) rows.push(h("div", {class: "dmempty"}, h("div", {class: "skel"}), h("div", {class: "skel"}), h("div", {class: "skel"}),
    h("p", {class: "tiny"}, "Add a friend, then click Message to start a conversation.")));
  list.replaceChildren(...rows);
}
function dmMenu(d, x, y) {
  const ch = "dm:" + d.key, u = dmOther(d);
  menu([
    {label: "Mark As Read", fn: () => { unread.delete(ch); mentionCount.delete(ch); renderSidebar(); }},
    "-",
    !d.group && {label: "Profile", fn: () => showProfile(friendPerson(u), {clientX: x, clientY: y})},
    {label: "Call", fn: () => startCall(d)},
    {label: "Video Call", fn: () => startCall(d, true)},
    d.group && {label: "Add Friends to Group", fn: () => pickFriends(null, d, x, y)},
    d.group && {label: "Change Group Name", fn: () => renameGroup(d)},
    !d.group && {label: "Close DM", fn: () => closeDM(d)},
    "-",
    !d.group && friends[u]?.state !== "friend" && friends[u]?.state !== "blocked" && {label: friends[u]?.state === "out" ? "Friend Request Sent" : "Add Friend", fn: () => toast(requestFriend(u)[1], 3000)},
    !d.group && friends[u]?.state === "friend" && {label: "Remove Friend", fn: () => confirmRemoveFriend(u)},
    !d.group && (friends[u]?.state === "blocked" ? {label: "Unblock", fn: () => unblockUser(u)} : {label: "Block", fn: () => blockUser(u)}),
    {label: prefs.muted[ch] ? "Unmute Conversation" : "Mute Conversation", fn: () => { prefs.muted[ch] = !prefs.muted[ch]; savePrefs(); renderSidebar(); }},
    "-",
    prefs.devMode && {label: d.group ? "Copy Channel ID" : "Copy User ID", fn: () => copyText(d.group ? d.key.slice(2) : u)},
    d.group && {label: "Leave Group", danger: true, fn: () => leaveGroup(d)},
  ], x, y);
}
function pickFriends(anchor, d = null, x = 0, y = 0) {
  const taken = new Set(d ? [...Object.keys(d.members), myUid] : [myUid]);
  const room_ = GROUP_MAX - (d ? Object.keys(d.members).length + (d.group ? 0 : 1) : 1);
  const chosen = new Set(), all = friendUids().filter(u => !taken.has(u)).map(friendPerson).sort((a, b) => a.name.localeCompare(b.name));
  const q = h("input", {placeholder: "Type the name of a friend", maxLength: 32});
  const listEl = h("div", {class: "pickl"}), info = h("div", {class: "tiny"});
  const go = h("button", {class: "btn primary"}, d ? (d.group ? "Add" : "Create Group DM") : "Create DM");
  const draw = () => {
    const f = q.value.trim().toLowerCase();
    info.textContent = room_ - chosen.size > 0 ? `You can add ${room_ - chosen.size} more friend${room_ - chosen.size === 1 ? "" : "s"}.` : "This group is full.";
    listEl.replaceChildren(...all.filter(p => !f || p.name.toLowerCase().includes(f)).map(p => h("label", {class: "pickrow"}, avatar(p, true), h("span", {class: "nm"}, p.name),
      h("input", {type: "checkbox", checked: chosen.has(p.uid), disabled: !chosen.has(p.uid) && chosen.size >= room_,
        onchange: e => { e.target.checked ? chosen.add(p.uid) : chosen.delete(p.uid); draw(); }}))));
    if (!all.length) listEl.replaceChildren(h("p", {class: "tiny"}, "No friends to add yet. Add some on the Friends page first."));
    go.disabled = !chosen.size;
  };
  q.oninput = draw;
  go.onclick = () => {
    closePop();
    const uids = [...chosen];
    if (d?.group) addToGroup(d, uids);
    else if (d) createGroup([dmOther(d), ...uids]);
    else if (uids.length === 1) messageUser(uids[0]);
    else createGroup(uids);
  };
  draw();
  openPop(h("div", {class: "pickf"}, h("h3", {}, "Select Friends"), info, q, listEl, go), anchor, x, y);
  setTimeout(() => q.focus());
}

/* the friends page */
function renderFriends() {
  const reqs = Object.values(friends).filter(f => f.state === "in").length;
  const tab = (k, label, extra) => h("button", {class: "ftab" + (friendsTab === k ? " sel" : "") + (k === "add" ? " add" : ""),
    onclick: () => { friendsTab = k; friendQuery = ""; renderFriends(); }}, label, extra || null);
  $("#chTitle").replaceChildren(icon("friends"), h("span", {class: "ftitle"}, "Friends"), h("span", {class: "fdot"}),
    tab("online", "Online"), tab("all", "All"), tab("pending", "Pending", reqs ? h("span", {class: "badge"}, reqs) : null), tab("blocked", "Blocked"), tab("add", "Add Friend"));
  const box = $("#view");
  if (friendsTab === "add") { box.replaceChildren(addFriendPage()); return; }
  const search = h("input", {class: "fsearch", placeholder: "Search", value: friendQuery, maxLength: 32,
    oninput: e => { friendQuery = e.target.value; fillFriendList(); }});
  box.replaceChildren(h("div", {class: "friends"}, h("div", {class: "fsearchw"}, search, icon("search")), h("div", {id: "flist"})));
  fillFriendList();
}
function fillFriendList() {
  const el = $("#flist");
  if (!el) return;
  const q = friendQuery.trim().toLowerCase();
  let uids = Object.keys(friends).filter(u => {
    const s = friends[u].state;
    if (friendsTab === "online") return s === "friend" && shownStatus(friendPerson(u)) !== "offline";
    if (friendsTab === "all") return s === "friend";
    if (friendsTab === "pending") return s === "in" || s === "out";
    return s === "blocked";
  });
  if (q) uids = uids.filter(u => friendPerson(u).name.toLowerCase().includes(q));
  uids.sort((a, b) => friendPerson(a).name.localeCompare(friendPerson(b).name));
  const label = {online: "Online", all: "All Friends", pending: "Pending", blocked: "Blocked"}[friendsTab];
  if (!uids.length) {
    const msg = q ? "Nobody here matches that search." : {online: "No one's around to play with right now.", all: "No friends yet. Add some with the Add Friend button!",
      pending: "There are no pending friend requests. Here's a wave for you instead 👋", blocked: "You haven't blocked anyone."}[friendsTab];
    el.replaceChildren(h("div", {class: "fempty"}, h("div", {class: "fart"}, icon(friendsTab === "blocked" ? "lock" : "friends")), h("p", {}, msg),
      friendsTab !== "add" && !q && friendsTab !== "blocked" ? h("button", {class: "btn primary", onclick: () => { friendsTab = "add"; renderFriends(); }}, "Add Friend") : null));
    return;
  }
  el.replaceChildren(h("div", {class: "sub fcount"}, `${label} — ${uids.length}`), ...uids.map(friendRow));
}
function friendRow(u) {
  const f = friends[u], p = friendPerson(u), st = f.state, s = shownStatus(p);
  const sub = st === "in" ? "Incoming Friend Request" : st === "out" ? "Outgoing Friend Request" : st === "blocked" ? "Blocked"
    : subline(p) || (s === "offline" ? "Offline" : STATUS[s][0]);
  const act = (ic, tip, fn, cls = "") => h("button", {class: "fact " + cls, "data-tip": tip, onclick: e => { e.stopPropagation(); fn(e.currentTarget); }}, icon(ic));
  const acts = st === "friend" ? [act("chat", "Message", () => messageUser(u)), act("moreV", "More", el => { const r = el.getBoundingClientRect(); friendMenu(u, r.left, r.bottom + 4); })]
    : st === "in" ? [act("check", "Accept", () => acceptFriend(u), "ok"), act("x", "Ignore", () => removeFriend(u), "no")]
    : st === "out" ? [act("x", "Cancel", () => removeFriend(u), "no")]
    : [act("x", "Unblock", () => unblockUser(u), "no")];
  return h("div", {class: "frow", onclick: e => st === "friend" ? messageUser(u) : showProfile(p, e), oncontextmenu: e => { e.preventDefault(); friendMenu(u, e.clientX, e.clientY); }},
    avatar(p, st === "friend"), h("div", {class: "fname"}, h("div", {}, h("b", {}, p.name)), h("div", {class: "fsub"}, sub)), h("div", {class: "facts"}, ...acts));
}
function friendMenu(u, x, y) {
  const p = friendPerson(u), st = friends[u]?.state;
  menu([
    {label: "Profile", fn: () => showProfile(p, {clientX: x, clientY: y})},
    st === "friend" && {label: "Message", fn: () => messageUser(u)},
    st === "friend" && {label: "Start Voice Call", fn: () => startCall(dmFor(u))},
    st === "friend" && {label: "Start Video Call", fn: () => startCall(dmFor(u), true)},
    "-",
    st === "friend" && {label: "Remove Friend", danger: true, fn: () => confirmRemoveFriend(u)},
    st === "blocked" ? {label: "Unblock", fn: () => unblockUser(u)} : {label: "Block", danger: true, fn: () => blockUser(u, p.name)},
    prefs.devMode && "-",
    prefs.devMode && {label: "Copy User ID", fn: () => copyText(u)},
  ], x, y);
}
function addFriendPage() {
  const input = h("input", {placeholder: "You can add friends with their LiteCord username or friend code.", maxLength: 40, autocomplete: "off"});
  const btn = h("button", {class: "btn primary", disabled: true}, "Send Friend Request");
  const note = h("div", {class: "afnote"}), wrap = h("div", {class: "afbox"}, input, btn);
  input.oninput = () => { btn.disabled = !input.value.trim(); note.textContent = ""; wrap.classList.remove("ok", "bad"); note.className = "afnote"; };
  const go = async () => {
    if (!input.value.trim()) return;
    btn.disabled = true;
    const [ok, msg] = await sendFriendRequest(input.value);
    note.textContent = msg; note.className = "afnote " + (ok ? "ok" : "bad");
    wrap.classList.toggle("ok", ok); wrap.classList.toggle("bad", !ok);
    if (ok) input.value = "";
    btn.disabled = !input.value.trim();
  };
  input.onkeydown = e => { if (e.key === "Enter") go(); };
  btn.onclick = go;
  setTimeout(() => input.focus());
  const hide = streamerHides();
  const near = [...people.values()].filter(p => UIDRE.test(p.uid || "") && p.uid !== myUid && !friends[p.uid]);
  return h("div", {class: "addfriend"},
    h("h2", {}, "Add Friend"), h("p", {class: "dim"}, "You can add friends with their LiteCord username or friend code."),
    wrap, note,
    h("div", {class: "afmine"},
      h("div", {}, "Your username: ", h("b", {}, hide ? "hidden (Streamer Mode)" : prefs.username ? "@" + prefs.username : "none yet"),
        !prefs.username ? h("button", {class: "mini", onclick: () => openSettings("account")}, "Pick one") : null),
      h("div", {}, "Your friend code: ", h("code", {}, hide ? "••••••••••••••••" : myUid), h("button", {class: "mini", onclick: () => copyText(myUid, "Friend code copied!")}, "Copy"))),
    near.length && !isDMRoom() ? h("div", {class: "afnear"}, h("div", {class: "sub"}, `People in ${room}`),
      ...near.map(p => h("div", {class: "frow"}, avatar(p, true), h("div", {class: "fname"}, h("b", {}, p.name)),
        h("div", {class: "facts"}, h("button", {class: "btn primary small", onclick: e => { toast(requestFriend(p.uid, p.name)[1], 3000); e.currentTarget.disabled = true; e.currentTarget.textContent = "Sent"; }}, "Add Friend"))))) : null);
}
function activeNow() {
  const act = friendUids().map(friendPerson).filter(p => shownStatus(p) !== "offline" && (p.act || p.live));
  return h("div", {class: "anow"}, h("h2", {}, "Active Now"),
    ...(act.length ? act.map(p => h("div", {class: "acard", onclick: e => showProfile(p, e)},
      h("div", {class: "acrow"}, avatar(p, true), h("div", {class: "acn"}, h("b", {}, p.name), h("div", {class: "tiny"}, p.live ? "Sharing their screen" : p.act)),
        p.live ? h("span", {class: "live"}, "LIVE") : h("span", {class: "ic"}, icon("speaker")))))
      : [h("div", {class: "aempty"}, h("b", {}, "It's quiet for now..."),
          h("p", {class: "tiny"}, "When a friend jumps into a voice channel or starts streaming, you'll see it here."))]));
}
function renderHomeMembers() {
  const el = $("#memberList");
  el.className = "";
  if (view?.kind === "friends") { el.hidden = false; el.className = "anowcol"; el.replaceChildren(activeNow()); return; }
  const d = dmOf(view?.ch);
  if (!d || !membersOpen) { el.hidden = true; return; }
  el.hidden = false;
  if (!d.group) { el.className = "dmprof"; el.replaceChildren(profileCard(friendPerson(dmOther(d)), true)); return; }
  const mem = Object.entries(d.members).map(([u, n]) => u === myUid ? selfPerson() : {...friendPerson(u), name: friendPerson(u).name === "Someone" ? n : friendPerson(u).name});
  el.replaceChildren(h("div", {class: "sub"}, `Members — ${mem.length}`), ...mem.map(p => h("div", {class: "mrow" + (shownStatus(p) === "offline" ? " offline" : ""),
      onclick: e => showProfile(p, e), oncontextmenu: e => { e.preventDefault(); groupMemberMenu(d, p, e.clientX, e.clientY); }},
    avatar(p, true), h("div", {class: "mn"}, h("div", {class: "nm"}, p.name, p.uid === d.owner ? h("span", {class: "crown", title: "Group Owner"}, " 👑") : null),
      subline(p) ? h("div", {class: "msub"}, subline(p)) : null))));
}
function groupMemberMenu(d, p, x, y) {
  const me_ = p.uid === myUid;
  menu([
    {label: "Profile", fn: () => showProfile(p, {clientX: x, clientY: y})},
    !me_ && {label: "Message", fn: () => messageUser(p.uid, p.name)},
    !me_ && !friends[p.uid] && {label: "Add Friend", fn: () => toast(requestFriend(p.uid, p.name)[1], 3000)},
    !me_ && d.owner === myUid && "-",
    !me_ && d.owner === myUid && {label: "Remove From Group", danger: true, fn: () => kickFromGroup(d, p.uid)},
    me_ && {label: "Leave Group", danger: true, fn: () => leaveGroup(d)},
  ], x, y);
}

/* the DM page */
function dmHeader() {
  const d = dmOf(view?.ch), title = $("#chTitle");
  if (!d) return "";
  if (d.group) title.replaceChildren(dmIcon(d), h("button", {class: "gname", "data-tip": "Click to rename", onclick: () => renameGroup(d)}, dmTitle(d)));
  else {
    const p = friendPerson(dmOther(d));
    title.replaceChildren(avatar(p, true), h("span", {class: "cn", style: "cursor:pointer", onclick: e => showProfile(p, e)}, p.name),
      p.custom ? h("span", {class: "hcust"}, p.custom) : null);
  }
  $("#membersBtn").dataset.tip = d.group ? "Show Member List" : "Show User Profile";
  return d.group ? `Message ${dmTitle(d)}` : `Message @${dmTitle(d)}`;
}
function dmIntro() {
  const d = dmOf(view.ch);
  if (d.group) return h("div", {class: "dmintro"}, h("div", {class: "bigav"}, dmIcon(d)), h("h1", {}, dmTitle(d)),
    h("p", {}, "Welcome to the beginning of the ", h("b", {}, dmTitle(d)), " group."),
    h("div", {class: "introbtns"}, h("button", {class: "btn", onclick: e => pickFriends(e.currentTarget, d)}, "Invite Friends")));
  const u = dmOther(d), p = friendPerson(u), f = friends[u];
  const fb = f?.state === "friend" ? h("button", {class: "btn", onclick: () => confirmRemoveFriend(u)}, "Remove Friend")
    : f?.state === "out" ? h("button", {class: "btn", disabled: true}, "Friend Request Sent")
    : f?.state === "in" ? h("button", {class: "btn primary", onclick: () => acceptFriend(u)}, "Accept Friend Request")
    : f?.state === "blocked" ? null
    : h("button", {class: "btn primary", onclick: () => toast(requestFriend(u)[1], 3000)}, "Add Friend");
  return h("div", {class: "dmintro"}, h("div", {class: "bigav"}, avatar(p)), h("h1", {}, p.name),
    h("p", {}, "This is the beginning of your direct message history with ", h("b", {}, p.name), "."),
    h("div", {class: "introbtns"}, fb,
      f?.state === "blocked" ? h("button", {class: "btn", onclick: () => unblockUser(u)}, "Unblock") : h("button", {class: "btn", onclick: () => blockUser(u, p.name)}, "Block")));
}
function dmCallSlot() {
  const d = dmOf(view?.ch);
  return d && room === "dm-" + d.cid ? [h("div", {class: "callslot dm", "data-ch": DM_VOICE})] : [];
}

/* profiles */
function profileCard(p, panel = false) {
  const isMe = p.uid === myUid || p.id === myId, pf = isMe ? pfOut() : p.pf || {};
  const s = isMe ? myStatus() : p.offline ? "offline" : shownStatus(p);
  const bannerHue = pf.banner >= 0 ? pf.banner : pf.color >= 0 ? pf.color : hue(p.uid || p.id);
  const st = p.state || {}, call = st.voice && !isDMRoom() ? chById(st.voice) : null;
  const f = p.uid && friends[p.uid];
  const canFriend = !isMe && UIDRE.test(p.uid || "");
  return h("div", {class: "profile" + (panel ? " panel" : "")},
    h("div", {class: "pbanner", style: `background:hsl(${bannerHue} 45% 32%)`}),
    h("div", {class: "pav"}, avatar(isMe ? selfPerson() : p, !p.offline)),
    h("div", {class: "pbody"},
      h("div", {class: "pname"}, p.name),
      h("div", {class: "ptag"}, isMe && prefs.username && !streamerHides() ? "@" + prefs.username : "", pf.pronouns ? h("span", {}, (isMe && prefs.username ? " • " : "") + pf.pronouns) : null),
      (isMe ? customText() : p.custom) ? h("div", {class: "pcust"}, isMe ? customText() : p.custom) : null,
      h("div", {class: "tiny"}, s === "offline" ? "Offline" : STATUS[s][0]),
      pf.about ? h("div", {class: "psec"}, h("div", {class: "sub"}, "About Me"), h("div", {class: "tx"}, ...md(pf.about))) : null,
      p.id === leader && !isDMRoom() ? h("div", {class: "tiny"}, "🏠 Hosting this server right now") : null,
      call ? h("div", {class: "psec"}, h("div", {class: "sub"}, "In Voice"), h("div", {class: "tiny"}, `${call.name}${st.screen ? " · 🔴 Live" : ""}`)) : null,
      f?.since && f.state === "friend" ? h("div", {class: "psec"}, h("div", {class: "sub"}, "Friends Since"), h("div", {class: "tiny"}, new Date(f.since).toLocaleDateString())) : null,
      !panel && !isMe && canFriend ? h("div", {class: "pbtns"},
        h("button", {class: "btn primary", onclick: () => { closePop(); messageUser(p.uid, p.name); }}, icon("chat"), "Message"),
        f?.state === "friend" ? null : f?.state === "blocked" ? null
          : h("button", {class: "btn", disabled: f?.state === "out", onclick: e => { toast(requestFriend(p.uid, p.name)[1], 3000); e.currentTarget.disabled = true; }}, icon("addUser"), f?.state === "out" ? "Sent" : f?.state === "in" ? "Accept" : "Add Friend")) : null,
      !panel && !isMe && !p.offline && !p.friend && p.state?.voice && p.state.voice === voiceCh ? volumeControls(p) : null,
      !panel && !isMe && isChatView() && !isDM(view?.ch) ? h("button", {class: "btn", onclick: () => { closePop(); insertAtCursor($("#text"), "@" + p.name + " "); }}, "Mention") : null,
      isMe ? h("button", {class: "btn", onclick: () => { closePop(); openSettings("profiles"); }}, icon("pencil"), "Edit Profile") : null));
}
function showProfile(p, e) {
  if (!p) return;
  openPop(profileCard(p), null, e.clientX, e.clientY).classList.add("pprof");
}

/* custom status */
function customStatusDlg() {
  const d = $("#miscDlg");
  const text = h("input", {maxLength: 128, value: prefs.custom?.text || "", placeholder: "What's up?"});
  const clear = h("select", {}, ...[["0", "Don't clear"], ["1800000", "30 minutes"], ["3600000", "1 hour"], ["14400000", "4 hours"], ["today", "Today"]].map(([v, l]) => new Option(l, v)));
  clear.value = "today";
  d.replaceChildren(h("h2", {}, "Set a custom status"),
    h("div", {class: "field"}, h("label", {}, "Status"), h("div", {class: "vrow"}, text,
      h("button", {type: "button", class: "ib", title: "Emoji", onclick: e => emojiPicker(e.currentTarget, em => insertAtCursor(text, em), true)}, icon("smile")))),
    h("div", {class: "field"}, h("label", {}, "Clear after"), clear),
    h("div", {class: "dlgbtns"},
      prefs.custom?.text ? h("button", {class: "btn", onclick: () => { prefs.custom = null; savePrefs(); broadcastProfile(); d.close(); }}, "Clear Status") : null,
      h("button", {class: "btn", onclick: () => d.close()}, "Cancel"),
      h("button", {class: "btn primary", onclick: () => {
        const t = oneLine(text.value).slice(0, 128);
        const end = new Date(); end.setHours(23, 59, 59, 999);
        prefs.custom = t ? {text: t, until: clear.value === "today" ? +end : +clear.value ? Date.now() + +clear.value : 0} : null;
        savePrefs(); broadcastProfile(); d.close();
      }}, "Save")));
  d.showModal();
  text.focus();
}
function broadcastProfile() {
  if (room) send({type: "status", status: myStatus(), custom: customText(), pf: pfOut()});
  pushPresence(); renderUserBar(); renderSidebar();
}

/* quick switcher (Ctrl+K) */
function openSwitcher() {
  const dlg = $("#switchDlg");
  const input = h("input", {class: "swin", placeholder: "Where would you like to go?", maxLength: 50});
  const res = h("div", {class: "swres"});
  let items = [], sel = 0;
  const all = () => [
    ...Object.values(dms).filter(d => !d.left).sort((a, b) => (b.last || 0) - (a.last || 0)).map(d => ({label: dmTitle(d), sub: d.group ? "Group DM" : "Direct Message", ic: () => dmIcon(d), go: () => openDM(d.key)})),
    ...friendUids().filter(u => !dms["u:" + u]).map(u => ({label: friendPerson(u).name, sub: "Friend", ic: () => avatar(friendPerson(u)), go: () => messageUser(u)})),
    ...(room && !isDMRoom() ? channels.filter(c => c.kind !== "forum" || true).map(c => ({label: c.name, sub: room, ic: () => c.kind === "text" ? icon("hash") : c.kind === "voice" ? icon("speaker") : h("span", {}, ICONS[c.kind]),
      go: () => { if (homeMode) { homeMode = false; homeView = view; } openView({ch: c.id, kind: c.kind === "voice" || c.kind === "stage" ? "call" : c.kind}); }})) : []),
    ...servers().filter(s => s !== room).map(s => ({label: s, sub: "Server", ic: () => h("span", {class: "av swsrv"}, initials(s)), go: () => goServer(s)})),
  ];
  const draw = () => {
    const q = input.value.trim().replace(/^[@#*!]/, "").toLowerCase();
    items = all().filter(x => !q || x.label.toLowerCase().includes(q)).slice(0, 10);
    sel = Math.min(sel, Math.max(0, items.length - 1));
    res.replaceChildren(...(items.length ? items.map((x, i) => h("div", {class: "swrow" + (i === sel ? " sel" : ""), onmousemove: () => { if (sel !== i) { sel = i; mark(); } },
      onclick: () => { dlg.close(); x.go(); }}, x.ic(), h("span", {class: "nm"}, x.label), h("span", {class: "tiny"}, x.sub)))
      : [h("p", {class: "tiny"}, "Can't find anything like that.")]));
  };
  const mark = () => [...res.children].forEach((el, i) => el.classList.toggle("sel", i === sel));
  input.oninput = () => { sel = 0; draw(); };
  input.onkeydown = e => {
    if (e.key === "ArrowDown") { e.preventDefault(); sel = Math.min(items.length - 1, sel + 1); mark(); }
    else if (e.key === "ArrowUp") { e.preventDefault(); sel = Math.max(0, sel - 1); mark(); }
    else if (e.key === "Enter" && items[sel]) { e.preventDefault(); dlg.close(); items[sel].go(); }
  };
  dlg.replaceChildren(input, res, h("div", {class: "tiny swtip"}, "Tip: use ↑ ↓ to pick and Enter to go."));
  draw();
  dlg.showModal();
  input.focus();
}

/* tooltips: dark bubbles above buttons, like Discord's */
let tipEl = null, tipFor = null;
function showTip(el) {
  const text = el.dataset.tip || el.title;
  if (!text) return;
  if (el.title) { el.dataset.tip = el.title; el.removeAttribute("title"); }
  hideTip();
  tipFor = el;
  const side = el.dataset.tipSide || "top";
  tipEl = h("div", {class: "tip " + side}, el.dataset.tip);
  document.body.append(tipEl);
  const r = el.getBoundingClientRect(), w = tipEl.offsetWidth, ht = tipEl.offsetHeight;
  let x, y;
  if (side === "right") { x = r.right + 12; y = r.top + r.height / 2 - ht / 2; }
  else { x = r.left + r.width / 2 - w / 2; y = r.top - ht - 8; if (y < 4) { y = r.bottom + 8; tipEl.classList.replace("top", "bottom"); } }
  tipEl.style.left = Math.max(4, Math.min(x, innerWidth - w - 4)) + "px";
  tipEl.style.top = Math.max(4, y) + "px";
  tipEl.style.setProperty("--ax", (r.left + r.width / 2 - Math.max(4, Math.min(x, innerWidth - w - 4))) + "px");
}
function hideTip() { tipEl?.remove(); tipEl = null; tipFor = null; }
document.addEventListener("mouseover", e => {
  const el = e.target.closest?.("[data-tip], #app button[title], .call button[title], .ringcard button[title]");
  if (el === tipFor) return;
  if (!el || pop?.contains(el)) return hideTip();
  showTip(el);
});
document.addEventListener("mousedown", hideTip, true);
document.addEventListener("scroll", hideTip, true);

function setHeadTools() {
  const mode = !view ? "none" : view.kind === "friends" ? "friends" : isDM(view.ch) ? "dm" : view.kind === "call" ? "call" : "server";
  const show = {callBtn: mode === "dm", vcallBtn: mode === "dm", pinsBtn: mode === "dm" || (mode === "server" && isChatView()), addDmBtn: mode === "dm",
                membersBtn: mode === "dm" || mode === "server", chatBtn: mode === "call", newGroupBtn: mode === "friends", search: mode !== "friends" && mode !== "none"};
  for (const [id, on] of Object.entries(show)) $("#" + id).hidden = !on;
  $("#membersBtn").classList.toggle("active", membersOpen);
  if (mode === "server") $("#membersBtn").dataset.tip = membersOpen ? "Hide Member List" : "Show Member List";
}
function openCall() {
  if (!voiceCh) return;
  if (isDMRoom()) { const d = curDM(); if (d) openDM(d.key); return; }
  if (homeMode) { homeMode = false; homeView = view; }
  openView({ch: voiceCh, kind: "call"});
}
async function outputMenu(anchor) {
  const r = anchor.getBoundingClientRect();
  const outs = await deviceItems("audiooutput", "spk", applySink);
  const v = h("input", {type: "range", min: 0, max: 200, value: Math.round((prefs.outVol ?? 1) * 100)});
  v.oninput = () => { prefs.outVol = v.value / 100; savePrefs(); applyAudio(); };
  menu([{el: h("div", {class: "sub"}, "Output Device")}, ...outs, "-",
        {el: h("div", {class: "mrowc"}, h("label", {}, "Output Volume"), h("div", {class: "vrow"}, v))},
        {label: deaf ? "Undeafen" : "Deafen", fn: toggleDeaf},
        {label: "Voice Settings", fn: () => openSettings("voice")}], r.left, r.top);
}
