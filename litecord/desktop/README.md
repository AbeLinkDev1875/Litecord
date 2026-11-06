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
- Remembers the last conversation you had open
- Link your browser account: in the web app, open **User Settings › Devices › Link the
  Desktop App**, copy the code and paste it on the desktop app's first screen

Not yet: servers (the globe button opens them in your browser), voice and video calls, and
sending files. Received files show their name, so open them in the browser for now.

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

Your account and chats are saved in `~/.local/share/litecord-desktop/state.json`. To run a
second copy with its own account (for testing), give it another folder:

```bash
LITECORD_DATA=/tmp/lc2 cargo run --release
```

## How it's laid out

- `src/main.rs`: opens the window
- `src/app.rs`: the interface (rail, sidebar, Friends page, DM view)
- `src/store.rs`: friends, DMs and the mailbox protocol, matching the web app's
  `cloudflare/public/index.html`, with tests (`cargo test`)
- `src/net.rs`: the WebSocket to the mailbox, on its own thread, with automatic reconnecting

## Next steps

1. Servers: join the relay rooms and sync chat history with the browsers
2. Calls: voice, video and screen share. The plan, like Discord's Activities, is to use a web
   view for calls first, then move to native WebRTC in Rust
3. Settings, notifications, sounds and file uploads
