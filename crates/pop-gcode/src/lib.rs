//! Slicer metadata from G-code and 3MF files.
//!
//! POP is not a slicer (ADR 0001). It reads what the slicer wrote into a file:
//! the printer model, the material, the estimated time and filament. Bambu
//! printers take a sliced `.gcode.3mf`, so metadata is read from the G-code
//! inside the 3MF as well as from plain G-code.
//!
//! Empty until the upload endpoint needs it.
