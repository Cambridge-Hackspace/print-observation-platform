#!/usr/bin/env python3
"""Records a real go2rtc's answers to everything POP asks it (#66), for the
fake go2rtc that POP's tests run against.

    python3 tools/go2rtc/record.py http://127.0.0.1:18984 user 'password' \
        http://127.0.0.1:18985/stream

Point it at a fresh go2rtc started with tools/go2rtc/go2rtc.yaml (no streams,
the API behind a user name and password), and give it an MJPEG camera that
works (tools/go2rtc/mjpeg.py serves one). For the MSE stream (#67) it runs
ffmpeg itself, publishing an H.264 test picture into go2rtc's RTSP server on
127.0.0.1:18554, as tools/go2rtc/go2rtc.yaml has it. go2rtc writes streams added
through its API into its configuration file, so start it from a fresh copy
of tools/go2rtc/go2rtc.yaml each time. It records each answer, as POP asks
it, into crates/pop-integrations/tests/recordings/go2rtc-<version>/. A camera
that does not answer is 127.0.0.1:1, where nothing listens.
"""
import base64, json, os, socket, subprocess, sys, time, urllib.error, urllib.parse, urllib.request

BASE, USER, PASSWORD, CAMERA = sys.argv[1].rstrip("/"), sys.argv[2], sys.argv[3], sys.argv[4]
DEAD = "rtsp://127.0.0.1:1/none"
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
AUTH = "Basic " + base64.b64encode(f"{USER}:{PASSWORD}".encode()).decode()

recorded = []


def call(name, method, path, query=None, auth=True):
    url = BASE + path + ("?" + urllib.parse.urlencode(query) if query else "")
    req = urllib.request.Request(url, method=method)
    if auth == "wrong":
        req.add_header("Authorization", "Basic " + base64.b64encode(f"{USER}:wrong".encode()).decode())
    elif auth:
        req.add_header("Authorization", AUTH)
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            status, ctype, raw = r.status, r.headers.get("Content-Type", ""), r.read()
    except urllib.error.HTTPError as e:
        status, ctype, raw = e.code, e.headers.get("Content-Type", ""), e.read()
    if ctype.startswith("image/jpeg"):
        # A picture is recorded by what POP checks of it, not its bytes.
        body = {"jpeg_bytes": len(raw), "starts_with_soi": raw[:2] == b"\xff\xd8"}
    elif ctype.startswith("application/json"):
        body = json.loads(raw)
        if isinstance(body, dict) and "config_path" in body:
            # Where this machine keeps it; nothing POP reads.
            body["config_path"] = os.path.basename(body["config_path"])
    else:
        body = raw.decode()
    recorded.append({"name": name, "method": method, "path": path,
                     "query": query or {}, "auth": auth,
                     "status": status, "content_type": ctype, "body": body})
    return status, body


def ok(status, what):
    if status >= 300:
        sys.exit(f"{what}: {status}")


s, info = call("info", "GET", "/api")
ok(s, "info")
version = info["version"]
call("no-auth", "GET", "/api/streams", auth=False)
call("wrong-password", "GET", "/api/streams", auth="wrong")
s, streams = call("streams-empty", "GET", "/api/streams")
if streams:
    sys.exit(f"go2rtc already has streams {sorted(streams)}: start it from a fresh copy of go2rtc.yaml")
ok(call("add-good", "PUT", "/api/streams", {"name": "pop_good", "src": CAMERA})[0], "add good")
ok(call("add-dead", "PUT", "/api/streams", {"name": "pop_dead", "src": DEAD})[0], "add dead")
call("add-insecure", "PUT", "/api/streams", {"name": "pop_exec", "src": "exec:echo hi"})
call("streams-two", "GET", "/api/streams")
call("stream-one", "GET", "/api/streams", {"src": "pop_dead"})
call("stream-missing", "GET", "/api/streams", {"src": "pop_nosuch"})
call("frame-good", "GET", "/api/frame.jpeg", {"src": "pop_good"})
call("frame-dead", "GET", "/api/frame.jpeg", {"src": "pop_dead"})
call("frame-missing", "GET", "/api/frame.jpeg", {"src": "pop_nosuch"})
# Changing a stream's source is adding it again under the same name.
ok(call("replace-dead", "PUT", "/api/streams", {"name": "pop_dead", "src": DEAD + "2"})[0], "replace")
call("stream-replaced", "GET", "/api/streams", {"src": "pop_dead"})
call("delete-dead", "DELETE", "/api/streams", {"src": "pop_dead"})
call("delete-missing", "DELETE", "/api/streams", {"src": "pop_nosuch"})
call("streams-one", "GET", "/api/streams")
call("config", "GET", "/api/config")

# ---- watching (#67): the MJPEG stream and MSE over a WebSocket -------------------
# What POP asks for MSE with: the codecs a browser can play (go2rtc's own
# web player's list).
CODECS = "avc1.640029,avc1.64002A,avc1.640033,hvc1.1.6.L153.B0,mp4a.40.2,mp4a.40.5,flac,opus"


def mjpeg(name, src):
    """The start of an MJPEG stream: its status, type, and first part's head."""
    u = urllib.parse.urlsplit(BASE)
    s = socket.create_connection((u.hostname, u.port), timeout=10)
    s.sendall((f"GET /api/stream.mjpeg?src={src} HTTP/1.1\r\nHost: {u.netloc}\r\n"
               f"Authorization: {AUTH}\r\n\r\n").encode())
    data = b""
    s.settimeout(5)
    try:
        while len(data) < 400:
            c = s.recv(400 - len(data))
            if not c:
                break
            data += c
    except socket.timeout:
        pass
    s.close()
    head, _, body = data.partition(b"\r\n\r\n")
    lines = head.decode().split("\r\n")
    status = int(lines[0].split()[1])
    headers = {k.lower(): v for k, v in (l.split(": ", 1) for l in lines[1:])}
    # The first part's own head (go2rtc sends it chunked; the chunk sizes and
    # the picture's bytes are not what POP reads).
    i = body.find(b"--")
    part = body[i:].split(b"\r\n\r\n", 1)[0].decode() if i >= 0 else body.decode(errors="replace")
    recorded.append({"name": name, "method": "GET", "path": "/api/stream.mjpeg",
                     "query": {"src": src}, "auth": True, "status": status,
                     "content_type": headers.get("content-type", ""),
                     "body": {"first_part": part}})


def ws_open(path, auth=True):
    u = urllib.parse.urlsplit(BASE)
    s = socket.create_connection((u.hostname, u.port), timeout=10)
    key = base64.b64encode(os.urandom(16)).decode()
    h = (f"GET {path} HTTP/1.1\r\nHost: {u.netloc}\r\nUpgrade: websocket\r\n"
         f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n")
    if auth:
        h += f"Authorization: {AUTH}\r\n"
    s.sendall((h + "\r\n").encode())
    head = b""
    while b"\r\n\r\n" not in head:
        c = s.recv(1)
        if not c:
            break
        head += c
    return s, int(head.split(b"\r\n")[0].split()[1])


def ws_send(s, text):
    p, mask = text.encode(), os.urandom(4)
    n = len(p)
    hdr = bytes([0x81]) + (bytes([0x80 | n]) if n < 126 else bytes([0x80 | 126]) + n.to_bytes(2, "big"))
    s.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(p)))


def ws_frame(s):
    def exact(n):
        b = b""
        while len(b) < n:
            c = s.recv(n - len(b))
            if not c:
                raise EOFError
            b += c
        return b
    b1, b2 = exact(2)
    n = b2 & 0x7F
    if n == 126:
        n = int.from_bytes(exact(2), "big")
    elif n == 127:
        n = int.from_bytes(exact(8), "big")
    return b1 & 0x0F, exact(n)


def mse(name, src, binaries=0):
    """MSE as POP asks for it: go2rtc's first text message, and what the
    first `binaries` binary ones are (an MP4 box type and a size)."""
    s, status = ws_open(f"/api/ws?src={src}")
    frames = []
    if status == 101:
        ws_send(s, json.dumps({"type": "mse", "value": CODECS}))
        s.settimeout(10)
        try:
            while True:
                op, p = ws_frame(s)
                if op == 1:
                    frames.append({"text": json.loads(p)})
                elif op == 2:
                    frames.append({"binary": p[4:8].decode(errors="replace"), "bytes": len(p)})
                if len(frames) > binaries:
                    break
        except (socket.timeout, EOFError):
            pass
    s.close()
    recorded.append({"name": name, "method": "GET", "path": "/api/ws", "query": {"src": src},
                     "auth": True, "status": status, "content_type": "",
                     "body": {"sent": {"type": "mse", "value": CODECS}, "frames": frames}})


ok(call("add-mjpeg-camera", "PUT", "/api/streams", {"name": "pop_mjpeg", "src": CAMERA})[0], "mjpeg camera")
ok(call("add-dead-again", "PUT", "/api/streams", {"name": "pop_dead", "src": DEAD})[0], "dead camera")
# A stream go2rtc serves from what is published into it: an H.264 camera.
ok(call("add-h264", "PUT", "/api/streams", {"name": "pop_h264", "src": DEAD + "/placeholder"})[0], "h264")
ffmpeg = subprocess.Popen(["ffmpeg", "-hide_banner", "-loglevel", "error", "-re", "-f", "lavfi",
                           "-i", "testsrc=size=320x240:rate=10", "-c:v", "libx264", "-preset",
                           "ultrafast", "-tune", "zerolatency", "-g", "10", "-pix_fmt", "yuv420p",
                           "-f", "rtsp", "rtsp://127.0.0.1:18554/pop_h264"])
time.sleep(3)
try:
    mse("mse-h264", "pop_h264", binaries=3)
    mse("mse-mjpeg-camera", "pop_mjpeg")
    mse("mse-dead", "pop_dead")
    mse("mse-missing", "pop_nosuch")
    s, status = ws_open("/api/ws?src=pop_h264", auth=False)
    s.close()
    recorded.append({"name": "mse-no-auth", "method": "GET", "path": "/api/ws",
                     "query": {"src": "pop_h264"}, "auth": False, "status": status,
                     "content_type": "", "body": None})
    mjpeg("mjpeg-mjpeg-camera", "pop_mjpeg")
    mjpeg("mjpeg-h264", "pop_h264")
    mjpeg("mjpeg-dead", "pop_dead")
    mjpeg("mjpeg-missing", "pop_nosuch")
finally:
    ffmpeg.terminate()

out = os.path.join(ROOT, "crates/pop-integrations/tests/recordings", f"go2rtc-{version}")
os.makedirs(out, exist_ok=True)
with open(os.path.join(out, "recordings.json"), "w") as f:
    json.dump({"version": version, "camera": CAMERA, "dead": DEAD,
               "recorded": time.strftime("%Y-%m-%d"), "exchanges": recorded},
              f, indent=2, sort_keys=True)
    f.write("\n")
print(f"{len(recorded)} answers from go2rtc {version} -> {out}")
