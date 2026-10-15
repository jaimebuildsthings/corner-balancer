//! The BOOT button (GPIO0 on ESP32 and ESP32-S3), read once the chip is running.
//!
//! - Held for [`HOST_TOGGLE_MS`]: toggle host mode (until the enclosure gets its own button).
//! - In simulator builds, held at all: adds weight, like pressing on a pad.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use esp_hal::gpio::Input;

/// How long BOOT must be held to toggle host mode.
pub const HOST_TOGGLE_MS: u64 = 5_000;

/// True while BOOT is held.
pub static HELD: AtomicBool = AtomicBool::new(false);

/// Raised once per long press.
pub static LONG_PRESS: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Polls the button every 20 ms.
#[embassy_executor::task]
pub async fn task(button: Input<'static>) {
    let mut held_ms = 0u64;
    loop {
        Timer::after_millis(20).await;
        let held = button.is_low();
        HELD.store(held, Ordering::Relaxed);
        if !held {
            held_ms = 0;
            continue;
        }
        held_ms += 20;
        if held_ms == HOST_TOGGLE_MS {
            LONG_PRESS.signal(());
        }
    }
}
