# Cameras through go2rtc

[go2rtc](https://github.com/AlexxIT/go2rtc) serves the printers' cameras. It
runs as its own service beside POP (ADR 0001), and POP drives it through its
API: one stream for each tower printer that has a camera in BAM!. Viewers
never reach go2rtc directly; they watch through POP (#67).

## Setting it up

1. Install go2rtc 1.9.15 or later, and run it as its own service.
2. Give it a configuration that keeps its API to this machine and behind a
   password, with `local_auth` on:

   ```yaml
   api:
     listen: "127.0.0.1:1984"
     username: "pop"
     password: "<a long random password>"
     local_auth: true
   ```

   **`local_auth` matters.** Without it, go2rtc asks nothing of requests from
   its own machine, whatever the password, so any program on the box can
   change its streams. POP checks: it asks go2rtc once with no password, and if go2rtc
   answers, it says so in the log, and `/healthz` fails its `go2rtc` check
   with "go2rtc answers without its password".
3. Let go2rtc write its configuration file, and nobody else read it (mode
   `0600`, owned by go2rtc's user). go2rtc writes the streams POP adds into
   that file, and a camera's address can hold the camera's password.
4. Tell POP where go2rtc is:

   ```toml
   [go2rtc]
   url = "http://127.0.0.1:1984"
   username = "pop"
   ```

   and give the password as `POP_GO2RTC_PASSWORD`. POP refuses a user name
   without a password, or a password without a user name, and warns at start
   when it has neither.
5. In BAM!, give each printer's `camera` field the camera's address: `rtsp`,
   `rtsps`, `http` or `https` (docs/integrations/bam.md). POP refuses any
   other kind, and go2rtc refuses a source that runs a program.

## What POP does

- Each tower printer with a camera is served as the stream `pop_<BAM! id>`.
  A camera changed in BAM! is changed in go2rtc; a printer that leaves the
  tower, or loses its camera, has its stream removed.
- POP looks again whenever the printers change, and every 30 seconds. A
  go2rtc started from a fresh configuration, or edited by hand, gets POP's
  streams back at the next look.
- Streams whose names do not start `pop_` are not POP's, and POP leaves them
  alone. Any `pop_` stream no printer wants is removed.
- A Bambu printer's own RTSPS camera reaches go2rtc through POP's relay, as
  `rtsp://bblp:<access code>@127.0.0.1:<port>/streaming/live/1`, so that its
  certificate is checked (docs/printers/bambu.md). An A1's or P1's camera
  never reaches go2rtc: POP reads it.
- At each look POP asks go2rtc for a picture from every camera. Admins see
  each camera's health at `GET /api/admin/cameras`: `live`, `no_picture` (go2rtc
  has the stream but the camera gives nothing), `not_in_go2rtc`, or `unknown`
  with what went wrong. A camera's address is never shown or logged.
- `/healthz` has a `go2rtc` check: whether go2rtc answers, and lets POP in. A
  single camera that does not work does not fail it.

## Watching

Members watch a tower printer's camera through POP, signed in, from inside
the space or out (#67). Two ways, because cameras send two kinds of video:

- **MSE**, for cameras that send H.264 or H.265 (most RTSP cameras):
  a WebSocket at `/api/printers/<BAM! id>/camera/mse`. The browser sends
  `{"type":"mse","value":"<the codecs it can play>"}`; POP answers
  `{"type":"mse","value":"<MIME type>"}` and then the video as MP4
  fragments, for a `MediaSource`. Or it answers `{"type":"error","value":...}`:
  `unsupported` (the camera sends JPEG: watch it as MJPEG), `no_picture`, or
  `bad_request`.
- **MJPEG**, for cameras that send JPEG pictures:
  `/api/printers/<BAM! id>/camera/mjpeg`, which an `<img>` can show.

Read-only by construction:

- A viewer names a tower printer, never a go2rtc stream. A printer that is
  not on the tower, or has no camera, is a 404, whatever the name.
- Nothing a viewer sends reaches go2rtc but the list of codecs, and only
  once POP has checked it is one (letters, digits, `.` and `,`). Every later
  message from the viewer is dropped.
- What go2rtc says went wrong can name the camera's address; a viewer hears
  only `no_picture`.
- At most 16 viewers watch at once, across every camera, since the tower's
  connection carries each one's video. The next is told to try again.

Not done: **WebRTC.** Its video travels from go2rtc to the viewer directly,
on go2rtc's own port, not through POP, so watching from outside the space
that way would mean opening go2rtc to the internet. MSE carries the same
H.264 through POP instead, with a few seconds more delay.

## What POP was tested against

`crates/pop-integrations/tests/recordings/go2rtc-1.9.15` holds a real go2rtc
1.9.15's answers to everything POP asks it, recorded by
`tools/go2rtc/record.py` on 2026-11-17 from go2rtc's own release binary
(`go2rtc_freebsd_amd64.zip`, SHA-256
`9d7fd00fdfd2dd234508e37bb94dc9ceb3661fc90649e3e9f84bf2d123c84726`). The fake
go2rtc (`crates/pop-integrations/src/fakes/go2rtc.rs`) answers as the
recording does, and `crates/pop-integrations/tests/go2rtc.rs` plays the whole
recording against it. What the recording shows:

- Adding a stream under a name it already has replaces it. Removing one go2rtc
  does not have still answers 200.
- Streams added through the API are written into go2rtc's configuration file,
  and outlive a restart.
- A source that runs a program (`exec:`) is refused: `streams: source from
  insecure producer`.
- A picture from a camera that does not answer is a 200 with an empty body.
  The status alone does not say whether a camera works; POP checks for a JPEG.
- With `local_auth` on, a request with no password or a wrong one is a 401.
- MSE: go2rtc answers the codec list with the MIME type, then `ftyp`, then
  `moof` after `moof`. A JPEG camera's answer is `codecs not matched`; a dead
  camera's error names the address it dialled.
- MJPEG: a JPEG camera's pictures as `multipart/x-mixed-replace`; an H.264
  camera, or a dead one, is a 200 with nothing.

What needs a real camera and a browser: that a stream plays. The recording's
cameras were a still picture served as MJPEG (`tools/go2rtc/mjpeg.py`) and
ffmpeg's H.264 test picture, published into go2rtc by the recorder.

### Recording again

For a new go2rtc version, with go2rtc started from a fresh copy of
`tools/go2rtc/go2rtc.yaml` (go2rtc writes into it), ffmpeg with libx264 on
the path, and a camera to record:

```sh
python3 tools/go2rtc/mjpeg.py 18985 picture.jpg &
python3 tools/go2rtc/record.py http://127.0.0.1:18984 pop recording-only \
    http://127.0.0.1:18985/stream
```
