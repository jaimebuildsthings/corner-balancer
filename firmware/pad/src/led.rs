//! The board LED, flashed by `blink` so someone can find a pad.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;

/// Raise to flash the LED for a few seconds.
pub static BLINK: Signal<CriticalSectionRawMutex, ()> = Signal::new();

const FLASHES: usize = 6;
const HALF_PERIOD_MS: u64 = 250;

/// Original ESP32 DevKit boards: a plain LED on GPIO2.
#[cfg(feature = "esp32")]
#[embassy_executor::task]
pub async fn task(pin: esp_hal::peripherals::GPIO2<'static>) {
    use esp_hal::gpio::{Level, Output, OutputConfig};
    let mut led = Output::new(pin, Level::Low, OutputConfig::default());
    loop {
        BLINK.wait().await;
        for _ in 0..FLASHES {
            led.set_high();
            Timer::after_millis(HALF_PERIOD_MS).await;
            led.set_low();
            Timer::after_millis(HALF_PERIOD_MS).await;
        }
    }
}

/// ESP32-S3 DevKitC boards: a WS2812 RGB LED, driven by the RMT peripheral.
/// GPIO48 on most boards; some revisions use GPIO38 (see docs/wiring.md).
#[cfg(feature = "esp32s3")]
#[embassy_executor::task]
pub async fn task(rmt: esp_hal::peripherals::RMT<'static>, pin: esp_hal::peripherals::GPIO48<'static>) {
    use esp_hal::gpio::Level;
    use esp_hal::rmt::{PulseCode, Rmt, TxChannelConfig, TxChannelCreator};
    use esp_hal::time::Rate;

    // 80 MHz ticks (12.5 ns). WS2812: 0 = 0.4 µs high + 0.85 µs low, 1 = 0.8 + 0.45.
    const ZERO: PulseCode = PulseCode::new(Level::High, 32, Level::Low, 68);
    const ONE: PulseCode = PulseCode::new(Level::High, 64, Level::Low, 36);
    fn frame(g: u8, r: u8, b: u8) -> [PulseCode; 25] {
        let bits = (u32::from(g) << 16) | (u32::from(r) << 8) | u32::from(b);
        let mut out = [PulseCode::end_marker(); 25];
        for (i, code) in out.iter_mut().take(24).enumerate() {
            *code = if bits & (1 << (23 - i)) != 0 { ONE } else { ZERO };
        }
        out
    }

    let Ok(rmt) = Rmt::new(rmt, Rate::from_mhz(80)) else { return };
    let Ok(channel) = rmt.into_async().channel0.configure_tx(&TxChannelConfig::default().with_clk_divider(1)) else {
        return;
    };
    let mut led = channel.with_pin(pin);
    let off = frame(0, 0, 0);
    let _ = led.transmit(&off).await;
    let on = frame(0, 0, 64); // dim blue
    loop {
        BLINK.wait().await;
        for _ in 0..FLASHES {
            let _ = led.transmit(&on).await;
            Timer::after_millis(HALF_PERIOD_MS).await;
            let _ = led.transmit(&off).await;
            Timer::after_millis(HALF_PERIOD_MS).await;
        }
    }
}
