# LiteCord on Cloudflare (no one has to host)

This version runs on a free Cloudflare account. Nobody keeps a server running on their
own computer.

## How it works (like torrents)

- **Cloudflare is the "tracker."** It serves the app and introduces browsers to each other.
  It doesn't store any chat and never shows anyone's IP address.
- **Everyone online is a "seeder."** Your friends' browsers connect to each other directly
  (WebRTC). Whoever has been online longest hands out the chat to newcomers (🏠 next to
  their name). If they leave, the next person takes over automatically.
- **Every device saves its own copy.** If everyone goes offline, the chat isn't lost.
  The next person to open it brings it back, and copies are merged when people reconnect,
  so nobody's messages get lost.
- Voice, video and screen share work as before: straight between browsers, encrypted.

## Put it online (one time, about 5 minutes)

You need [Node.js](https://nodejs.org) (already installed on this PC) and your Cloudflare account.

1. Open a terminal in this folder:
   ```bash
   cd ~/Downloads/litecord/cloudflare
   ```
2. Sign in to Cloudflare. A browser window opens. Click **Allow**:
   ```bash
   npx wrangler login
   ```
3. Upload it:
   ```bash
   npx wrangler deploy
   ```
   The first time, it may ask you to pick a `workers.dev` subdomain. Pick anything.
4. It prints your link, like `https://litecord.yourname.workers.dev`. That's it.
   Send it to your friends.

To update it later (after changing a file), run `npx wrangler deploy` again.

### Sharing the Cloudflare account with your group

- If your login has access to more than one Cloudflare account, `wrangler deploy` asks which
  one to use. Pick the group account.
- **Keep one shared copy of these files** (for example a GitHub repo everyone can edit).
  Each `deploy` replaces the whole app, so if two people deploy from their own copies,
  the last one wins and the other person's changes disappear.
- Anyone who can deploy can change what the app does, including what it does with messages.
  Only give the account to people you trust.
- If someone (or their AI) changes the app, test it first with `npx wrangler dev`.
  That runs it at `http://localhost:8787` without touching the live version.

## Using it

- Open the link, type your name and a **server name**, and click **Join**.
  Everyone who types the same server name is in the same server.
- **Pick a server name that's hard to guess**, like `pizza-gang-7731`. There are no
  passwords, so anyone who knows the link *and* the server name can join.
- Use **Copy link** in the app to send a link that already has the server name in it.

### Install it as an app

In **Chrome, Edge or Brave**, click **⬇ Install as an app** on the join screen (or the
install icon at the right end of the address bar). LiteCord then gets its own window,
taskbar icon and app-menu entry, with no browser tabs. That's how Discord's desktop app
works too (it's a built-in Chrome), so it runs the same or lighter.
Firefox can't install web apps on Linux or Mac, but the site still works fine in a tab.

## Good to know

- **IP addresses:** the app never shows them. For a direct connection, browsers still have
  to exchange addresses behind the scenes. Someone digging in their browser's developer
  tools could find yours. This is the same for Eaglercraft, Discord calls and any other
  peer-to-peer app.
- **Strict networks** (some school, work or mobile networks) can block direct connections.
  Chat and files then go through the Cloudflare relay instead, so they still work.
  Voice and video may not (⚠️ shows next to that person in a call).
- **Who can read what:** anyone in the server can read its chat (it's a group chat).
  Messages are encrypted in transit. When the relay is used as a fallback, they pass
  through Cloudflare.
- **Saved copies** live in each browser's storage. To delete yours, clear the site's data
  in your browser settings.
- **Limits:** 25 MB per file, 500 MB of files per server, 200 messages per channel,
  100 posts per forum, about 2 to 6 people per call.
- **Cost:** Cloudflare's free plan easily covers a friend group.

## Voice features

- **AI noise suppression** (on by default): removes fans, keyboards, pets and background
  noise. Settings → Voice & Video lets you pick RNNoise (light, recommended) or GTCRN
  (stronger, more CPU). The wave button in the voice panel turns it on or off quickly.
- **Voice Activity** with automatic or manual input sensitivity, or **Push to Talk** with any
  key or a side mouse button. Push to talk only works while the LiteCord window is focused
  (browsers can't see key presses in other programs).
- Input and output volume, plus per-person volume up to 200% (click someone's name).
- Mic test that plays your voice back so you can hear what friends hear.

## Files

- `public/index.html`: the whole app (runs in the browser)
- `src/worker.js`: the matchmaker that runs on Cloudflare
- `wrangler.jsonc`: Cloudflare settings
- `public/manifest.webmanifest`, `public/sw.js`, `public/icon*`: make it installable as an app
- `public/voice-gate.js`: voice activity and push-to-talk (runs on the audio thread)
- `public/vendor/noise/`: open-source AI noise suppression models (MIT license, see its README)


## Friends, DMs and group chats

Click the speech-bubble button at the top left. Add friends with their username or friend code
(Settings → My Account), then message them, start group chats (up to 10 people), or call them.
DMs reach friends even when they're offline: they wait in a small mailbox on Cloudflare for up to
30 days and are deleted there once picked up.

## Calls on strict networks (optional relay)

Calls go straight between browsers. Some home, school or phone networks block that, and then
people can't hear each other. Cloudflare has a free relay (TURN) for exactly this:

1. In the Cloudflare dashboard open **Realtime → TURN Server → Create**, and copy the
   **Turn Token ID** and **API Token**.
2. In this folder run:
   `npx wrangler secret put TURN_KEY_ID` (paste the token ID), then
   `npx wrangler secret put TURN_KEY_TOKEN` (paste the API token).

That's it. The relay only passes along the encrypted audio and video when a direct connection fails.
