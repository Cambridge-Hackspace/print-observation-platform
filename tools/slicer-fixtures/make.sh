#!/usr/bin/env bash
# Make pop-gcode's test fixtures with the real slicers: each slices the same
# plate (box.py) with a vendor's own presets, and what it writes is kept
# whole. Every file is POP's own output, so it carries no one else's license,
# and each names the version that wrote it.
#
# Runs as root on a throwaway Ubuntu 26.04 machine (a reaper guest), since it
# installs packages:
#
#   tools/slicer-fixtures/make.sh WORKDIR OUTDIR
#
# Then copy OUTDIR's files into crates/pop-gcode/tests/fixtures/. The
# fixtures' README.md says what each one is and how it was made.

set -euo pipefail

work=${1:?WORKDIR}
out=${2:?OUTDIR}
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$work" "$out"
cd "$work"

# Each AppImage pinned by version and hash. PrusaSlicer publishes no Linux
# build of its own (Flathub only), so it comes from Ubuntu's package.
fetch() { # name url sha256
	if ! echo "$3  $1" | sha256sum -c --quiet 2>/dev/null; then
		curl -sSfLo "$1" "$2"
		echo "$3  $1" | sha256sum -c --quiet
	fi
	chmod +x "$1"
	[ -d "x/$1" ] || (mkdir -p "x/$1" && cd "x/$1" && "../../$1" --appimage-extract >/dev/null)
}
gh=https://github.com
fetch orca.AppImage "$gh/SoftFever/OrcaSlicer/releases/download/v2.5.0/OrcaSlicer_Linux_AppImage_Ubuntu2404_V2.5.0.AppImage" \
	d13eb0a9cd618ed080ab6b82c64c0a60c4501aac91e88d5ac2c3b526fb141a7c
fetch superslicer.AppImage "$gh/supermerill/SuperSlicer/releases/download/2.5.59.13/SuperSlicer-ubuntu_20.04-2.5.59.13.AppImage" \
	332dfbb7046fc12c1936900be2a85ce0f13031b381edf266f8c6d08d94730da4
fetch bambu.AppImage "$gh/bambulab/BambuStudio/releases/download/v02.09.01.70/BambuStudio_ubu24-v02.09.01.70-20261103183404.AppImage" \
	668dabb1be53c758189ed6bc89935285dea5e0af46216546b1bafd6882e9aff0
fetch cura.AppImage "$gh/Ultimaker/Cura/releases/download/5.14.0/UltiMaker-Cura-5.14.0-linux-X64.AppImage" \
	988a2a67cce7ee7b9b9a843aad65ec4754d2ceb0a72e70eeaf884b995b75990f

export DEBIAN_FRONTEND=noninteractive
apt-get update -q >/dev/null
apt-get install -y -q prusa-slicer=2.9.4+dfsg-4 xvfb xauth libopengl0 libegl1 libgl1 libglu1-mesa \
	libwebkit2gtk-4.1-0 libgstreamer-plugins-base1.0-0 >/dev/null

# OrcaSlicer ships its presets packed; the plain ones are in its source at
# the same tag.
if [ ! -d orca-src ]; then
	curl -sSfLo orca-src.tar.gz https://codeload.github.com/SoftFever/OrcaSlicer/tar.gz/refs/tags/v2.5.0
	mkdir orca-src
	tar -xzf orca-src.tar.gz -C orca-src --strip-components=1 \
		--wildcards '*/resources/profiles/Creality*' '*/resources/profiles/OrcaFilamentLibrary*'
fi

python3 "$here/box.py" plate.stl
plate=$work/plate.stl

# PrusaSlicer 2.9.4: an Original Prusa MK4S, Prusament PLA. The MK4S's own
# profile writes binary G-code, which POP does not read yet, so this one is
# made as text, as any printer whose profile does not ask for binary is.
python3 "$here/flatten_ini.py" /usr/share/PrusaSlicer/profiles/PrusaResearch.ini mk4s.ini \
	"printer:Original Prusa MK4S HF0.4 nozzle" "print:0.20mm SPEED @MK4S HF0.4" "filament:Prusament PLA @MK4S HF0.4"
sed -i 's/^binary_gcode = 1$/binary_gcode = 0/' mk4s.ini
xvfb-run -a prusa-slicer --export-gcode --load mk4s.ini -o "$out/prusaslicer-mk4s.gcode" "$plate" >/dev/null
# PrusaSlicer with nothing chosen: no printer model, and a filament with no
# density, so the file says it weighs 0 g.
xvfb-run -a prusa-slicer --export-gcode -o "$out/prusaslicer-no-presets.gcode" "$plate" >/dev/null

# SuperSlicer 2.5.59.13: a Creality Ender-3 V2, generic PLA.
ss=$work/x/superslicer.AppImage/squashfs-root
python3 "$here/flatten_ini.py" "$ss/resources/profiles/Creality.ini" ender3v2.ini \
	"printer:Creality Ender-3 V2" "print:0.20mm NORMAL @CREALITY" "filament:Generic PLA @CREALITY"
(cd "$ss" && xvfb-run -a ./AppRun --export-gcode --load "$work/ender3v2.ini" -o "$out/superslicer-ender3v2.gcode" "$plate" >/dev/null)

# OrcaSlicer 2.5.0: a Creality Ender-3 V2, Creality PETG.
o=orca-src/resources/profiles
mkdir -p orca && rm -rf orca/out
python3 "$here/flatten_json.py" "$o/Creality" machine "Creality Ender-3 V2 0.4 nozzle" orca/m.json
python3 "$here/flatten_json.py" "$o/Creality" process "0.20mm Standard @Creality Ender3V2" orca/p.json
python3 "$here/flatten_json.py" "$o/Creality:$o/OrcaFilamentLibrary" filament "CR-PETG @Ender-3 V2-all" orca/f.json
(cd orca && xvfb-run -a "$work/x/orca.AppImage/squashfs-root/AppRun" --slice 0 \
	--load-settings "m.json;p.json" --load-filaments f.json --outputdir out --export-3mf plate.gcode.3mf "$plate" >/dev/null 2>&1)
cp orca/out/plate_1.gcode "$out/orcaslicer-ender3v2.gcode"
cp orca/out/plate.gcode.3mf "$out/orcaslicer-ender3v2.gcode.3mf"

# Bambu Studio 02.09.01.70: a Bambu Lab A1 mini, Bambu PLA Basic; then the
# same with Bambu PETG HF loaded in a second slot and not used.
b=$work/x/bambu.AppImage/squashfs-root/resources/profiles/BBL
mkdir -p bambu && rm -rf bambu/out bambu/out2
python3 "$here/flatten_json.py" "$b" machine "Bambu Lab A1 mini 0.4 nozzle" bambu/m.json
python3 "$here/flatten_json.py" "$b" process "0.20mm Standard @BBL A1M" bambu/p.json
python3 "$here/flatten_json.py" "$b" filament "Bambu PLA Basic @BBL A1M" bambu/f.json
python3 "$here/flatten_json.py" "$b" filament "Bambu PETG HF @BBL A1M" bambu/f2.json
bs=$work/x/bambu.AppImage/squashfs-root/AppRun
(cd bambu && xvfb-run -a "$bs" --slice 0 --load-settings "m.json;p.json" --load-filaments f.json \
	--outputdir out --export-3mf plate.gcode.3mf "$plate" >/dev/null 2>&1)
(cd bambu && xvfb-run -a "$bs" --slice 0 --load-settings "m.json;p.json" --load-filaments "f.json;f2.json" \
	--outputdir out2 --export-3mf plate.gcode.3mf "$plate" >/dev/null 2>&1)
cp bambu/out/plate_1.gcode "$out/bambustudio-a1mini.gcode"
cp bambu/out/plate.gcode.3mf "$out/bambustudio-a1mini.gcode.3mf"
cp bambu/out2/plate_1.gcode "$out/bambustudio-a1mini-two-filaments.gcode"
cp bambu/out2/plate.gcode.3mf "$out/bambustudio-a1mini-two-filaments.gcode.3mf"

# Cura 5.14.0's engine: a Creality Ender-3 (Marlin flavor) and an UltiMaker
# S5 (Griffin flavor, which writes a header of its own).
c=$work/x/cura.AppImage/squashfs-root
python3 "$here/cura.py" "$c" creality_ender3 creality_base_extruder_0 "$plate" "$out/cura-ender3.gcode"
python3 "$here/cura.py" "$c" ultimaker_s5 ultimaker_s5_extruder_left "$plate" "$out/cura-s5.gcode"

ls -l "$out"
