#!/usr/bin/env python3
"""Records a real go2rtc's answers to everything POP asks it (#66), for the
fake go2rtc that POP's tests run against.

    python3 tools/go2rtc/record.py http://127.0.0.1:18984 user 'password' \
        http://127.0.0.1:18985/stream

Point it at a fresh go2rtc started with tools/go2rtc/go2rtc.yaml (no streams,
the API behind a user name and password), and give it an MJPEG camera that
works (tools/go2rtc/mjpeg.py serves one). go2rtc writes streams added
through its API into its configuration file, so start it from a fresh copy
of tools/go2rtc/go2rtc.yaml each time. It records each answer, as POP asks
it, into crates/pop-integrations/tests/recordings/go2rtc-<version>/. A camera
that does not answer is 127.0.0.1:1, where nothing listens.
"""
import base64, json, os, sys, time, urllib.error, urllib.parse, urllib.request

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

out = os.path.join(ROOT, "crates/pop-integrations/tests/recordings", f"go2rtc-{version}")
os.makedirs(out, exist_ok=True)
with open(os.path.join(out, "recordings.json"), "w") as f:
    json.dump({"version": version, "camera": CAMERA, "dead": DEAD,
               "recorded": time.strftime("%Y-%m-%d"), "exchanges": recorded},
              f, indent=2, sort_keys=True)
    f.write("\n")
print(f"{len(recorded)} answers from go2rtc {version} -> {out}")
