"""Records each request a slicer sends, byte for byte, answering as OctoPrint
1.10 does, for POP's tests of the slicer endpoint (#37).

    python3 record.py PORT OUTDIR

Each request is written to OUTDIR/<n>-<method>-<path>.http, exactly as it
arrived. A chunked body is kept chunked.
"""
import json, os, socket, sys

port, out = int(sys.argv[1]), sys.argv[2]
os.makedirs(out, exist_ok=True)
VERSION = {"api": "0.1", "server": "1.10.0", "text": "OctoPrint 1.10.0"}
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", port)); s.listen(5)
n = 0
while True:
    c, _ = s.accept()
    raw = b""
    while b"\r\n\r\n" not in raw:
        raw += c.recv(65536)
    head, body = raw.split(b"\r\n\r\n", 1)
    lines = head.decode("latin-1").split("\r\n")
    method, path, _ = lines[0].split(" ", 2)
    h = {l.split(":", 1)[0].lower(): l.split(":", 1)[1].strip() for l in lines[1:]}
    if h.get("expect", "").lower() == "100-continue":
        c.sendall(b"HTTP/1.1 100 Continue\r\n\r\n")
    if "content-length" in h:
        while len(body) < int(h["content-length"]):
            body += c.recv(65536)
    elif h.get("transfer-encoding", "").lower() == "chunked":
        while not body.endswith(b"0\r\n\r\n"):
            body += c.recv(65536)
    n += 1
    name = f"{n:02d}-{method}-{path.strip('/').replace('/', '_')}.http"
    open(os.path.join(out, name), "wb").write(head + b"\r\n\r\n" + body)
    if method == "GET":
        reply, code = VERSION, "200 OK"
    else:
        reply, code = {"done": True, "files": {"local": {"name": "x", "origin": "local"}}}, "201 Created"
    b = json.dumps(reply).encode()
    c.sendall(f"HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {len(b)}\r\nConnection: close\r\n\r\n".encode() + b)
    c.close()
