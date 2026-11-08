# LiteCord Desktop (Rust + GPUI)

A native LiteCord app, built the way Discord is rebuilding theirs: Rust, with
[GPUI](https://www.gpui.rs) (the UI engine from the Zed editor) drawing the interface
on the GPU. No Electron, no browser engine.

We use GPUI through [GPUI Kit](https://gpui-kit.com) (`gpui-kit` on crates.io), Longbridge's
packaged GPUI plus a component library. Zed doesn't publish GPUI releases itself.

It talks to the same Cloudflare server as the web app, so friends and DMs work between the
desktop app and browsers.

## What works

- Friends: Online / All / Pending / Blocked, Add Friend by username or friend code, accept,
  ignore, remove
- Direct messages and group DMs: send and receive, offline delivery (the mailbox holds
  messages for 30 days), typing indicator, unread badges, edits and deletes from the web app
- Live online status and custom status of friends
- Remembers the last conversation you had open, and the window size
- Discord-style title bar drawn in the theme's colors. It shows the same window buttons as the rest
  of your desktop (read from KDE's or GNOME's settings), or the ones you pick. You can switch to
  the system's title bar in Settings (on Windows that takes a restart)
- User Settings like Discord's: My Account (display name, username, friend code), Appearance
  (Light, Ash, Dark and Onyx themes, zoom, chat font size, cozy or compact messages),
  Linux/Windows Settings (window frame and buttons), Notifications, Keybinds, Language & Time
  (12/24-hour clock) and Advanced
- Zoom from 50% to 200% for big and small screens, with a suggestion for your screen.
  Ctrl + and Ctrl − change it anywhere, Ctrl 0 resets it, and Ctrl , opens Settings
- Status (Online, Idle, Do Not Disturb, Invisible) and a custom status, from your name in the
  bottom left
- Desktop notifications for new DMs (off during Do Not Disturb)
- Hover a message to copy it, or delete your own
- Link your browser account: in the web app, open **User Settings › Devices › Link the
  Desktop App**, copy the code and paste it on the desktop app's first screen

- **Calls, natively** (no browser, no web view), in the same calls as browser users:
  - Voice and video calls in DMs and group DMs: the phone and camera buttons in a DM start a
    call and ring everyone; an incoming call card lets you join or decline (with a desktop
    notification). Ringing stops after 30 seconds, like the web app
  - Mute and deafen (in the user bar and the call bar), who's speaking (green outline),
    connection problems, a call timer, and the "Voice Connected" panel above your user bar
  - Camera, screen share (on Wayland the system asks which screen or window to share), and
    watching everyone else's cameras and streams. Click a tile to make it big
  - Your microphone goes through WebRTC's echo cancellation, noise suppression and automatic
    gain, like a browser. Pick the microphone and speakers in **Settings › Voice & Video**
- **Servers**: the **+** in the server rail joins a server by name (the same names as the web app,
  e.g. `lobby`), or creates it. Text channels with chat history, and voice channels: click one to
  join, and see who's in each. Your copy of each server is saved, so it survives everyone
  leaving, like the browser's copy

Not yet: sending files (received files show their name), forums (they open in the browser),
stage moderation, per-person volume, call sounds and ringtones, and choosing a camera.

## Run it

You need Rust. On CachyOS or Arch:

```bash
sudo pacman -S rustup
rustup default stable
```

Then, from this folder:

```bash
cargo run --release
```

The first build takes a few minutes, because it compiles GPUI. Later builds take seconds.

Your account and chats are saved in `~/.local/share/litecord-desktop/state.json`. Settings
are in `prefs.json` next to it. To run a second copy with its own account (for testing), give it
another folder:

```bash
LITECORD_DATA=/tmp/lc2 cargo run --release
```

## Testing calls without a window

`callbot` runs the real call engine headless: it joins a room and its voice channel and prints
connection states, audio bytes and levels and video frames once a second. Type `mute`, `deaf`,
`cam`, `screen`, `leave`, `join`, `reopen` or `chat <text>` while it runs.

```bash
cargo run --release --bin callbot -- --room dm-<call room> --name Bot --secs 60
```

`--mic` and `--speaker` pick devices by name (`--list-devices` shows them). To keep tests silent,
point it at a PulseAudio/PipeWire null sink: `PULSE_SINK=<sink> PULSE_SOURCE=<sink>.monitor
callbot --mic pulse --speaker pulse …`. `LITECORD_FAKE_CAMERA=1` sends a moving test picture
instead of a camera, and `LC_DEBUG=1` / `LC_WEBRTC_LOG=1` print signaling and libwebrtc logs.

## How it's laid out

- `src/main.rs`: opens the window
- `src/app.rs`: the interface (rail, sidebar, Friends page, DM view)
- `src/store.rs`: friends, DMs and the mailbox protocol, matching the web app's
  `cloudflare/public/index.html`, with tests (`cargo test`)
- `src/net.rs`: the WebSocket to the mailbox, on its own thread, with automatic reconnecting
- `src/call/`: the call engine, with no UI code, on its own thread:
  - `room.rs`: the web app's room protocol (who hosts, voice states, channels, chat), with tests
  - `relay.rs`: the WebSocket to the relay room (`/relay?room=…`), big messages in pieces
  - `media.rs`: WebRTC connections, one per person in the call, with the web app's "perfect
    negotiation" and fresh starts when a connection fails
  - `audio.rs`: microphone and speakers with `cpal`, and WebRTC's audio processing
  - `video.rs`, `capture.rs`: camera (`nokhwa`), screen share (libwebrtc's desktop capturer) and
    other people's video turned into pictures for the UI
- `src/callui.rs`: the call area, call bar, voice panel and incoming call card
- `src/servers.rs`: servers in the rail, their channels, text chat and voice channels
- `src/rooms.rs`: saved copies of servers
- `src/callbot.rs`: the headless call tester
- `src/prefs.rs`: settings, themes, zoom, and detecting your desktop's window buttons
- `src/settings.rs`: the User Settings screen
- `src/titlebar.rs`: the custom title bar

## How calls work

WebRTC is Google's libwebrtc, through LiveKit's [`libwebrtc`](https://crates.io/crates/libwebrtc)
crate (its build downloads a prebuilt libwebrtc for Linux or Windows). Calls use the web app's
rooms and signaling exactly: the relay passes `{type: "signal"}` messages, every pair of people
in a call has its own connection, and voice states go through the room's host. Building needs
**clang 21 or newer** on Linux (libwebrtc ships its own libc++ that needs it), plus the ALSA and
GLib development files (`alsa-lib`, `glib2`; on Ubuntu `libasound2-dev libglib2.0-dev`).

Each call connection gets its own WebRTC factory: with one shared factory, LiveKit's audio plumbing
stops decoding new incoming audio once everyone who was there earlier has left.

## Next steps

1. Files in chats and DMs, forums
2. Call sounds and ringtones, per-person volume, stage moderation, camera picker
