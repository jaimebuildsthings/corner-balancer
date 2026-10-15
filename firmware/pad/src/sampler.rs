//! Load cell sampling. Runs on core 1 so radio and web work on core 0
//! can't delay the HX711's timing-sensitive reads.

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Instant;
#[cfg(not(feature = "sim"))]
use embassy_time::Timer;
#[cfg(not(feature = "sim"))]
use esp_hal::delay::Delay;
#[cfg(not(feature = "sim"))]
use esp_hal::gpio::{Input, Output};
#[cfg(not(feature = "sim"))]
use hx711::Hx711;

/// One raw reading.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// Load cell channel, 0–3.
    pub ch: u8,
    /// Raw HX711 count.
    pub raw: i32,
    /// Milliseconds since boot.
    pub t_ms: u64,
}

/// Raw readings from core 1 to the app on core 0.
pub static SAMPLES: Channel<CriticalSectionRawMutex, Sample, 32> = Channel::new();

/// Readings dropped because the app fell behind.
pub static DROPPED: AtomicU32 = AtomicU32::new(0);

fn publish(ch: u8, raw: i32) {
    let sample = Sample { ch, raw, t_ms: Instant::now().as_millis() };
    if SAMPLES.try_send(sample).is_err() {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// The HX711 behind one channel.
#[cfg(not(feature = "sim"))]
pub type Amp = Hx711<Input<'static>, Output<'static>, Delay>;

/// Reads one HX711 forever. DOUT is polled every millisecond; at 10 or 80
/// samples/s that is well inside the window before the next conversion.
#[cfg(not(feature = "sim"))]
#[embassy_executor::task(pool_size = 4)]
pub async fn hx711_task(ch: u8, mut amp: Amp) {
    loop {
        match amp.read() {
            Ok(raw) => publish(ch, raw),
            Err(hx711::Error::NotReady) => Timer::after_millis(1).await,
            Err(hx711::Error::Input(e)) => match e {},
            Err(hx711::Error::Output(e)) => match e {},
        }
    }
}

/// Load added while the BOOT button is held, kg.
#[cfg(feature = "sim")]
pub const BOOT_PRESS_KG: f32 = 20.0;

/// Simulated load in kg as `f32` bits, set with the `sim KG` console command.
#[cfg(feature = "sim")]
pub static SIM_KG: AtomicU32 = AtomicU32::new(0);

/// Simulated pad for boards without load cells: 10 samples/s of the load set
/// by `sim KG` (default 0), scaled like a bench pad, with a few grams of noise.
/// Holding the board's BOOT button adds [`BOOT_PRESS_KG`], like pressing on a pad.
#[cfg(feature = "sim")]
#[embassy_executor::task]
pub async fn sim_task(ch: u8) {
    const ZERO: f32 = -120_000.0;
    const COUNTS_PER_KG: f32 = 21_000.0;
    let mut ticker = embassy_time::Ticker::every(embassy_time::Duration::from_millis(100));
    let mut rng: u32 = 0x2545_F491;
    loop {
        ticker.next().await;
        // xorshift32
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        let noise = (rng % 81) as f32 - 40.0;
        let pressed = if crate::button::HELD.load(Ordering::Relaxed) { BOOT_PRESS_KG } else { 0.0 };
        let kg = f32::from_bits(SIM_KG.load(Ordering::Relaxed)) + pressed;
        let counts = (ZERO + COUNTS_PER_KG * kg + noise).clamp(hx711::MIN as f32, hx711::MAX as f32);
        publish(ch, counts as i32);
    }
}
