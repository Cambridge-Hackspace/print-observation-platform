//! Printer access: one driver interface, and a driver per kind of printer.
//!
//! Every printer is reached through one trait (ADR 0001, "Printers: one driver
//! interface"). Each driver says what it can do, and each connected printer
//! runs as its own supervised task, so one misbehaving printer cannot take the
//! fleet with it. Drivers arrive in the ADR's order: Moonraker, then OctoPrint
//! and PrusaLink, then Bambu in LAN mode, then USB serial for Marlin.
//!
//! Empty until the first driver lands.
