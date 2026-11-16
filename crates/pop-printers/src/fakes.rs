//! Fake printers that speak each real protocol over a real socket (#26).
//!
//! Each driver is tested against its fake, and each fake can inject the
//! faults real printers produce: dropped connections, slow answers,
//! malformed messages and refused commands.

pub mod bambu;
pub mod moonraker;
