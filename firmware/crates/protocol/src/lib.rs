//! Wire formats shared by the firmware and the tools that read it.
//!
//! - [`PadMsg`]: one reading from a pad, sent over ESP-NOW to the pad hosting the UI.
//! - [`csv`]: the CSV rows streamed over USB serial for logging.
//! - [`command`]: the serial console's command language.

#![cfg_attr(not(test), no_std)]

pub mod command;
pub mod csv;
mod pad_msg;

pub use command::{Command, ParseError};
pub use pad_msg::{DecodeError, PadMsg};
