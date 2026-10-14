//! Wire formats shared by the firmware and the tools that read it.
//!
//! - [`PadMsg`]: one reading from a pad, broadcast over ESP-NOW to the other pads.
//! - [`cmd_msg`]: commands between pads over ESP-NOW (assign a corner, tare, blink).
//! - [`pad`]: pad names (from the MAC address) and corners.
//! - [`assign`]: press-to-assign detection.
//! - [`csv`]: the CSV rows streamed over USB serial for logging.
//! - [`command`]: the serial console's command language.
//! - [`record`]: a crash-safe record format for settings kept in flash.
//! - [`link`]: link-quality statistics for readings from other pads.
//! - [`json`]: the scales protocol, JSON lines for apps (docs/scales-protocol.md).

#![cfg_attr(not(test), no_std)]

pub mod assign;
pub mod cmd_msg;
pub mod command;
pub mod csv;
pub mod json;
pub mod link;
pub mod pad;
mod pad_msg;
pub mod record;

pub use command::{Command, ParseError};
pub use pad_msg::{DecodeError, PadMsg};
