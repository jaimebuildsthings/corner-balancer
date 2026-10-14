//! Pad temperature from an optional DS18B20, for drift data.

use core::sync::atomic::{AtomicU32, Ordering};

use ds18b20::{CONVERSION_MS, Ds18b20};
use embassy_time::Timer;
use esp_hal::delay::Delay;
use esp_hal::gpio::Flex;

use crate::out::say;

/// Latest temperature as `f32` bits; NaN when no probe answers.
static TEMP_BITS: AtomicU32 = AtomicU32::new(0x7FC0_0000);

/// Latest temperature, °C.
pub fn latest() -> Option<f32> {
    let t = f32::from_bits(TEMP_BITS.load(Ordering::Relaxed));
    (!t.is_nan()).then_some(t)
}

/// Reads the probe every few seconds. A missing probe is reported once and retried.
#[embassy_executor::task]
pub async fn task(pin: Flex<'static>) {
    // Releasing the bus can't fail on an esp-hal pin.
    let Ok(mut probe) = Ds18b20::new(pin, Delay::new()) else { return };
    let mut reported_missing = false;
    loop {
        let reading = match probe.start_conversion() {
            Ok(()) => {
                Timer::after_millis(u64::from(CONVERSION_MS)).await;
                probe.read_celsius()
            }
            Err(e) => Err(e),
        };
        match reading {
            Ok(t) => {
                TEMP_BITS.store(t.to_bits(), Ordering::Relaxed);
                reported_missing = false;
            }
            Err(e) => {
                TEMP_BITS.store(f32::NAN.to_bits(), Ordering::Relaxed);
                if !reported_missing {
                    say!("temperature: no reading ({:?})", e);
                    reported_missing = true;
                }
            }
        }
        Timer::after_secs(4).await;
    }
}
