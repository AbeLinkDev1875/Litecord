const SETTINGS = [
  ["User Settings", [["account", "My Account"], ["profiles", "Profiles"], ["social", "Content & Social"], ["privacy", "Data & Privacy"], ["devices", "Devices"]]],
  ["App Settings", [["look", "Appearance"], ["a11y", "Accessibility"], ["voice", "Voice & Video"], ["chat", "Chat"], ["notify", "Notifications"],
                    ["keys", "Keybinds"], ["lang", "Language & Time"], ["streamer", "Streamer Mode"], ["advanced", "Advanced"]]],
  ["Activity Settings", [["activity", "Activity Privacy"]]],
];
const streamerHides = () => !!prefs.streamer?.on && prefs.streamer.hideInfo !== false;
function stopMicTest() { micTest?.stop(); micTest = null; }
let setTab = "account", setQuery = "";
function openSettings(tab = setTab) {
  const d = $("#settings");
  stopMicTest();
  setTab = tab;
  const search = h("input", {class: "setsearch", placeholder: "Search", value: setQuery, oninput: e => { setQuery = e.target.value; drawNav(); }});
  const nav = h("div", {class: "setlist"});
  const drawNav = () => {
    const q = setQuery.trim().toLowerCase();
    nav.replaceChildren(...SETTINGS.flatMap(([group, items]) => {
      const shown = items.filter(([, label]) => !q || label.toLowerCase().includes(q));
      return shown.length ? [h("div", {class: "sub"}, group), ...shown.map(([k, label]) => h("button", {class: "mi" + (k === setTab ? " sel" : ""), onclick: () => openSettings(k)}, label)), h("div", {class: "msep"})] : [];
    }),
    h("button", {class: "mi", onclick: () => openSettings("new")}, "What's New"),
    h("div", {class: "msep"}),
    h("button", {class: "mi danger", onclick: logOut}, h("span", {}, "Log Out"), icon("logout")),
    h("div", {class: "tiny setver"}, "LiteCord · friend-made, Nitro-free"));
  };
  drawNav();
  $("#setNav").replaceChildren(h("div", {class: "setnavin"}, search, nav));
  $("#setBody").replaceChildren(h("div", {class: "setin"}, ...settingsPage(tab)),
    h("div", {class: "setclose"}, h("button", {class: "closex", "data-tip": "Close", onclick: () => d.close()}, icon("x")), h("span", {}, "ESC")));
  if (!d.open) { d.showModal(); document.body.classList.add("setopen"); }
  $("#setBody").scrollTop = 0;
}
$("#settings").addEventListener("close", () => { stopMicTest(); stopCamTest(); document.body.classList.remove("setopen"); });
function radio(name, value, current, label, onpick, desc) {
  return h("label", {class: "chk rad"}, h("input", {type: "radio", name, value, checked: value === current, onchange: () => onpick(value)}),
    desc ? h("div", {}, h("div", {}, label), h("div", {class: "tiny"}, desc)) : label);
}
function check(label, on, onchange) {
  return h("label", {class: "chk"}, h("input", {type: "checkbox", checked: !!on, onchange: e => onchange(e.target.checked)}), label);
}
function toggleRow(title, desc, on, onchange) {
  return h("label", {class: "toggle"}, h("div", {}, h("b", {}, title), desc ? h("span", {}, desc) : null),
    h("input", {type: "checkbox", class: "sw", checked: !!on, onchange: e => onchange(e.target.checked)}));
}
function slider(min, max, step, value, fmt, oninput) {
  const s = h("input", {type: "range", min, max, step, value}), l = h("span", {class: "tiny"}, fmt(value));
  const paint = () => s.style.setProperty("--p", ((s.value - min) / (max - min) * 100) + "%");
  s.oninput = () => { l.textContent = fmt(+s.value); paint(); oninput(+s.value); };
  paint();
  return h("div", {class: "vrow"}, s, l);
}
const sect = (title, ...kids) => h("section", {class: "sset"}, title ? h("h3", {}, title) : null, ...kids);
const setp = (k, v) => { prefs[k] = v; savePrefs(); };
function logOut() {
  if (!confirm("Log out on this device? Your chats, friends and settings stay saved here, so logging back in with any name picks up where you left off.")) return;
  localStorage.removeItem("lcName");
  location.href = location.pathname;
}
function settingsPage(tab) {
  if (tab === "account") {
    const name = h("input", {value: myName, maxLength: 32});
    const uname = h("input", {value: prefs.username || "", maxLength: 32, placeholder: "pick-a-name", autocomplete: "off"});
    const unote = h("div", {class: "tiny"});
    const saveUser = async () => {
      const u = uname.value.trim().toLowerCase().replace(/^@/, "");
      if (u && !/^[a-z0-9_.]{2,32}$/.test(u)) { unote.textContent = "Use 2–32 lowercase letters, numbers, dots or underscores."; return; }
      unote.textContent = "Saving…";
      const r = await fetch("/api/username", {method: "POST", headers: {"content-type": "application/json"}, body: JSON.stringify({uid: myUid, key: myKey, name: u})})
        .then(r => r.json()).catch(() => null);
      if (r?.ok) { prefs.username = u; savePrefs(); unote.textContent = u ? `Saved! Friends can add you as @${u}.` : "Username removed."; }
      else unote.textContent = r?.error === "taken" ? "That username is taken. Try another one." : "Couldn't save it right now. Try again in a moment.";
    };
    const hide = streamerHides();
    return [h("h2", {}, "My Account"),
      h("div", {class: "acct"}, profileCard(selfPerson(), true)),
      sect("Display Name", h("div", {class: "vrow"}, name, h("button", {class: "btn primary", onclick: () => { changeName(name.value); toast("Name saved.", 2000); }}, "Save"))),
      sect("Username", h("p", {class: "tiny"}, "Optional. A unique name your friends can use to add you. Without one, they can use your friend code."),
        h("div", {class: "vrow"}, hide ? h("input", {value: "hidden (Streamer Mode)", disabled: true}) : uname, h("button", {class: "btn primary", disabled: hide, onclick: saveUser}, "Save")), unote),
      sect("Friend Code", h("div", {class: "vrow"}, h("code", {class: "code"}, hide ? "••••••••••••••••" : myUid),
        h("button", {class: "btn", onclick: () => copyText(myUid, "Friend code copied!")}, icon("copy"), "Copy"))),
      sect("Status", ...Object.entries(STATUS).map(([k, [label]]) => radio("status", k, prefs.status || "online", label, setStatus)),
        h("button", {class: "btn", onclick: customStatusDlg}, icon("smile"), customText() ? "Edit Custom Status" : "Set a Custom Status")),
      sect("Password and Authentication", h("p", {class: "tiny"}, "LiteCord has no accounts or passwords. You are this browser: your identity, friends, chats and settings are saved here. A secret key only this browser knows protects your mailbox.")),
      sect("Account Removal", h("p", {class: "tiny"}, "Logging out keeps everything on this device. Deleting removes your friends, DMs, servers and settings from this browser for good."),
        h("div", {class: "vrow"}, h("button", {class: "btn", onclick: logOut}, "Log Out"),
          h("button", {class: "btn danger", onclick: async () => {
            if (!confirm("Delete EVERYTHING LiteCord saved in this browser (identity, friends, DMs, servers, settings)? This can't be undone.")) return;
            clearTimeout(saveTimer); saveTimer = -1;
            await idb("rooms", "readwrite", st => st.clear()); await idb("files", "readwrite", st => st.clear());
            Object.keys(localStorage).filter(k => k.startsWith("lc")).forEach(k => localStorage.removeItem(k));
            location.href = location.pathname;
          }}, "Delete Everything")))];
  }
  if (tab === "profiles") {
    const pf = prefs.profile ||= {};
    const preview = h("div", {class: "ppreview"}, profileCard(selfPerson(), true));
    const redraw = () => { savePrefs(); preview.replaceChildren(profileCard(selfPerson(), true)); };
    const send_ = () => broadcastProfile();
    const pron = h("input", {value: pf.pronouns || "", maxLength: 40, placeholder: "Add your pronouns", oninput: e => { pf.pronouns = e.target.value; redraw(); }, onchange: send_});
    const about = h("textarea", {value: pf.about || "", maxLength: 190, rows: 4, placeholder: "Tell people a little about yourself", oninput: e => { pf.about = e.target.value; left.textContent = 190 - e.target.value.length; redraw(); }, onchange: send_});
    const left = h("span", {class: "tiny"}, 190 - (pf.about || "").length);
    const swatches = (k) => h("div", {class: "hues"}, h("button", {class: "hue none" + (!(pf[k] >= 0) ? " sel" : ""), title: "Default", onclick: () => { pf[k] = -1; redraw(); send_(); openSettings("profiles"); }}),
      ...[0, 25, 45, 90, 140, 170, 200, 225, 260, 285, 320, 340].map(v => h("button", {class: "hue" + (pf[k] === v ? " sel" : ""), style: `background:hsl(${v} 50% 45%)`, onclick: () => { pf[k] = v; redraw(); send_(); openSettings("profiles"); }})));
    return [h("h2", {}, "Profiles"), h("div", {class: "pcols"}, h("div", {},
      sect("Display Name", h("div", {class: "vrow"}, h("input", {value: myName, maxLength: 32, onchange: e => { changeName(e.target.value); redraw(); }}))),
      sect("Pronouns", pron),
      sect("Avatar Color", swatches("color")),
      sect("Banner Color", swatches("banner")),
      sect("About Me", h("p", {class: "tiny"}, "You can use markdown and links."), about, left)),
      h("div", {}, h("div", {class: "sub"}, "Preview"), preview))];
  }
  if (tab === "social") {
    return [h("h2", {}, "Content & Social"),
      sect("Social Permissions",
        toggleRow("Allow friend requests", "Anyone with your username or friend code can send you one.", prefs.friendReqs !== false, v => setp("friendReqs", v)),
        toggleRow("Allow direct messages from people who aren't your friends", "For example, people from your servers.", prefs.serverDMs !== false, v => setp("serverDMs", v))),
      sect("Content",
        toggleRow("Show images and videos in chat", "Turn off to show them as download links instead.", prefs.inlineMedia !== false, v => { setp("inlineMedia", v); refreshMessages(); }),
        h("div", {class: "field"}, h("label", {}, "Show spoiler content"),
          radio("spoil", "click", prefs.spoilers || "click", "On click", v => { setp("spoilers", v); applyLook(); }),
          radio("spoil", "always", prefs.spoilers || "click", "Always", v => { setp("spoilers", v); applyLook(); })))];
  }
  if (tab === "privacy") {
    return [h("h2", {}, "Data & Privacy"),
      sect("How LiteCord handles your data",
        h("p", {class: "tiny"}, "Server chats, voice, video and screen share go straight between browsers. Nobody's IP address is shown to anyone."),
        h("p", {class: "tiny"}, "DMs and friend requests wait in your friends' mailboxes on Cloudflare until they pick them up (at most 30 days), then they're deleted there. Each device keeps its own copy of your chats.")),
      sect("Download Your Data", h("p", {class: "tiny"}, "A file with your friends list, DMs and settings."),
        h("button", {class: "btn", onclick: () => {
          const data = {exported: new Date().toISOString(), name: myName, uid: myUid, username: prefs.username || null, friends, dms,
                        messages: Object.fromEntries([...hist].filter(([k]) => isDM(k))), prefs: {...prefs}};
          const a = h("a", {href: URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], {type: "application/json"})), download: "litecord-data.json"});
          a.click(); setTimeout(() => URL.revokeObjectURL(a.href), 5000);
        }}, "Download")),
      sect("Saved Chats", h("p", {class: "tiny"}, "Each device keeps a copy of your servers' chats so they survive when everyone goes offline."),
        h("div", {class: "vrow"},
          h("button", {class: "btn danger", onclick: async () => {
            if (!confirm("Delete every saved server chat and file on this device? Your servers stay in other people's copies.")) return;
            clearTimeout(saveTimer); saveTimer = -1;
            await idb("rooms", "readwrite", st => st.delete(IDBKeyRange.bound("", "\u0001", false, true)));
            await idb("rooms", "readwrite", st => st.delete(IDBKeyRange.lowerBound("\u0002")));
            await idb("files", "readwrite", st => st.delete(IDBKeyRange.lowerBound("\u0002")));
            toast("Deleted. Reloading…"); setTimeout(() => location.reload(), 800);
          }}, "Delete saved server chats"),
          h("button", {class: "btn danger", onclick: async () => {
            if (!confirm("Delete all your DM history on this device? Your friends keep their copies.")) return;
            await idb("rooms", "readwrite", st => st.delete(IDBKeyRange.bound("\u0001dm:", "\u0001dm:￿")));
            await idb("files", "readwrite", st => st.delete(IDBKeyRange.bound("\u0001dm/", "\u0001dm/￿")));
            for (const k of [...hist.keys()]) if (isDM(k)) hist.delete(k);
            toast("DM history deleted.", 3000); renderView();
          }}, "Delete DM history")))];
  }
  if (tab === "devices") {
    const ua = navigator.userAgent;
    const browser = /Edg\//.test(ua) ? "Edge" : /OPR\//.test(ua) ? "Opera" : /Firefox\//.test(ua) ? "Firefox" : /Chrome\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : "Browser";
    const os = /Windows/.test(ua) ? "Windows" : /Android/.test(ua) ? "Android" : /iPhone|iPad/.test(ua) ? "iOS" : /Mac OS/.test(ua) ? "macOS" : /Linux/.test(ua) ? "Linux" : "Unknown";
    const app = matchMedia("(display-mode: standalone)").matches;
    return [h("h2", {}, "Devices"),
      h("p", {class: "tiny"}, "LiteCord has no accounts, so each browser or installed app is its own identity. To use the same friends on another device, add yourself as a friend from there."),
      sect("Current Device", h("div", {class: "devrow"}, icon(os === "Android" || os === "iOS" ? "phone" : "screen"),
        h("div", {}, h("b", {}, `${os} · ${browser}${app ? " (installed app)" : ""}`), h("div", {class: "tiny"}, "This device")))),
      installPrompt ? sect("Install", h("button", {class: "btn primary", onclick: () => installPrompt?.prompt()}, "Install LiteCord as an app")) : null];
  }
  if (tab === "voice") return voicePage();
  if (tab === "chat") {
    return [h("h2", {}, "Chat"),
      sect("Display Images, Videos and Lolcats",
        toggleRow("When posted as files", "Show pictures and videos right in the chat.", prefs.inlineMedia !== false, v => { setp("inlineMedia", v); refreshMessages(); })),
      sect("Emoji",
        toggleRow("Show emoji reactions on messages", "", prefs.showReacts !== false, v => { setp("showReacts", v); refreshMessages(); }),
        toggleRow("Automatically convert emoticons in your messages to emoji", "For example, when you type :) it becomes 🙂", prefs.emoticons !== false, v => setp("emoticons", v))),
      sect("Message Box",
        toggleRow("Show a Send button", "Handy on touch screens.", !!prefs.sendBtn, v => { setp("sendBtn", v); if (isChatView()) renderView(); })),
      sect("Spoilers", radio("spoil2", "click", prefs.spoilers || "click", "Show spoilers when clicked", v => { setp("spoilers", v); applyLook(); }),
        radio("spoil2", "always", prefs.spoilers || "click", "Always show spoilers", v => { setp("spoilers", v); applyLook(); }))];
  }
  if (tab === "notify") {
    const perm = "Notification" in window ? Notification.permission : "unsupported";
    const pick = v => {
      prefs.notify = v; savePrefs();
      if (v !== "off" && perm === "default") Notification.requestPermission().then(() => openSettings("notify"));
    };
    return [h("h2", {}, "Notifications"),
      sect("Desktop Notifications",
        radio("notify", "all", prefs.notify, "All messages", pick),
        radio("notify", "mentions", prefs.notify, "Only @mentions, replies and DMs", pick),
        radio("notify", "off", prefs.notify, "Nothing", pick),
        h("p", {class: "tiny"}, perm === "denied" ? "Your browser is blocking notifications for this site. Allow them in the browser's site settings."
          : perm === "unsupported" ? "This browser doesn't support desktop notifications." : perm === "granted" ? "Desktop notifications are allowed."
          : "Pick an option and your browser will ask to allow notifications. Do Not Disturb status turns them off.")),
      sect("", toggleRow("Enable Unread Message Badge", "Shows how many messages you missed in the tab or window title.", prefs.titleBadge !== false, v => setp("titleBadge", v))),
      sect("Sounds",
        ...Object.entries(SFX_LABELS).map(([k, label]) => toggleRow(label, "", prefs.sounds[k] !== false, v => { prefs.sounds[k] = v; savePrefs(); if (v) sfx(k); })),
        h("div", {class: "field"}, h("label", {}, "Sound Effects Volume"), slider(0, 100, 5, Math.round((prefs.sfxVol ?? .6) * 100), v => v + "%", v => setp("sfxVol", v / 100))))];
  }
  if (tab === "look") {
    const swatch = (k, label, bg) => h("button", {class: "tsw" + (prefs.theme === k ? " sel" : ""), "data-tip": label, style: `background:${bg}`,
      onclick: () => { prefs.theme = k; savePrefs(); applyLook(); openSettings("look"); }}, k === "sync" ? icon("sync") : null);
    return [h("h2", {}, "Appearance"),
      h("div", {class: "lookprev"}, h("div", {class: "msg"}, h("div", {class: "gut"}, avatar(selfPerson())), h("div", {}, h("div", {class: "hd"}, h("b", {}, myName), h("time", {}, fmtTime(Date.now()))),
        h("div", {class: "tx"}, "Look at me, I'm a beautiful butterfly 🦋")))),
      sect("Theme",
        toggleRow("Same as Device Theme", "Match your device's light or dark mode.", prefs.theme === "sync", v => {
          prefs.theme = v ? "sync" : lightMQ.matches ? "light" : "dark"; savePrefs(); applyLook(); openSettings("look");
        }),
        h("div", {class: "themes"}, swatch("light", "Light", "#ffffff"), swatch("ash", "Ash", "#313338"), swatch("dark", "Dark", "#1d1e21"),
          swatch("onyx", "Onyx", "#000000"), swatch("sync", "Sync with device", "transparent"))),
      sect("Message Display",
        radio("look", "cozy", prefs.compact ? "compact" : "cozy", "Cozy", () => { prefs.compact = false; savePrefs(); applyLook(); }, "Modern, beautiful, and easy on your eyes."),
        radio("look", "compact", prefs.compact ? "compact" : "cozy", "Compact", () => { prefs.compact = true; savePrefs(); applyLook(); }, "Fit more messages on screen at one time.")),
      sect("Chat Font Scaling", slider(12, 24, 1, prefs.fs || 15, v => v + "px", v => { prefs.fs = v; savePrefs(); applyLook(); })),
      sect("Space Between Message Groups", slider(0, 24, 2, prefs.gap ?? 14, v => v + "px", v => { prefs.gap = v; savePrefs(); applyLook(); })),
      sect("Zoom Level", slider(50, 200, 10, Math.round((prefs.zoom || 1) * 100), v => v + "%", v => { prefs.zoom = v / 100; savePrefs(); applyLook(); }))];
  }
  if (tab === "a11y") {
    return [h("h2", {}, "Accessibility"),
      sect("Saturation", h("p", {class: "tiny"}, "Turn down how colorful LiteCord is."),
        slider(0, 100, 5, Math.round((prefs.saturation ?? 1) * 100), v => v + "%", v => { setp("saturation", v / 100); applyLook(); })),
      sect("Contrast", slider(80, 140, 5, Math.round((prefs.contrast ?? 1) * 100), v => v + "%", v => { setp("contrast", v / 100); applyLook(); })),
      sect("Links", toggleRow("Always underline links", "Make links stand out more from the rest of the text.", !!prefs.underline, v => { setp("underline", v); applyLook(); })),
      sect("Reduced Motion",
        toggleRow("Sync with computer", "Use your computer's reduced motion setting.", prefs.reduceSync !== false, v => { setp("reduceSync", v); applyLook(); openSettings("a11y"); }),
        toggleRow("Enable Reduced Motion", "Turns off animations and smooth scrolling.", !!prefs.reduce, v => { setp("reduce", v); applyLook(); })),
      sect("Text-to-Speech", h("div", {class: "field"}, h("label", {}, "Speech Rate"),
        slider(50, 200, 10, Math.round((prefs.ttsRate ?? 1) * 100), v => (v / 100).toFixed(1) + "×", v => setp("ttsRate", v / 100))),
        "speechSynthesis" in window ? h("button", {class: "btn", onclick: () => speak("This is what text to speech sounds like.")}, "Preview") : null)];
  }
  if (tab === "keys") return keybindsPage();
  if (tab === "lang") {
    return [h("h2", {}, "Language & Time"),
      sect("Language", h("select", {disabled: true}, new Option("English", "en")), h("p", {class: "tiny"}, "LiteCord is in English for now. Dates use your device's format.")),
      sect("Time Format",
        radio("clock", "auto", prefs.clock || "auto", "Automatic", v => { setp("clock", v); refreshMessages(); }),
        radio("clock", "12", prefs.clock || "auto", "12-hour (1:30 PM)", v => { setp("clock", v); refreshMessages(); }),
        radio("clock", "24", prefs.clock || "auto", "24-hour (13:30)", v => { setp("clock", v); refreshMessages(); }))];
  }
  if (tab === "streamer") {
    const s = prefs.streamer ||= {};
    const set = (k, v) => { s[k] = v; savePrefs(); applyLook(); };
    return [h("h2", {}, "Streamer Mode"),
      h("p", {class: "tiny"}, "Hides personal info while you stream or share your screen."),
      toggleRow("Enable Streamer Mode", "", !!s.on, v => { set("on", v); openSettings("streamer"); }),
      sect("Options",
        toggleRow("Hide Personal Information", "Hides your username and friend code.", s.hideInfo !== false, v => set("hideInfo", v)),
        toggleRow("Hide Invite Links", "Invite links aren't shown when you copy them.", s.hideLinks !== false, v => set("hideLinks", v)),
        toggleRow("Disable Sounds", "Turns off all sound effects.", !!s.noSounds, v => set("noSounds", v)),
        toggleRow("Disable Notifications", "Turns off desktop notifications.", s.noNotif !== false, v => set("noNotif", v)))];
  }
  if (tab === "advanced") {
    return [h("h2", {}, "Advanced"),
      toggleRow("Developer Mode", "Adds Copy ID to the right-click menus of people, messages and channels.", !!prefs.devMode, v => setp("devMode", v)),
      sect("Reset", h("p", {class: "tiny"}, "Puts every setting back to the default. Your friends, chats and servers stay."),
        h("button", {class: "btn danger", onclick: () => {
          if (!confirm("Reset all settings to default?")) return;
          const keep = {username: prefs.username, status: prefs.status, profile: prefs.profile, custom: prefs.custom, muted: prefs.muted};
          localStorage.lcPrefs = JSON.stringify(keep); location.reload();
        }}, "Reset Settings"))];
  }
  if (tab === "activity") {
    return [h("h2", {}, "Activity Privacy"),
      toggleRow("Share my activity with friends", "Friends see when you're in a voice channel or streaming (never which server).", prefs.shareActivity !== false, v => { setp("shareActivity", v); pushPresence(); })];
  }
  if (tab === "new") {
    return [h("h2", {}, "What's New"),
      sect("Friends, DMs and group chats", h("p", {class: "tiny"}, "Add friends by username or friend code, message them even when they're offline, start group chats with up to 10 people, and call or video call right from a DM.")),
      sect("A smoother call screen", h("p", {class: "tiny"}, "Watch Stream, pop out video, full screen, a call timer, chat next to the call, and tooltips everywhere.")),
      sect("Every free setting", h("p", {class: "tiny"}, "Profiles, custom status, keybinds, streamer mode, accessibility and more. No Nitro, ever."))];
  }
  return [];
}

/* Voice & Video: laid out like Discord's, with input profiles */
const nsEff = () => prefs.inputProfile === "studio" ? "off" : prefs.inputProfile === "isolation" ? (NOISE[prefs.nsMode] ? prefs.nsMode : "rnnoise") : prefs.nsMode;
const ecEff = () => prefs.inputProfile === "studio" ? false : prefs.inputProfile === "isolation" ? true : prefs.ec;
const agcEff = () => prefs.inputProfile === "studio" ? false : prefs.inputProfile === "isolation" ? true : prefs.agc;
let camTest = null;
function stopCamTest() { camTest?.getTracks().forEach(t => t.stop()); camTest = null; }
function voicePage() {
  const devSelect = (kind, key, label, after, ic) => {
    const sel = h("select", {onchange: () => { prefs[key] = sel.value; savePrefs(); after(); }});
    sel.add(new Option("Default", ""));
    navigator.mediaDevices?.enumerateDevices().then(list => {
      list.filter(d => d.kind === kind && d.deviceId && d.deviceId !== "default")
        .forEach((d, i) => sel.add(new Option(d.label || `${label} ${i + 1}`, d.deviceId)));
      sel.value = [...sel.options].some(o => o.value === prefs[key]) ? prefs[key] : "";
    }).catch(() => {});
    return h("div", {class: "field"}, h("label", {}, label), h("div", {class: "selw"}, icon(ic), sel));
  };
  const pct = (key, label, after) => h("div", {class: "field"}, h("label", {}, label),
    slider(0, 200, 1, Math.round((prefs[key] ?? 1) * 100), v => v + "%", v => { prefs[key] = v / 100; savePrefs(); after(); }));
  const remakeMic = () => { restartMic(); if (micTest) { stopMicTest(); startTest(); } };

  // Mic test: a row of ticks that light up with your voice, like Discord's.
  const ticks = h("div", {class: "ticks"}, ...Array.from({length: 56}, () => h("i")));
  const drawTicks = () => {
    if (!ticks.isConnected) return;
    const m = micTest, n = m ? Math.round(Math.max(0, Math.min(1, (m.level + 70) / 70)) * 56) : 0;
    [...ticks.children].forEach((t, i) => { t.className = i < n ? (m.open ? "on" : "low") : ""; });
    requestAnimationFrame(drawTicks);
  };
  requestAnimationFrame(drawTicks);
  const testBtn = h("button", {class: "btn primary mictest"}, micTest ? "Stop Testing" : "Mic Test");
  const startTest = async () => {
    try {
      const raw = await navigator.mediaDevices.getUserMedia({audio: micConstraints()});
      micTest = await buildMic(raw, {loopback: true});
      testBtn.textContent = "Stop Testing";
    } catch (err) { toast("Couldn't use your microphone: " + err.message); }
  };
  testBtn.onclick = () => { if (micTest) { stopMicTest(); testBtn.textContent = "Mic Test"; } else startTest(); };

  // Sensitivity: the live level with the threshold marker.
  const bar = h("div", {class: "lvl"}), mark = h("div", {class: "thr"});
  const meter = h("div", {class: "meter big", title: "Your voice level. It turns green when your voice is being sent."}, bar, mark);
  const pos = db => Math.max(0, Math.min(100, (db + 80) / 80 * 100)) + "%";
  const draw = () => {
    if (!meter.isConnected) return;
    const m = micTest || micProc;
    bar.style.width = m ? pos(m.level) : "0";
    bar.classList.toggle("open", !!m?.open);
    mark.hidden = !m;
    if (m) mark.style.left = pos(prefs.autoSens ? m.threshold : prefs.sens);
    requestAnimationFrame(draw);
  };
  requestAnimationFrame(draw);
  const sens = h("input", {type: "range", min: -80, max: 0, value: prefs.sens, disabled: !!prefs.autoSens});
  const sensLabel = h("span", {class: "tiny"}, prefs.sens + " dB");
  sens.oninput = () => { prefs.sens = +sens.value; sensLabel.textContent = sens.value + " dB"; savePrefs(); updateGates(); };
  const keyBtn = h("button", {class: "btn keybtn"}, keyName(prefs.pttKey));
  keyBtn.onclick = () => recordKey(keyBtn, code => { prefs.pttKey = code; savePrefs(); }, () => keyName(prefs.pttKey), true);
  const delay = slider(0, 2000, 20, prefs.pttDelay, v => v + " ms", v => setp("pttDelay", v));
  const custom = h("div", {class: "custom", hidden: prefs.inputProfile !== "custom"});
  const drawCustom = () => {
    const ptt = prefs.inputMode === "ptt";
    custom.replaceChildren(
      sect("Input Mode",
        radio("imode", "vad", prefs.inputMode, "Voice Activity", v => { setp("inputMode", v); updateGates(); drawCustom(); }),
        radio("imode", "ptt", prefs.inputMode, "Push to Talk", v => { setp("inputMode", v); updateGates(); drawCustom(); })),
      ptt ? h("div", {class: "cols"}, h("div", {class: "field"}, h("label", {}, "Shortcut"), keyBtn), h("div", {class: "field"}, h("label", {}, "Push to Talk Release Delay"), delay))
        : sect("Input Sensitivity",
            toggleRow("Automatically determine input sensitivity", "", prefs.autoSens, v => { prefs.autoSens = v; sens.disabled = v; savePrefs(); updateGates(); }),
            h("div", {class: "vrow"}, sens, sensLabel), meter,
            h("p", {class: "tiny"}, "The bar turns green when your voice is loud enough to be sent. Start a Mic Test to see it move.")),
      ptt ? h("p", {class: "tiny"}, "Browsers only see key presses while LiteCord's window is focused, so push-to-talk won't work while you're clicked into a game. Voice Activity works everywhere.") : null,
      sect("Voice Processing",
        h("div", {class: "field"}, h("label", {}, "Noise Suppression"),
          radio("nsm", "rnnoise", prefs.nsMode, "AI (RNNoise)", ns, "Removes keyboards, fans, dogs and background talk. Recommended."),
          radio("nsm", "gtcrn", prefs.nsMode, "AI, stronger (GTCRN)", ns, "Cleans up even more, uses more of your computer."),
          radio("nsm", "browser", prefs.nsMode, "Standard", ns, "Your browser's built-in noise filter."),
          radio("nsm", "off", prefs.nsMode, "None", ns)),
        toggleRow("Echo Cancellation", "", prefs.ec, v => { setp("ec", v); remakeMic(); }),
        toggleRow("Automatic Gain Control", "", prefs.agc, v => { setp("agc", v); remakeMic(); })));
  };
  const ns = v => { prefs.nsMode = v; savePrefs(); remakeMic(); renderPanel(); };
  drawCustom();
  const profile = v => { setp("inputProfile", v); custom.hidden = v !== "custom"; remakeMic(); updateGates(); renderPanel(); };
  const canPickSpeaker = "setSinkId" in HTMLMediaElement.prototype || "setSinkId" in AudioContext.prototype;

  const camBox = h("div", {class: "camprev"}, h("div", {class: "camph"}, icon("camera")));
  const camBtn = h("button", {class: "btn primary"}, "Test Video");
  camBtn.onclick = async () => {
    if (camTest) { stopCamTest(); camBox.replaceChildren(h("div", {class: "camph"}, icon("camera"))); camBtn.textContent = "Test Video"; return; }
    try {
      camTest = await navigator.mediaDevices.getUserMedia({video: {deviceId: prefs.cam ? {ideal: prefs.cam} : undefined, width: {ideal: 1280}, height: {ideal: 720}}});
      const v = h("video", {autoplay: true, muted: true, playsInline: true, class: "mirror"}); v.srcObject = camTest;
      camBox.replaceChildren(v); camBtn.textContent = "Stop Video";
    } catch (err) { toast("Couldn't use your camera: " + err.message); }
  };
  return [h("h2", {}, "Voice & Video"),
    h("h3", {class: "big"}, "Voice"),
    h("div", {class: "cols"},
      devSelect("audioinput", "mic", "Microphone", remakeMic, "mic"),
      canPickSpeaker ? devSelect("audiooutput", "spk", "Speaker", applySink, "head") : h("p", {class: "tiny"}, "This browser can't pick a speaker. It uses your system's default."),
      pct("inVol", "Microphone Volume", setInputVolume),
      pct("outVol", "Speaker Volume", applyAudio)),
    h("div", {class: "mtrow"}, testBtn, ticks),
    h("p", {class: "tiny"}, "Say something during a Mic Test and you'll hear yourself the way your friends do."),
    h("hr"),
    sect("Input Profile",
      radio("iprof", "isolation", prefs.inputProfile, "Voice Isolation", profile, "Just your voice: AI noise suppression cuts through the noise."),
      radio("iprof", "studio", prefs.inputProfile, "Studio", profile, "Pure audio: open mic with no processing."),
      radio("iprof", "custom", prefs.inputProfile, "Custom", profile, "Advanced mode: all the buttons and dials.")),
    custom,
    h("hr"),
    sect("Soundboard", h("div", {class: "field"}, h("label", {}, "Soundboard Volume"),
      slider(0, 100, 5, Math.round((prefs.sbVol ?? .8) * 100), v => v + "%", v => setp("sbVol", v / 100))),
      h("button", {class: "btn", onclick: () => playBoard(0)}, "Test Soundboard")),
    h("hr"),
    h("h3", {class: "big"}, "Video"),
    h("div", {class: "cols"}, h("div", {}, camBox, camBtn), devSelect("videoinput", "cam", "Camera", () => { restartCamera(); if (camTest) { stopCamTest(); camBtn.click(); } }, "camera")),
    h("hr"),
    sect("Screen Share", h("p", {class: "tiny"}, `Default quality: ${prefs.share.res === "max" ? "Source" : prefs.share.res + "p"}, ${prefs.share.fps} FPS. You can change it each time you click Share Your Screen.`)),
    h("p", {class: "tiny"}, "Device names show up after you've allowed the microphone or camera once.")];
}

/* Keybinds */
const DEFAULT_KEYS = {mute: "Ctrl+Shift+KeyM", deaf: "Ctrl+Shift+KeyD", cam: "", share: "", leave: "", switcher: "Ctrl+KeyK", home: "", sound: ""};
const KEYACTIONS = {
  mute: ["Toggle Mute", () => toggleMute()],
  deaf: ["Toggle Deafen", () => toggleDeaf()],
  cam: ["Toggle Camera", () => voiceCh && toggleCamera()],
  share: ["Toggle Screen Share", () => voiceCh && (screenStream ? stopShare() : openShareDialog())],
  leave: ["Disconnect From Voice", () => leaveVoice()],
  switcher: ["Open Quick Switcher", () => openSwitcher()],
  home: ["Go to Friends and DMs", () => setHome(true)],
  sound: ["Toggle All Sounds", () => { prefs.streamer ||= {}; prefs.allQuiet = !prefs.allQuiet; savePrefs(); toast(prefs.allQuiet ? "Sound effects off." : "Sound effects on.", 2000); }],
};
prefs.keys = {...DEFAULT_KEYS, ...(prefs.keys || {})};
const comboOf = e => [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Meta", e.code].filter(Boolean).join("+");
const comboName = c => !c ? "Not set" : c.split("+").map(k => k === "Backquote" ? "`" : k.replace(/^Key|^Digit/, "").replace(/(Left|Right)$/, "")).join(" + ");
function recordKey(btn, save, label, bare = false) {
  btn.textContent = "Press a key… (Esc to cancel)";
  btn.classList.add("rec");
  const done = code => {
    removeEventListener("keydown", onKey, true); removeEventListener("mousedown", onMouse, true);
    recordingKey = null; btn.classList.remove("rec");
    if (code) save(code);
    btn.textContent = label();
  };
  const onKey = e => {
    e.preventDefault(); e.stopPropagation();
    if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;  // wait for the real key
    done(e.code === "Escape" ? null : bare ? e.code : comboOf(e));
  };
  const onMouse = e => { if (e.button >= 3) { e.preventDefault(); done("Mouse" + (e.button + 1)); } };
  recordingKey = done;
  addEventListener("keydown", onKey, true); addEventListener("mousedown", onMouse, true);
}
function keybindsPage() {
  const rows = Object.entries(KEYACTIONS).map(([k, [label]]) => {
    const btn = h("button", {class: "btn keybtn"}, comboName(prefs.keys[k]));
    btn.onclick = () => recordKey(btn, c => { prefs.keys[k] = c; savePrefs(); }, () => comboName(prefs.keys[k]));
    return h("div", {class: "kbrow"}, h("b", {}, label), btn,
      h("button", {class: "ib", "data-tip": "Clear", onclick: () => { prefs.keys[k] = ""; savePrefs(); btn.textContent = comboName(""); }}, icon("x")));
  });
  return [h("h2", {}, "Keybinds"),
    h("p", {class: "tiny"}, "Click a shortcut, then press the keys you want. Keybinds only work while LiteCord's window is focused."),
    h("div", {class: "kblist"}, ...rows,
      h("div", {class: "kbrow"}, h("b", {}, "Push to Talk"), h("span", {class: "kbd"}, keyName(prefs.pttKey)), h("span", {class: "tiny"}, "Set in Voice & Video"))),
    h("button", {class: "btn", onclick: () => { prefs.keys = {...DEFAULT_KEYS}; savePrefs(); openSettings("keys"); }}, "Reset to Default"),
    sect("Default Shortcuts", h("div", {class: "kblist"},
      ...[["Esc", "Close menus, pop-ups and settings"], ["↑ in an empty message box", "Edit your last message"], ["Shift + Enter", "New line in a message"],
          ["Double-click a voice channel", "Join it"], ["Double-click a video", "Full screen"]].map(([k, d]) => h("div", {class: "kbrow"}, h("span", {class: "kbd"}, k), h("span", {class: "tiny"}, d)))))];
}
addEventListener("keydown", e => {
  if (recordingKey || e.repeat) return;
  const typing = e.target.closest?.("input, textarea, select, [contenteditable]");
  if (typing && !e.ctrlKey && !e.altKey && !e.metaKey) return;
  const c = comboOf(e);
  for (const [k, [, fn]] of Object.entries(KEYACTIONS))
    if (prefs.keys[k] && prefs.keys[k] === c) { e.preventDefault(); fn(); return; }
});
function speak(text) {
  if (!("speechSynthesis" in window)) return;
  const u = new SpeechSynthesisUtterance(text);
  u.rate = prefs.ttsRate ?? 1;
  speechSynthesis.speak(u);
}
const inputModeEff = () => prefs.inputProfile === "custom" ? prefs.inputMode : "vad";
function hour12() { return prefs.clock === "12" ? true : prefs.clock === "24" ? false : undefined; }
