# Slicer fixtures

Real files, each written by the slicer it is named for. Every one slices
the same 20 × 20 × 2 mm plate (`tools/slicer-fixtures/box.py`) with that
vendor's own presets, made on 2026-11-26 by `tools/slicer-fixtures/make.sh`
on an Ubuntu 26.04 machine. The files are POP's own output, so they carry
no one else's license.

| File | Slicer | Printer | Presets |
|---|---|---|---|
| `prusaslicer-mk4s.gcode` | PrusaSlicer 2.9.4 (Ubuntu's `prusa-slicer` 2.9.4+dfsg-4) | Original Prusa MK4S HF0.4 nozzle | 0.20mm SPEED @MK4S HF0.4, Prusament PLA @MK4S HF0.4 |
| `prusaslicer-no-presets.gcode` | PrusaSlicer 2.9.4 | none chosen | PrusaSlicer's defaults |
| `superslicer-ender3v2.gcode` | SuperSlicer 2.5.59.13 | Creality Ender-3 V2 | 0.20mm NORMAL @CREALITY, Generic PLA @CREALITY |
| `orcaslicer-ender3v2.gcode` | OrcaSlicer 2.5.0 | Creality Ender-3 V2 0.4 nozzle | 0.20mm Standard @Creality Ender3V2, CR-PETG @Ender-3 V2-all |
| `bambustudio-a1mini.gcode` | Bambu Studio 02.09.01.70 | Bambu Lab A1 mini 0.4 nozzle | 0.20mm Standard @BBL A1M, Bambu PLA Basic @BBL A1M |
| `bambustudio-a1mini-two-filaments.gcode` | Bambu Studio 02.09.01.70 | as above | as above, with Bambu PETG HF @BBL A1M loaded in slot 2 and unused |
| `cura-ender3.gcode` | Cura 5.14.0's engine | Creality Ender-3 (Marlin flavor) | the machine's definition defaults |
| `cura-s5.gcode` | Cura 5.14.0's engine | UltiMaker S5 (Griffin flavor) | the machine's definition defaults |

## How they differ from a member's

The slicers ran from the command line, which is not quite what a member
clicking **Export** or **Upload** gets.

- **Presets were flattened first.** The command lines read a preset without
  what it inherits, so `make.sh` resolves each preset's `inherits` chain
  first, as the GUI does. Without that, a Bambu filament has density 0, and
  the file says the print weighs 0 g.
- **The MK4S file is text G-code.** The MK4S's own profile writes binary
  G-code (`.bgcode`), which POP does not read yet. This file was made with
  `binary_gcode = 0`, as any printer whose profile does not ask for binary
  is.
- **Cura's frontend did not run.** Cura's engine cannot evaluate the formulas
  its frontend computes settings with. `tools/slicer-fixtures/cura.py`
  supplied the two it asked for, `roofing_layer_count` and
  `flooring_layer_count`, both from their definitions' default of 0. A file
  exported from Cura itself also ends with `;SETTING_3` lines of the profile,
  and a Griffin header from Cura names each extruder's material by GUID.
  This reader uses neither, since a GUID is not a material's name.
- **No thumbnails.** The guest had no display for the slicers to render one
  on.

## Making them again

On a throwaway Ubuntu 26.04 machine, as root:

```sh
tools/slicer-fixtures/make.sh /tank/work /tank/out
```

then copy `/tank/out/*.gcode` here. Each AppImage is pinned by its hash. A
new slicer version is a new fixture beside the old one, not a replacement,
so a format that changes keeps a test of both.
