//! Wire formats shared by the firmware and the tools that read it.
//!
//! - [`PadMsg`]: one reading from a pad, sent over ESP-NOW to the pad hosting the UI.
//! - [`csv`]: the CSV rows streamed over USB serial for logging.
//! - [`command`]: the serial console's command language.
//! - [`record`]: a crash-safe record format for settings kept in flash.
//! - [`link`]: link-quality statistics for readings from other pads.

#![cfg_attr(not(test), no_std)]

pub mod command;
pub mod csv;
pub mod link;
mod pad_msg;
pub mod record;

pub use command::{Command, ParseError};
pub use pad_msg::{DecodeError, PadMsg};
