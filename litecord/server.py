#!/usr/bin/env python3
"""LiteCord - a tiny Discord-style chat server.

Text channels, voice channels, stage channels, forums, file sharing,
camera video and screen share. Uses only the Python standard library.

The server relays chat, files and small WebRTC setup messages. Voice, camera
and screen share go directly between browsers, encrypted end to end
(DTLS-SRTP), so the server never sees or hears them.

    python3 server.py            # http://localhost:8080
    python3 server.py --https    # self-signed HTTPS, for friends on your network
"""
import argparse
import json
import mimetypes
import os
import queue
import re
import secrets
import shutil
import signal
import socket
import ssl
import subprocess
import tempfile
import threading
import time
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, quote, urlparse

HERE = os.path.dirname(os.path.abspath(__file__))
UPLOADS = ""                              # temporary folder, made at start, removed at exit
HISTORY_LIMIT = 200                       # messages kept per channel or forum post
POST_LIMIT = 100                          # posts kept per forum
MAX_CHANNELS = 50
MAX_FILE = 25 * 1024 * 1024               # 25 MB per file
MAX_UPLOADS = 2 * 1024 ** 3               # 2 GB of uploads in total
KINDS = ("text", "voice", "stage", "forum")
DEFAULT_CHANNELS = (("general", "text"), ("Lounge", "voice"), ("Stage", "stage"), ("forum", "forum"))
# Files of these types are shown in the chat. Everything else is a download.
INLINE = {"image/png", "image/jpeg", "image/gif", "image/webp", "video/mp4", "video/webm",
          "audio/mpeg", "audio/ogg", "audio/wav", "audio/x-wav", "audio/webm", "audio/mp4"}

lock = threading.RLock()
clients = {}   # client id -> Client
rooms = {}     # room name -> list of channels
chans = {}     # channel or forum post id -> {"room", "kind", "obj"}
history = {}   # channel or forum post id -> list of messages
files = {}     # file id -> file info


class Client:
    def __init__(self, cid, name, room):
        self.id, self.name, self.room = cid, name, room
        self.state = {}
        self.q = queue.Queue()

    def info(self):
        return {"id": self.id, "name": self.name, "state": self.state}


def new_id():
    return secrets.token_hex(6)


def get_room(name):
    if name not in rooms:
        rooms[name] = []
        for ch_name, kind in DEFAULT_CHANNELS:
            add_channel(name, ch_name, kind)
    return rooms[name]


def add_channel(room, name, kind):
    ch = {"id": new_id(), "name": name, "kind": kind}
    if kind == "forum":
        ch["posts"] = []
    rooms[room].append(ch)
    chans[ch["id"]] = {"room": room, "kind": kind, "obj": ch}


def forget(cid):
    chans.pop(cid, None)
    history.pop(cid, None)


def lookup(c, cid, kinds):
    e = chans.get(str(cid))
    return e if e and e["room"] == c.room and e["kind"] in kinds else None


def broadcast(room, msg, exclude=None):
    with lock:
        for c in clients.values():
            if c.room == room and c is not exclude:
                c.q.put(msg)


def drop(c):
    with lock:
        if clients.get(c.id) is not c:
            return
        del clients[c.id]
        broadcast(c.room, {"type": "leave", "id": c.id})
    c.q.put(None)


def public_file(f):
    return {k: f[k] for k in ("id", "name", "size", "type", "inline", "url")}


def set_state(c, st):
    voice = st.get("voice")
    e = lookup(c, voice, ("voice", "stage"))
    voice = voice if e else None

    def stream_id(k):
        v = st.get(k)
        return str(v)[:64] if v and voice else None

    new = {"voice": voice, "muted": bool(st.get("muted")), "deaf": bool(st.get("deaf")),
           "cam": stream_id("cam"), "screen": stream_id("screen"), "hand": False, "speaker": False}
    if e and e["kind"] == "stage":
        others_speaking = any(o is not c and o.room == c.room and o.state.get("voice") == voice
                              and o.state.get("speaker") for o in clients.values())
        if c.state.get("voice") == voice:
            new["speaker"] = bool(c.state.get("speaker"))
        else:
            new["speaker"] = not others_speaking  # first one on an empty stage hosts it
        hand = bool(st.get("hand"))
        if hand and not new["speaker"] and not others_speaking:
            new["speaker"] = True  # nobody on stage: raising your hand gets you up there
        new["hand"] = hand and not new["speaker"]
        if not new["speaker"]:
            new["cam"] = new["screen"] = None
    c.state = new
    broadcast(c.room, {"type": "state", "id": c.id, "state": new})


def stage_action(c, m):
    t = clients.get(str(m.get("target", "")))
    v = c.state.get("voice")
    if not t or t.room != c.room or not v or t.state.get("voice") != v or chans.get(v, {}).get("kind") != "stage":
        return
    action = m.get("action")
    if action == "invite" and c.state.get("speaker"):
        t.state = {**t.state, "speaker": True, "hand": False}
    elif action == "audience" and (c.state.get("speaker") or t is c):
        t.state = {**t.state, "speaker": False, "cam": None, "screen": None}
    else:
        return
    broadcast(c.room, {"type": "state", "id": t.id, "state": t.state})


def handle_message(c, m):
    if not isinstance(m, dict):
        return
    kind = m.get("type")
    with lock:
        if kind == "chat":
            e = lookup(c, m.get("ch"), ("text", "post"))
            if not e:
                return
            text = str(m.get("text", "")).strip()[:2000]
            f = files.get(str(m.get("file", "")))
            if f and f["room"] != c.room:
                f = None
            if not text and not f:
                return
            ch = str(m.get("ch"))
            msg = {"type": "chat", "ch": ch, "id": c.id, "name": c.name, "text": text,
                   "ts": int(time.time() * 1000)}
            if f:
                msg["file"] = public_file(f)
            h = history.setdefault(ch, [])
            h.append(msg)
            del h[:-HISTORY_LIMIT]
            if e["kind"] == "post":
                e["obj"]["count"] += 1
                e["obj"]["last"] = msg["ts"]
            broadcast(c.room, msg)

        elif kind == "post":
            e = lookup(c, m.get("ch"), ("forum",))
            title = " ".join(str(m.get("title", "")).split())[:100]
            if not e or not title:
                return
            now = int(time.time() * 1000)
            post = {"id": new_id(), "title": title, "author": c.name, "uid": c.id,
                    "ts": now, "last": now, "count": 0}
            posts = e["obj"]["posts"]
            posts.insert(0, post)
            for old in posts[POST_LIMIT:]:
                forget(old["id"])
            del posts[POST_LIMIT:]
            chans[post["id"]] = {"room": c.room, "kind": "post", "obj": post}
            text = str(m.get("text", "")).strip()[:2000]
            history[post["id"]] = [{"type": "chat", "ch": post["id"], "id": c.id, "name": c.name,
                                    "text": text, "ts": now}] if text else []
            broadcast(c.room, {"type": "post", "ch": e["obj"]["id"], "post": post})

        elif kind == "channel":
            room = get_room(c.room)
            if m.get("action") == "create":
                k = m.get("kind")
                name = " ".join(str(m.get("name", "")).split())[:32]
                if k in ("text", "forum"):
                    name = name.lower().replace(" ", "-")
                if k not in KINDS or not name or len(room) >= MAX_CHANNELS:
                    return
                add_channel(c.room, name, k)
            elif m.get("action") == "delete":
                e = lookup(c, m.get("ch"), KINDS)
                if not e or (e["kind"] == "text" and sum(x["kind"] == "text" for x in room) <= 1):
                    return  # always keep at least one text channel
                room.remove(e["obj"])
                for p in e["obj"].get("posts", []):
                    forget(p["id"])
                forget(e["obj"]["id"])
            else:
                return
            broadcast(c.room, {"type": "channels", "channels": room})

        elif kind == "signal":
            to = clients.get(str(m.get("to", "")))
            if to and to.room == c.room:
                to.q.put({"type": "signal", "from": c.id, "data": m.get("data")})

        elif kind == "state":
            if isinstance(m.get("state"), dict):
                set_state(c, m["state"])

        elif kind == "stage":
            stage_action(c, m)


def safe_name(name):
    name = os.path.basename(str(name).replace("\\", "/"))
    name = "".join(ch for ch in name if ch.isprintable() and ch not in '"<>|:*?').strip()[:100]
    return name or "file"


class Handler(BaseHTTPRequestHandler):
    server_version = "LiteCord"

    def log_message(self, *args):
        pass  # keep the terminal quiet

    def reply(self, code, body=b"", ctype="text/plain"):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def reply_json(self, data):
        self.reply(200, json.dumps(data).encode(), "application/json")

    def do_GET(self):
        url = urlparse(self.path)
        qs = parse_qs(url.query)
        if url.path in ("/", "/index.html"):
            with open(os.path.join(HERE, "index.html"), "rb") as f:
                return self.reply(200, f.read(), "text/html; charset=utf-8")
        if url.path == "/events":
            return self.events(qs)
        if url.path == "/history":
            return self.send_history(qs)
        parts = url.path.split("/")
        if len(parts) >= 3 and parts[1] == "files" and re.fullmatch(r"[0-9a-f]{24}", parts[2]):
            return self.send_upload(parts[2])
        self.reply(404, b"not found")

    def events(self, qs):
        def arg(k):
            return (qs.get(k) or [""])[0].strip()

        cid, name, room = arg("id")[:64], arg("name")[:32], arg("room")[:32] or "lobby"
        if not cid or not name:
            return self.reply(400, b"id and name required")

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("X-Accel-Buffering", "no")
        self.end_headers()

        c = Client(cid, name, room)
        with lock:
            old = clients.get(cid)
            rejoin = old is not None and old.room == room
            if old:
                old.q.put(None)  # close the stale stream quietly
                if rejoin:
                    c.state = old.state
                else:
                    broadcast(old.room, {"type": "leave", "id": cid}, exclude=old)
            clients[cid] = c
            peers = [p.info() for p in clients.values() if p.room == room and p is not c]
            c.q.put({"type": "welcome", "peers": peers, "channels": get_room(room)})
            if not rejoin:
                broadcast(room, {"type": "join", "peer": c.info()}, exclude=c)

        try:
            while True:
                try:
                    msg = c.q.get(timeout=15)
                except queue.Empty:
                    self.wfile.write(b": ping\n\n")  # keeps the connection alive
                    self.wfile.flush()
                    continue
                if msg is None:
                    break
                with lock:
                    data = json.dumps(msg).encode()
                self.wfile.write(b"data: " + data + b"\n\n")
                self.wfile.flush()
        except OSError:
            pass
        finally:
            drop(c)

    def send_history(self, qs):
        def arg(k):
            return (qs.get(k) or [""])[0]

        with lock:
            c = clients.get(arg("id"))
            e = lookup(c, arg("ch"), ("text", "post")) if c else None
            data = json.dumps(history.get(arg("ch"), [])).encode() if e else None
        if data is None:
            return self.reply(404, b"not found")
        self.reply(200, data, "application/json")

    def send_upload(self, fid):
        f = files.get(fid)
        path = os.path.join(UPLOADS, fid)
        if not f or not os.path.exists(path):
            return self.reply(404, b"not found")
        size, start, end = f["size"], 0, f["size"] - 1
        m = re.fullmatch(r"bytes=(\d*)-(\d*)", self.headers.get("Range", ""))
        partial = bool(m and (m[1] or m[2]))
        if partial:  # lets videos seek
            if m[1]:
                start = int(m[1])
                end = min(int(m[2]), size - 1) if m[2] else size - 1
            else:
                start = max(0, size - int(m[2]))
            if start > end:
                self.send_response(416)
                self.send_header("Content-Range", f"bytes */{size}")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
        self.send_response(206 if partial else 200)
        self.send_header("Content-Type", f["type"] if f["inline"] else "application/octet-stream")
        disposition = "inline" if f["inline"] else "attachment"
        self.send_header("Content-Disposition", f"{disposition}; filename*=UTF-8''{quote(f['name'])}")
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("Accept-Ranges", "bytes")
        if partial:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        # Uploaded files can never run scripts on this site.
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Content-Security-Policy", "sandbox; default-src 'none'")
        self.send_header("Cache-Control", "private, max-age=86400")
        self.end_headers()
        with open(path, "rb") as fh:
            fh.seek(start)
            left = end - start + 1
            while left > 0:
                chunk = fh.read(min(65536, left))
                if not chunk:
                    break
                self.wfile.write(chunk)
                left -= len(chunk)

    def do_POST(self):
        url = urlparse(self.path)
        if url.path == "/upload":
            return self.receive_upload(parse_qs(url.query))

        size = int(self.headers.get("Content-Length") or 0)
        if size > 1_000_000:
            return self.reply(413, b"too big")
        try:
            data = json.loads(self.rfile.read(size) or b"{}")
        except ValueError:
            return self.reply(400, b"bad json")
        if not isinstance(data, dict):
            return self.reply(400, b"bad json")
        with lock:
            c = clients.get(str(data.get("id", "")))

        if url.path == "/leave":
            if c:
                drop(c)
        elif url.path == "/send":
            if not c:
                return self.reply(410, b"not connected")
            msgs = data.get("msgs")
            for m in (msgs if isinstance(msgs, list) else [])[:200]:
                handle_message(c, m)
        else:
            return self.reply(404, b"not found")
        self.reply(204)

    def receive_upload(self, qs):
        c = clients.get((qs.get("id") or [""])[0])
        size = int(self.headers.get("Content-Length") or 0)
        if not c:
            return self.reply(410, b"not connected")
        if size <= 0:
            return self.reply(400, b"empty file")
        if size > MAX_FILE:
            return self.reply(413, b"file is over 25 MB")
        with lock:
            if sum(f["size"] for f in files.values()) + size > MAX_UPLOADS:
                return self.reply(507, b"server upload space is full")
        name = safe_name((qs.get("name") or ["file"])[0])
        fid = secrets.token_hex(12)
        path = os.path.join(UPLOADS, fid)
        left = size
        with open(path, "wb") as out:
            while left > 0:
                chunk = self.rfile.read(min(65536, left))
                if not chunk:
                    break
                out.write(chunk)
                left -= len(chunk)
        if left:
            os.remove(path)
            return self.reply(400, b"upload interrupted")
        ctype = mimetypes.guess_type(name)[0] or "application/octet-stream"
        info = {"id": fid, "name": name, "size": size, "type": ctype, "inline": ctype in INLINE,
                "url": f"files/{fid}/{quote(name)}", "room": c.room}
        with lock:
            files[fid] = info
        self.reply_json(public_file(info))


class Server(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = os.name != "nt"  # on Windows this would let two servers share a port

    def handle_error(self, request, client_address):
        pass  # dropped connections and rejected certificates are normal


def make_cert():
    cert, key = os.path.join(HERE, "cert.pem"), os.path.join(HERE, "key.pem")
    if not (os.path.exists(cert) and os.path.exists(key)):
        print("Creating a self-signed certificate (one time only)...")
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                        "-keyout", key, "-out", cert, "-days", "3650",
                        "-subj", "/CN=litecord"], check=True, capture_output=True)
    return cert, key


def lan_ip():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("10.255.255.255", 1))  # no packet is sent
        return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"
    finally:
        s.close()


def main():
    ap = argparse.ArgumentParser(description="LiteCord chat server")
    ap.add_argument("--port", type=int, default=8080)
    ap.add_argument("--https", action="store_true", help="serve HTTPS with a self-signed certificate")
    ap.add_argument("--no-browser", action="store_true", help="don't open the browser automatically")
    args = ap.parse_args()

    # Chat history lives in memory, so uploads only need to last as long as this run.
    global UPLOADS
    UPLOADS = tempfile.mkdtemp(prefix="litecord-uploads-")

    server = None
    for port in range(args.port, args.port + 20):  # if the port is busy, try the next one
        try:
            server = Server(("0.0.0.0", port), Handler)
            break
        except OSError:
            continue
    if not server:
        raise SystemExit(f"Ports {args.port}-{args.port + 19} are all busy. Try: --port 9000")

    scheme = "http"
    if args.https:
        try:
            cert = make_cert()
        except (OSError, subprocess.CalledProcessError):
            raise SystemExit("--https needs the 'openssl' program, which wasn't found.\n"
                             "Use a Cloudflare tunnel instead (see README), or install OpenSSL.")
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ctx.load_cert_chain(*cert)
        server.socket = ctx.wrap_socket(server.socket, server_side=True,
                                        do_handshake_on_connect=False)
        scheme = "https"

    local = f"{scheme}://localhost:{port}"
    print(f"LiteCord is running!\n"
          f"  This computer: {local}\n"
          f"  Your network:  {scheme}://{lan_ip()}:{port}", flush=True)
    if not args.https:
        print("  Friends need HTTPS for voice/video/screen share: use --https or a tunnel (see README).")
    print("Keep this window open. Press Ctrl+C (or close it) to stop.", flush=True)
    if not args.no_browser:
        threading.Timer(0.5, webbrowser.open, [local]).start()

    def stop(*_):
        raise KeyboardInterrupt
    for sig in ("SIGTERM", "SIGHUP", "SIGBREAK"):  # closing the window also cleans up
        if hasattr(signal, sig):
            signal.signal(getattr(signal, sig), stop)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nStopped.")
    finally:
        shutil.rmtree(UPLOADS, ignore_errors=True)


if __name__ == "__main__":
    main()
