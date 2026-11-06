/* ---------- call view: click a voice channel to look, double-click (or Join Voice) to join ---------- */
let callFocus = null, callRaf = 0, lastTileClick = {key: null, t: 0}, callChat = localStorage.lcCallChat === "1", voiceSince = 0;
const localVids = new Map(), watching = new Set();
const callSlot = () => document.querySelector("#view .callslot");
function renderCallSoon() {
  if (callRaf || document.fullscreenElement || !callSlot()) return;
  // A short timer instead of an animation frame, so the call still updates while the window is in the background.
  callRaf = setTimeout(() => { callRaf = 0; if (!document.fullscreenElement) renderCall(); }, 30);
}
document.addEventListener("fullscreenchange", renderCallSoon);
function localVideo(stream) {
  if (!stream) return null;
  let v = localVids.get(stream.id);
  if (!v) { v = h("video", {autoplay: true, playsInline: true, muted: true}); v.srcObject = stream; localVids.set(stream.id, v); }
  return v;
}
function videoFor(p, kind) {
  if (kind === "user" && p.id !== myId && prefs.noVideo[volKey(p)]) return null;
  const sid = p.state?.[kind === "screen" ? "screen" : "cam"];
  if (!sid) return null;
  if (p.id === myId) return localVideo(kind === "screen" ? screenStream : camStream);
  for (const r of remote.values()) if (r.peer === p.id && r.kind === "video" && r.streamId === sid) return r.el;
  return null;
}
function toggleCallChat(on = !callChat) {
  callChat = on;
  localStorage.lcCallChat = on ? "1" : "0";
  if (view?.kind === "call") renderView();
}
// A server voice channel's page: the call, plus its text chat on the right if it's open (like Discord's chat in voice).
function renderCallView() {
  const c = chById(view.ch);
  if (!c) return;
  $("#memberList").hidden = true;
  $("#chTitle").replaceChildren(c.kind === "stage" ? h("span", {}, "🎙️") : icon("speaker"), h("span", {class: "cn"}, c.name));
  $("#chatBtn").classList.toggle("active", callChat);
  const parts = [h("div", {class: "callslot", "data-ch": c.id})];
  let msgs = null;
  if (callChat) {
    msgs = h("div", {id: "msgs"});
    parts.push(h("div", {class: "vchat"},
      h("div", {class: "vchead"}, icon("chat"), h("b", {class: "cn"}, c.name),
        h("button", {class: "ib", "data-tip": "Close Chat", onclick: () => toggleCallChat(false)}, icon("x"))),
      msgs, composer(`Message ${c.name}`), h("div", {id: "typing"})));
  }
  $("#view").replaceChildren(h("div", {class: "callwrap"}, ...parts));
  if (msgs) { fillMessages(msgs); msgs.scrollTop = msgs.scrollHeight; renderReplyBar(); renderTyping(); }
  renderCall();
}
function renderCall() {
  const slot = callSlot();
  if (!slot) return;
  const c = chById(slot.dataset.ch), dm = slot.classList.contains("dm");
  if (!c) { slot.replaceChildren(h("div", {class: "call" + (dm ? " dm" : "")}, h("div", {class: "cempty"}, h("p", {}, "Connecting to the call…")))); return; }
  const here = voiceCh === c.id, stage = c.kind === "stage";
  const inside = everyone().filter(p => p.state?.voice === c.id);
  const onStage = stage ? inside.filter(p => p.state.speaker) : inside, audience = stage ? inside.filter(p => !p.state.speaker) : [];
  const tiles = [];
  for (const p of onStage) {
    tiles.push({key: p.id + ":user", p, kind: "user"});
    if (p.state.screen) tiles.push({key: p.id + ":screen", p, kind: "screen"});
  }
  const d = dm ? curDM() : null;
  if (d && calling && inside.some(p => p.uid && p.uid !== myUid)) stopCalling();
  if (d && here)  // people in the DM who haven't joined yet
    for (const u of dmTargets(d)) if (!inside.some(p => p.uid === u)) tiles.push({key: "ring:" + u, p: friendPerson(u), kind: "ring"});
  if (callFocus && !tiles.some(t => t.key === callFocus)) callFocus = null;
  let body;
  if (!inside.length)
    body = h("div", {class: "cempty"}, h("div", {},
      dm && d ? h("div", {class: "cavs"}, ...dmTargets(d).slice(0, 5).map(u => avatar(friendPerson(u)))) : null,
      h("h2", {}, dm ? dmTitle(d) : c.name),
      h("p", {}, dm ? "No one's in the call yet." : "No one's here yet. Join and invite your friends!")));
  else if (callFocus) {
    const big = tiles.find(t => t.key === callFocus);
    body = h("div", {class: "cfocus"}, tileEl(big), h("div", {class: "cstrip"}, ...tiles.filter(t => t !== big).map(tileEl)));
  } else body = tiles.length ? h("div", {class: "cgrid" + (tiles.length === 1 ? " one" : tiles.length === 2 ? " two" : "")}, ...tiles.map(tileEl))
                             : h("div", {class: "cempty"}, h("p", {}, "Nobody's on stage yet."));
  const parts = [body];
  if (audience.length)
    parts.push(h("div", {class: "caud"}, h("span", {}, `Audience — ${audience.length}`),
      ...audience.map(p => h("span", {title: p.name + (p.state.hand ? " ✋ wants to speak" : ""), style: "cursor:pointer",
        onclick: e => showProfile(p, e), oncontextmenu: e => { e.preventDefault(); userMenu(p, e.clientX, e.clientY); }}, avatar(p)))));
  parts.push(callBar(c, here, dm));
  const floats = [...slot.querySelectorAll(".sbfloat")];
  slot.replaceChildren(h("div", {class: "call" + (dm ? " dm" : "")}, ...parts, ...floats));
  slot.querySelectorAll("video").forEach(v => v.play?.().catch(() => {}));
}
function tileEl(t) {
  const {p, kind} = t, st = p.state || {}, isMe = p.id === myId;
  if (kind === "ring") {
    return h("div", {class: "ctile ring" + (calling ? " ringing" : ""), style: `background:hsl(${p.pf?.color >= 0 ? p.pf.color : hue(p.uid || p.id)} 22% 18%)`,
      title: calling ? "Ringing…" : "Not in the call. Click to ring them.", onclick: () => { if (!calling) ringMembers(false); }},
      avatar(p), h("div", {class: "clabel"}, h("span", {}, p.name)), h("div", {class: "cwait"}, calling ? "Ringing…" : "Not in the call"));
  }
  if (callFocus === t.key) watching.add(t.key);
  const v = videoFor(p, kind);
  const unwatched = kind === "screen" && !isMe && !watching.has(t.key);
  const el = h("div", {class: "ctile" + (kind === "screen" ? " screen" : "") + (unwatched ? " unwatched" : ""), "data-vid": kind === "user" ? p.id : ""});
  el.onclick = () => {
    if (unwatched) { watching.add(t.key); callFocus = t.key; return renderCall(); }
    const now = Date.now(), dbl = lastTileClick.key === t.key && now - lastTileClick.t < 400;
    lastTileClick = {key: t.key, t: now};
    if (dbl) {
      if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
      else { callFocus = t.key; el.requestFullscreen?.().catch(() => {}); }
      return;
    }
    if (document.fullscreenElement) return;
    callFocus = callFocus === t.key ? null : t.key;
    renderCall();
  };
  el.oncontextmenu = e => { e.preventDefault(); userMenu(p, e.clientX, e.clientY); };
  if (v) {
    v.classList.toggle("mirror", isMe && kind === "user");
    el.append(v);
  } else {
    el.style.background = `hsl(${p.pf?.color >= 0 ? p.pf.color : hue(p.uid || p.id)} 28% 22%)`;
    el.append(avatar(p));
    if (kind === "screen") el.append(h("div", {class: "cwait"}, isMe ? "Starting your stream…" : "Loading stream…"));
  }
  if (unwatched && v) el.append(h("button", {class: "watchbtn"}, "Watch Stream"));
  if (kind === "screen") el.append(h("div", {class: "ctop"}, h("span", {class: "live"}, "LIVE")));
  el.append(h("div", {class: "clabel"}, kind === "screen" ? icon("screen") : null,
    h("span", {}, kind === "screen" ? (isMe ? "Your stream" : p.name) : p.name),
    kind === "user" ? (st.deaf ? icon("headOff") : st.muted ? icon("micOff") : null) : null));
  el.append(h("button", {class: "tmore", "data-tip": "More", onclick: e => { e.stopPropagation(); const r = e.currentTarget.getBoundingClientRect(); userMenu(p, r.left - 160, r.bottom + 4); }}, icon("more")));
  return el;
}
function callBar(c, here, dm) {
  if (!here) return h("div", {class: "cbar"}, h("div", {class: "cside"}), h("div", {class: "cmid"},
    h("button", {class: "cjoin", onclick: () => dm ? joinDMCall(false) : joinVoice(c.id)}, dm ? "Join Call" : c.kind === "stage" ? "Join Stage" : "Join Voice"),
    dm ? h("button", {class: "cjoin vid", onclick: () => joinDMCall(true)}, icon("camera"), "Join with Video") : null), h("div", {class: "cside r"}));
  const ok = canSend(), micBad = muted || deaf;
  const b = (ic, tip, fn, cls = "", disabled = false) => h("button", {class: "cbtn " + cls, "data-tip": tip, disabled, onclick: e => fn(e.currentTarget)}, icon(ic));
  const slot = callSlot();
  const pip = () => {
    const v = slot?.querySelector(".cfocus>.ctile video") || [...(slot?.querySelectorAll(".ctile video") || [])].find(x => x.videoWidth);
    if (!v) return toast("Turn on a camera or watch a stream first.", 2500);
    v.requestPictureInPicture?.().catch(err => toast("Couldn't pop out: " + err.message, 3000));
  };
  return h("div", {class: "cbar"},
    h("div", {class: "cside"}, dm ? null : b("addUser", "Invite to Server", () => copyInvite())),
    h("div", {class: "cmid"},
      h("div", {class: "cgrp"},
        h("div", {class: "csplit" + (micBad ? " bad" : "")},
          b(micBad ? "micOff" : "mic", muted ? "Unmute" : "Mute", toggleMute),
          b("chevronUp", "Audio Settings", audioMenu)),
        h("div", {class: "csplit" + (camStream ? " lit" : "")},
          b(camStream ? "camera" : "cameraOff", camStream ? "Turn Off Camera" : "Turn On Camera", toggleCamera, camStream ? "on" : "", !ok),
          b("chevronUp", "Video Settings", videoMenu))),
      h("div", {class: "cgrp"},
        b("share", screenStream ? "Stream Options" : "Share Your Screen", shareClick, screenStream ? "on" : "", !ok || !canShare),
        b("board", "Open Soundboard", soundboard),
        b("more", "More Options", moreMenu)),
      h("button", {class: "chang", "data-tip": "Disconnect", onclick: leaveVoice}, icon("hangup"))),
    h("div", {class: "cside r"},
      document.pictureInPictureEnabled ? b("popout", "Pop Out", pip) : null,
      b("fullscreen", "Full Screen", () => document.fullscreenElement ? document.exitFullscreen().catch(() => {}) : slot?.querySelector(".call")?.requestFullscreen?.().catch(() => {}))));
}
// The timer next to the voice channel you're in (like Discord's).
setInterval(() => {
  const t = voiceSince ? Math.floor((Date.now() - voiceSince) / 1000) : 0;
  const txt = `${Math.floor(t / 3600)}:${String(Math.floor(t / 60) % 60).padStart(2, "0")}:${String(t % 60).padStart(2, "0")}`;
  document.querySelectorAll(".vtimer").forEach(el => { el.textContent = txt; });
}, 1000);
