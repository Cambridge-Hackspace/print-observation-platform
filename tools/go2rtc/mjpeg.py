#!/usr/bin/env python3
"""Serves one JPEG as an MJPEG camera, for tools/go2rtc/record.py:

    python3 tools/go2rtc/mjpeg.py 18985 picture.jpg
"""
import http.server, sys, time
JPG = open(sys.argv[2], "rb").read()
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "multipart/x-mixed-replace; boundary=frame")
        self.end_headers()
        try:
            while True:
                self.wfile.write(b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: %d\r\n\r\n" % len(JPG) + JPG + b"\r\n")
                time.sleep(0.2)
        except Exception:
            pass
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
