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

With `--features sim` there are no load cells: set the simulated load with `sim 20` (kg), `sim 0`, and so on. Holding the board's **BOOT** button adds 20 kg while held, like pressing on a pad.

## Pads and corners

Every pad broadcasts its weight 10×/s and listens for the others. Each pad has a **name** (last 6 hex digits of its MAC, e.g. `9a91a8`; put it on a sticker) and a **corner** (LF, RF, LR, RR, or none), saved on the pad.

```text
assign LF         # then press firmly on the pad under the left-front wheel
assign RF 9a91a8  # or assign directly by name (or `this`)
unassign RR       # free a corner
blink LF          # flash a pad's LED to find it (S3: RGB LED, ESP32: GPIO2 LED)
tare all          # zero every pad at its next settled reading
pads              # names, corners, weights, link loss, longest gap, totals
pads reset        # clear link counters before a timed run
```

Assigning a corner takes it from whichever pad had it. With sim pads: `assign LF`, then hold **BOOT** on the board that should be LF; repeat for RF. Then `sim 14` on LF and `sim 6` on RF: `pads` shows `RF carries 30.00 %`.
