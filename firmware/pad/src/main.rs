//! Firmware for one corner-scale pad.
//!
//! Core 1 samples the HX711 load cell amplifiers; core 0 filters, runs the
//! serial console and bench tests, reads the temperature probe, and exchanges
//! readings with other pads over ESP-NOW. The phone UI comes next (phase 1b).
//!
//! The console runs on UART0: the board's "UART"/"COM" USB port.

#![no_std]
#![no_main]

#[cfg(all(feature = "esp32s3", feature = "esp32"))]
compile_error!("enable exactly one chip feature: `esp32s3` (default) or `esp32`");

mod app;
mod calib_store;
mod console;
mod led;
mod link;
mod out;
mod pads;
mod sampler;
mod temperature;

use embassy_executor::Spawner;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{DriveMode, Flex, OutputConfig, Pull};
#[cfg(not(feature = "sim"))]
use esp_hal::gpio::{Input, InputConfig, Level, Output};
use esp_hal::system::Stack;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::uart::{self, Uart};
use esp_rtos::embassy::Executor;
use esp_storage::FlashStorage;
use static_cell::StaticCell;

/// Load cell channels the firmware supports (four HX711s on one board).
pub const MAX_CHANNELS: usize = 4;

#[cfg(feature = "esp32s3")]
const CHIP: &str = "ESP32-S3";
#[cfg(feature = "esp32")]
const CHIP: &str = "ESP32";

esp_bootloader_esp_idf::esp_app_desc!();

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    esp_println::println!("\r\npanic: {info}\r\nrestarting");
    esp_hal::system::software_reset()
}

#[esp_hal::main]
async fn main(spawner: Spawner) {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    // The radio driver needs a heap.
    esp_alloc::heap_allocator!(size: 72 * 1024);

    // Pin map; see docs/wiring.md. Avoids strapping pins, flash/PSRAM pins
    // (ESP32: 6–11; ESP32-S3 N16R8: 26–37) and UART0.
    #[cfg(feature = "esp32s3")]
    let (hx_dout, hx_sck, onewire, uart_tx, uart_rx) = (p.GPIO4, p.GPIO5, p.GPIO6, p.GPIO43, p.GPIO44);
    #[cfg(feature = "esp32")]
    let (hx_dout, hx_sck, onewire, uart_tx, uart_rx) = (p.GPIO16, p.GPIO17, p.GPIO4, p.GPIO1, p.GPIO3);

    let (rx, tx) = Uart::new(p.UART0, uart::Config::default())
        .expect("UART0 config")
        .with_rx(uart_rx)
        .with_tx(uart_tx)
        .into_async()
        .split();
    spawner.spawn(out::task(tx).expect("output task"));
    spawner.spawn(console::task(rx).expect("console task"));

    let mut probe = Flex::new(onewire);
    probe.apply_output_config(&OutputConfig::default().with_drive_mode(DriveMode::OpenDrain).with_pull(Pull::Up));
    probe.set_output_enable(true);
    probe.set_input_enable(true);
    spawner.spawn(temperature::task(probe).expect("temperature task"));

    #[cfg(feature = "esp32s3")]
    spawner.spawn(led::task(p.RMT, p.GPIO48).expect("LED task"));
    #[cfg(feature = "esp32")]
    spawner.spawn(led::task(p.GPIO2).expect("LED task"));

    // ESP-NOW frames carry the station MAC, which is the chip's base MAC.
    let mut mac = [0u8; 6];
    mac.copy_from_slice(esp_hal::efuse::base_mac_address().as_bytes());

    let wifi = esp_radio::wifi::WifiController::new(p.WIFI, Default::default()).expect("Wi-Fi init");
    let esp_now = wifi.esp_now();
    esp_now.set_channel(link::CHANNEL).expect("ESP-NOW channel");
    let (_manager, tx, rx) = esp_now.split();
    // ESP-NOW keeps the radio running after the controller is dropped.
    drop(wifi);
    spawner.spawn(link::sender(tx).expect("ESP-NOW sender"));
    spawner.spawn(link::receiver(rx).expect("ESP-NOW receiver"));

    // Writes park core 1 for the few milliseconds a sector erase takes.
    let flash = FlashStorage::new(p.FLASH).multicore_auto_park();
    let store = calib_store::CalibStore::open(flash).ok();

    // HX711 DOUT is pulled up so a disconnected amplifier reads as "never ready".
    #[cfg(not(feature = "sim"))]
    let amp = hx711::Hx711::new(
        Input::new(hx_dout, InputConfig::default().with_pull(Pull::Up)),
        Output::new(hx_sck, Level::Low, OutputConfig::default()),
        esp_hal::delay::Delay::new(),
        hx711::Gain::A128,
    )
    .unwrap_or_else(|e| match e {
        hx711::Error::NotReady => unreachable!(),
        hx711::Error::Input(e) => match e {},
        hx711::Error::Output(e) => match e {},
    });
    #[cfg(feature = "sim")]
    let _unused_hx711_pins = (hx_dout, hx_sck);

    static CORE1_STACK: StaticCell<Stack<8192>> = StaticCell::new();
    esp_rtos::start_second_core(p.CPU_CTRL, p.FROM_CPU_INTR1, CORE1_STACK.init(Stack::new()), move || {
        static EXECUTOR: StaticCell<Executor> = StaticCell::new();
        EXECUTOR.init(Executor::new()).run(|spawner| {
            #[cfg(not(feature = "sim"))]
            spawner.spawn(sampler::hx711_task(0, amp).expect("hx711 task"));
            #[cfg(feature = "sim")]
            spawner.spawn(sampler::sim_task(0).expect("sim task"));
        });
    });

    app::App::new(1, store, mac).run().await
}
