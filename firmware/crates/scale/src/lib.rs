//! The load cell signal chain, from raw HX711 counts to kilograms.
//!
//! Everything here is plain `no_std` Rust with no hardware or allocator
//! dependencies, so it runs unchanged on the ESP32-S3 and in host tests.
//!
//! - [`Scale`]: median-of-5 then exponential moving average, stability flag, tare.
//! - [`Calibration`]: multi-point least-squares fit from counts to kg, with residuals.
//! - [`corner`]: corner-weight percentages and the two-pad split.
//! - [`stats`] and [`bench`]: statistics and the bench test protocol.

#![cfg_attr(not(test), no_std)]

pub mod bench;
pub mod calibration;
pub mod corner;
mod filter;
mod math;
pub mod stats;

pub use calibration::{CalError, CalPoint, Calibration, Fit};
pub use corner::{CornerResult, corners, split};
pub use filter::{FilterConfig, MAX_STABLE_SAMPLES, Reading, Scale};
