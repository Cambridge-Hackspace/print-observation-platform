# Slicer captures

What PrusaSlicer, SuperSlicer and OrcaSlicer send to an OctoPrint, recorded
byte for byte, for `crates/pop-server/tests/slicer_uploads.rs` (#37). The
recordings are in `crates/pop-server/tests/fixtures/slicer-uploads/`.

The slicers upload only from their windows, so the requests were not taken
from the running programs. Each was sent instead by `replay.c`, which makes
the libcurl calls each slicer's `src/slic3r/Utils/Http.cpp` and
`OctoPrint.cpp` make at its pinned version:

- **Its user agent** (`PrusaSlicer/2.9.4`, `SuperSlicer/2.5.59.13`,
  `OrcaSlicer/2.5.0`) and its `X-Api-Key`.
- **Its fields, in its order:** `print` ("true" for Upload and print),
  `path` (empty: no folder), OrcaSlicer's `plateindex` for a
  `.gcode.3mf`, then the file.
- **The file as each adds it:** `curl_formadd` with `CURLFORM_STREAM`,
  `CURLFORM_CONTENTSLENGTH` and `application/octet-stream`.

The connection test, `GET /api/version`, is replayed too. Each slicer asks
it before every upload, and refuses a host whose `text` does not start
"OctoPrint".

`record.py` answers as OctoPrint 1.10 does and writes each request as it
arrived. The files uploaded are the slicer fixtures in
`crates/pop-gcode/tests/fixtures/`, and the key recorded is `CAPTURED-KEY`,
which the tests swap for a live one.

These were made on 2026-11-27 with libcurl 8.18.0 (Ubuntu 26.04). The
slicers bundle their own libcurl, so the boundary's form and the headers
libcurl adds may differ in their builds. The fields, their order, the part
headers and the file are theirs.

## Making them again

On a throwaway Linux machine with `gcc`, `libcurl4-openssl-dev` and
`python3`:

```sh
gcc -O2 -o replay replay.c -lcurl -Wno-deprecated-declarations
python3 record.py 18765 caps &
U=http://127.0.0.1:18765
./replay $U/api/version PrusaSlicer/2.9.4 CAPTURED-KEY get
./replay $U/api/files/local PrusaSlicer/2.9.4 CAPTURED-KEY post \
  print=false path= file=prusaslicer-mk4s.gcode:plate_0.2mm_PLA_MK4S_2m.gcode
```

and so on for each slicer, as the test file names them.
