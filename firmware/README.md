# Firmware

Rust, no ESP-IDF. Layout and design: [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md).

## Host tests (stable Rust, no hardware)

```sh
cd firmware
cargo test
```

## One-time setup for the pad firmware

```sh
cargo install espup espflash
espup install          # Espressif's Rust toolchain for Xtensa chips
```

In every new shell before building: `source ~/export-esp.sh`.

## Build and flash

Plug the board into its **UART / COM** USB port (the console runs on UART0), then from `firmware/pad`:

| Board | Command |
| --- | --- |
| ESP32-S3 | `cargo run --release` |
| ESP32 (spare boards) | `cargo esp32-run` |
| Either, no load cells | add `--features sim` |

`cargo run` flashes with the project's partition table and opens the serial monitor. Type `help`.

## Pins

| Signal | ESP32-S3 | ESP32 |
| --- | --- | --- |
| HX711 DOUT | GPIO4 | GPIO16 |
| HX711 SCK | GPIO5 | GPIO17 |
| DS18B20 data (4.7 kΩ to 3.3 V) | GPIO6 | GPIO4 |
| Console UART0 TX / RX | GPIO43 / GPIO44 | GPIO1 / GPIO3 |

## First session at the console

```text
status                 # chip, channels, calibration
stream on              # CSV rows; `stream off` to stop
tare                   # empty pad
cal add 0              # empty pad, once stable
cal add 20             # 20 kg on the pad
cal fit                # prints residuals and linearity verdict
cal save
test noise             # then: test repeat, test offcenter, test creep 600, test zero
```

With `--features sim` there are no load cells: set the simulated load with `sim 20` (kg), `sim 0`, and so on.
