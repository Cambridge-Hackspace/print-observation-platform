#!/usr/bin/env python3
"""Records a real BAM!'s answers to everything POP's BAM! client asks (#20),
for the fake BAM! that POP's tests run against.

    python3 tools/bam/record.py http://127.0.0.1:18090 admin@example.org 'password'

Point it at a fresh, empty BAM! (bam init, migrate, admin create; see
crates/pop-integrations/tests/recordings/README.md). It seeds the printers a
space would have, makes a token the way an admin would for POP (a role holding
only assets.read and events.read), and records each answer, as POP asks it,
into crates/pop-integrations/tests/recordings/<version>/. The token's secret is
never written down.
"""
import http.cookiejar, json, os, sys, urllib.error, urllib.request

BASE, EMAIL, PASSWORD = sys.argv[1].rstrip("/"), sys.argv[2], sys.argv[3]
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

jar = http.cookiejar.CookieJar()
session = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))


def call(method, path, body=None, token=None, opener=session):
    req = urllib.request.Request(BASE + path, method=method)
    req.add_header("Accept", "application/json")
    if body is not None:
        req.data = json.dumps(body).encode()
        req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with opener.open(req, timeout=30) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"null")


def ok(status, body, what):
    if status >= 300:
        sys.exit(f"{what}: {status} {body}")
    return body


ok(*call("POST", "/api/v1/auth/login", {"email": EMAIL, "password": PASSWORD}), "signing in")
ver = os.environ.get("BAM_VERSION", "v0.20.0")

# ---- what a space has: printer fields, categories, printers ---------------------
def field(key, label, vtype, options=None):
    body = {"key": key, "label": label, "validation_type": vtype}
    if options:
        body["options"] = options
    return ok(*call("POST", "/api/v1/fields", body), f"field {key}")["field"]["id"]

choices = lambda *vs: {"choices": [{"label": l, "value": v} for v, l in vs]}
fields = {
    "connection": field("connection", "Connection", "select", choices(
        ("moonraker", "Moonraker"), ("octoprint", "OctoPrint"), ("prusalink", "PrusaLink"),
        ("bambu", "Bambu LAN"), ("usb_serial", "USB serial"))),
    "address": field("address", "Address", "text"),
    "camera": field("camera", "Camera", "url"),
    "model": field("model", "Model", "text"),
    "materials": field("materials", "Materials", "text"),
    "training": field("training", "Training", "select", choices(("basic", "Basic"), ("advanced", "Advanced"))),
    "serial": field("serial", "Serial number", "text"),
}
def category(name, parent=None):
    body = {"name": name, "is_consumable": False, "unit": "", "is_lendable": False, "is_running_low": False,
            "low_stock_threshold": None}
    if parent:
        body["parent_id"] = parent
    return ok(*call("POST", "/api/v1/categories", body), f"category {name}")["category"]["id"]

printers = category("Printers")
resin = category("Resin printers", printers)
filament = category("Filament")
for key, fid in fields.items():
    ok(*call("POST", f"/api/v1/categories/{printers}/fields", {"field_id": fid, "is_required": False}),
       f"assigning {key}")

def asset(name, cats, values):
    return ok(*call("POST", "/api/v1/assets", {"name": name, "category_ids": cats, "field_values": values}),
              f"asset {name}")["asset"]["asset"]["id"]

x1 = asset("X1 Carbon", [printers], {"connection": "bambu", "address": "192.168.1.50", "model": "X1C",
                                     "serial": "01S00C123456789", "materials": "PLA, PETG",
                                     "training": "basic", "camera": "rtsps://192.168.1.50/streaming/live/1"})
voron = asset("Voron 2.4", [printers], {"connection": "moonraker", "address": "ws://voron.local:7125/websocket",
                                        "model": "Voron 2.4 R2", "materials": "PLA, PETG, ABS", "training": "advanced"})
mk4 = asset("MK4", [printers], {"connection": "prusalink", "address": "http://192.168.1.52", "model": "Original Prusa MK4"})
incomplete = asset("Ender 3", [printers], {"address": "192.168.1.53"})
old = asset("Old Prusa", [printers], {"connection": "octoprint", "address": "http://192.168.1.54"})
asset("Form 3", [resin], {"model": "Form 3"})
asset("Coral PLA", [filament], {})

# ---- the token an admin gives POP ----------------------------------------------
role = ok(*call("POST", "/api/v1/roles", {"name": "POP", "description": "Reads printers and their events",
                                          "permissions": ["assets.read", "events.read"]}), "the role")["role"]["id"]
made = ok(*call("POST", "/api/v1/tokens", {"name": "POP", "role_ids": [role]}), "the token")
token = made["secret"]

# Changes after the token exists, so the event history has them.
ok(*call("PUT", f"/api/v1/assets/{voron}", {"field_values": {"connection": "moonraker",
    "address": "ws://voron.local:7125/websocket", "model": "Voron 2.4 R2", "materials": "PLA, PETG, ABS, ASA",
    "training": "advanced"}}), "editing the Voron")
ok(*call("DELETE", f"/api/v1/assets/{old}"), "archiving the old Prusa")

# ---- what POP asks, as POP asks it ----------------------------------------------
out = os.path.join(ROOT, "crates", "pop-integrations", "tests", "recordings", ver)
os.makedirs(out, exist_ok=True)
anon = urllib.request.build_opener()
recorded = {}
def record(name, method, path, token=token):
    # One answer per request: the fake keys them by request, so a second
    # would silently replace the first.
    same = [n for n, r in recorded.items()
            if (r["method"], r["path"], r.get("_token")) == (method, path, token)]
    if same:
        sys.exit(f"{name} asks what {same[0]} asked: record each request once")
    status, body = call(method, path, token=token, opener=anon)
    recorded[name] = {"method": method, "path": path, "status": status, "body": body, "_token": token}

record("categories", "GET", "/api/v1/categories")
record("printer_fields", "GET", f"/api/v1/categories/{printers}/fields")
record("printers", "GET", f"/api/v1/assets?category_id={printers}")
record("printers_archived", "GET", f"/api/v1/assets?archived=true&category_id={printers}")
record("x1", "GET", f"/api/v1/assets/{x1}")
record("missing_asset", "GET", "/api/v1/assets/00000000-0000-4000-8000-000000000000")
record("bad_token", "GET", f"/api/v1/assets?category_id={printers}", token="bam_not_a_real_token")
record("no_token", "GET", f"/api/v1/assets?category_id={printers}", token=None)
kinds = "asset.created,asset.edited,asset.archived,asset.restored"
record("events", "GET", f"/api/v1/events?kinds={kinds}&categories={printers}&limit=500")
record("live_without_websocket", "GET", f"/api/v1/events/live?kinds={kinds}&categories={printers}")

# ---- the live WebSocket, as POP's sync opens it (#22) ---------------------------
import base64, socket, urllib.parse

def ws_open(path, token):
    """A WebSocket to BAM!, with POP's token: enough of RFC 6455 to read text."""
    u = urllib.parse.urlsplit(BASE)
    s = socket.create_connection((u.hostname, u.port or 80), timeout=10)
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall((f"GET {path} HTTP/1.1\r\nHost: {u.netloc}\r\nUpgrade: websocket\r\n"
               f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
               f"Authorization: Bearer {token}\r\n\r\n").encode())
    head = b""
    while b"\r\n\r\n" not in head:
        head += s.recv(1)
    if b" 101 " not in head.split(b"\r\n")[0]:
        sys.exit(f"the live connection was refused: {head!r}")
    return s

def ws_text(s):
    """The next text frame (pings and the like skipped)."""
    def exact(n):
        b = b""
        while len(b) < n:
            chunk = s.recv(n - len(b))
            if not chunk:
                sys.exit("the live connection closed")
            b += chunk
        return b
    while True:
        b1, b2 = exact(2)
        n = b2 & 0x7F
        if n == 126:
            n = int.from_bytes(exact(2), "big")
        elif n == 127:
            n = int.from_bytes(exact(8), "big")
        payload = exact(n)
        if b1 & 0x0F == 1:
            return json.loads(payload)

live_path = f"/api/v1/events/live?kinds={kinds}&categories={printers}"
ws = ws_open(live_path, token)
frames = [ws_text(ws)]  # hello
ok(*call("PUT", f"/api/v1/assets/{mk4}", {"field_values": {"connection": "prusalink",
    "address": "http://192.168.1.62", "model": "Original Prusa MK4"}}), "editing the MK4 while live")
frames.append(ws_text(ws))
ok(*call("POST", f"/api/v1/assets/{old}/restore"), "restoring the old Prusa while live")
frames.append(ws_text(ws))
ws.close()
recorded["live"] = {"method": "GET", "path": live_path, "status": 101, "body": {"frames": frames}}

ids = {"printers": printers, "resin": resin, "filament": filament, "x1": x1, "voron": voron, "mk4": mk4,
       "incomplete": incomplete, "old": old}
for r in recorded.values():
    r.pop("_token", None)  # never write a token down
with open(os.path.join(out, "recordings.json"), "w") as f:
    json.dump({"bam": ver, "ids": ids, "recorded": recorded}, f, indent=1, sort_keys=True)
    f.write("\n")
assert token not in open(os.path.join(out, "recordings.json")).read(), "the token's secret reached the recording"
print(f"recorded {len(recorded)} answers from BAM! {ver} into {out}")
