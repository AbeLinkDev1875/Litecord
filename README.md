# LiteCord

A tiny Discord-style app for you and your friends:

- **Text channels** with file sharing (images, videos, documents, up to 25 MB each)
- **Voice channels** with camera video and screen sharing
- **Stage channels**: speakers talk, the audience listens and can raise a hand
- **Forums**: posts with titles and their own reply threads
- **End-to-end encrypted** voice, video and screen share, with safety codes you can check

It's built for weak laptops: it runs in a normal browser tab (no heavy desktop
app), and the server is one Python file with nothing to install (~20 MB of RAM).

## Starting the server (anyone can do it)

Only **one person** needs to run the server. It can be anyone in the group.
Everyone else just opens a link in their browser.

1. Install **Python 3** if you don't have it: https://www.python.org/downloads/
   (On Windows, tick **"Add python.exe to PATH"** during install.)
2. Unzip LiteCord and double-click the start file for your computer:
   - **Windows:** `start-windows.bat`
   - **Mac:** `start-mac.command` (the first time, right-click it and choose **Open**)
   - **Linux:** `start-linux.sh` (or run `./start-linux.sh` in a terminal)
3. Your browser opens LiteCord. Type a name and a server name, then click **Join**.

Keep the black window open. Closing it stops the server.
If port 8080 is busy, it uses the next free one and shows which.

## Getting your friends in

Browsers only allow microphone, camera and screen sharing on **HTTPS** pages
(or on `localhost`), so your friends need an HTTPS link. Pick one:

### Option A: over the internet (easiest)

Use a free Cloudflare tunnel. No account and no router setup needed.

1. Install `cloudflared`: https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/
2. With the server running, open a second terminal and run:
   ```bash
   cloudflared tunnel --url http://localhost:8080
   ```
3. It prints a link like `https://something-random.trycloudflare.com`.
   Everyone opens that link. Use **Copy link** in the app to share a specific server.

The link changes every time you restart the tunnel.

### Option B: same Wi-Fi, or you've set up port forwarding

Start the server with `--https` (for example `python3 server.py --https`).
Send your friends the "Your network" address it prints, like `https://192.168.1.20:8080`.
The browser will warn about the certificate because it's self-made. Click
**Advanced → Proceed**. This option needs OpenSSL, which Mac and Linux already have.

## Using it

**Channels.** Click **+** next to a category to make a channel. Hover a channel
and click **×** to delete it.

**Files.** Click **+** in the message box, drag files into the chat, or paste
an image. Pictures, videos and audio play right in the chat. Other files are downloads.

**Voice channels.** Click one to join. The panel at the bottom-left has:
- **Mute / Deafen** (shortcuts: `Ctrl+Shift+M`, `Ctrl+Shift+D`)
- **Video**: your camera, 720p at 30 fps by default. Change the resolution, frame
  rate and bitrate under **Settings → Voice & Video → Camera Quality** (pick
  360p at 15 fps on a slow computer)
- **Share**: your screen, a window or a tab. Tick "share audio" in the browser's
  picker if you want sound too. Click any video to make it fullscreen.
- **Screen share limits**: pick a **resolution** (360p up to 1440p, or Full),
  type any **fps** from 1 to 60, and choose **Smooth** (games, video) or
  **Sharp text** (code, documents). You can change these while sharing.
  Lower = less CPU and internet for you and your friends.
- **Stream bitrate and codec**: in the same box, pick a **bitrate** (Auto, a
  preset, or type your own in Mbps) and a **video codec** (H.264 is lightest;
  VP9 and AV1 look sharper but use more CPU). Bitrate can be changed while live.
- **Stream sound in Firefox**: Firefox can't capture a screen's sound, so the
  share box sends a sound input along with your stream instead. It picks
  "Stereo Mix", a virtual audio cable or a Linux "Monitor of…" input for you
  if you have one, and tells you how to turn one on if you don't.
- **Simple call audio**: if you can't hear friends or they can't hear you
  (mostly in Firefox), turn this on under **Settings → Voice & Video**. It
  uses the plain microphone and standard playback, without the extras.
- **Connection & Stream Stats**: click the signal icon in the voice panel, or
  **More → Connection & Stream Stats**. For each person it shows the route
  (direct or relayed), ping, bitrates, resolution, codec, packet loss, and how
  much each stream is buffering.
- **📴** leaves the call.

**Stage channels.** The first person to join is a speaker. Everyone after that joins
the audience and can listen but not talk. Audience members click **✋ Raise hand**.
Speakers click **⬆** next to someone's name to invite them up, or **⬇** to move
them back. If the stage is empty of speakers, raising your hand puts you on stage.

**Forums.** Write a title and a message, then click **Post**. Click any post to
read and reply. Posts with the newest replies are shown first.

## Encryption

Voice, video and screen share go **straight between browsers**, encrypted with
DTLS-SRTP (the standard WebRTC encryption). The server, and the tunnel if you use
one, only pass along the setup messages. They never see or hear the call.

To check that nobody is in the middle, click **🔒** in the voice panel. It shows
a safety code for each person you're connected to. Read the codes to each other.
If they match, the call is private.

Text messages and files are **not** end-to-end encrypted. They're stored on the
computer running the server (in memory and a temporary folder), so the person
hosting could read them.

## Why it's light

- Voice and video go directly between browsers. The server only relays chat and
  small setup messages.
- You only connect to people in the same voice channel, and only once someone
  turns something on. Text-only chat uses no video connections at all.
- Video uses H.264 when it can, so laptops with a hardware video encoder barely
  use the CPU for it. Frame rate and bitrate are capped.
- No preview of your own screen share, so your browser isn't drawing it twice.

## Limits

- Chat history and files are lost when the server stops. Files are kept in a
  temporary folder that's deleted when the server closes.
- Limits: 25 MB per file, 2 GB of files in total, 200 messages per channel, 100 posts per forum.
- Best for small groups (about 2 to 6 people in one call). Everyone sends their
  own stream to everyone else in the call.
- Some strict networks (some mobile hotspots, school or work networks) block
  direct connections. You'll see ⚠️ next to that person's name. Fixing that
  needs a TURN relay server, which this doesn't include.
- There are no passwords. Anyone with the link and the server name can join,
  so don't post the link publicly.
