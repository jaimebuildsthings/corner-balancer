# Architecture

How the pieces fit together. For *why* and the build plan, see [plan.md](plan.md).

**Contents:** [System](#system) · [Repo map](#repo-map) · [Pad firmware](#pad-firmware) · [Signal chain](#signal-chain) · [Flash layout](#flash-layout) · [Interfaces](#interfaces) · [Status](#status)

## System

Every board is a **pad**. One pad also hosts the phone UI. No dedicated hub board.

```mermaid
flowchart TB
    phone["📱 Phone browser"]
    laptop["💻 Laptop"]
    A["Pad A<br/>(also hosts the UI)"]
    B["Pads B…D"]
    phone -. "Wi-Fi AP + WebSocket (1b)" .-> A
    laptop -- "USB serial: console + CSV<br/>(any pad, 1a)" --> A
    B -- "ESP-NOW broadcast (1b)" --> A
```

Solid = built. Dashed = planned.

## Repo map

| Path | What |
| --- | --- |
| `firmware/crates/scale` | Filter, tare, calibration, corner math, bench-test statistics |
| `firmware/crates/protocol` | Wire formats: console commands, CSV rows, `PadMsg`, flash records |
| `firmware/crates/hx711` | HX711 bit-bang driver |
| `firmware/crates/ds18b20` | DS18B20 one-wire driver |
| `firmware/pad` | The binary for ESP32-S3 (and ESP32 for bench work) |
| `web/` | Phone UI (phase 1b) |
| `docs/` | Plan, wiring, assembly, test protocol |

The four crates are `no_std`, hardware-independent and tested on the host with `cargo test`. Only `pad/` touches ESP hardware.

```mermaid
flowchart TB
    pad["pad (binary)"]
    pad --> scale
    pad --> protocol
    pad --> hx711
    pad --> ds18b20
    pad --> esp["esp-hal · esp-rtos · embassy"]
```

## Pad firmware

Two cores, connected by a queue. Core 1 only samples, so radio and web work on core 0 can't disturb the HX711's timing.

```mermaid
flowchart TB
    subgraph core1["Core 1"]
        hx["hx711_task ×1–4<br/>(or sim_task)"]
    end
    q[["SAMPLES queue (32)"]]
    subgraph core0["Core 0"]
        app["App: filter · tests · CSV"]
        con["console task<br/>(UART0 RX)"]
        temp["temperature task<br/>(DS18B20, every ~5 s)"]
        out["output task<br/>(UART0 TX)"]
        radio["ESP-NOW tasks<br/>(send · receive)"]
        flash[("calib partition")]
    end
    hx --> q --> app
    con -- "lines" --> app
    temp -- "latest °C" --> app
    app -- "text, CSV" --> out
    app <--> flash
    app <-- "PadMsg" --> radio
```

| File | Role |
| --- | --- |
| `main.rs` | Pin map, starts tasks and core 1 |
| `sampler.rs` | Reads the HX711 (or simulates one), pushes raw counts |
| `app.rs` | Main loop: commands, filtering, bench tests, CSV |
| `console.rs` / `out.rs` | Serial input with echo / output queue |
| `calib_store.rs` | Saves and loads calibration |
| `temperature.rs` | Probe reading, shared as an atomic |
| `link.rs` | ESP-NOW: broadcast this pad's readings, collect others' |

## Signal chain

Per channel, all in `scale`:

```mermaid
flowchart TB
    raw["raw counts (24-bit)"] --> med["median of 5<br/>rejects spikes"]
    med --> ema["moving average (EMA)"]
    ema --> fit["calibration fit<br/>counts → kg"]
    fit --> tare["minus tare"]
    tare --> rd["Reading { kg, stable }"]
    ema --> stab["stability: last N within band"] --> rd
```

- **Calibration:** least-squares line through up to 16 points. Residuals show nonlinearity.
- **Tare** is stored in counts, so it survives recalibration.
- **Bench tests** (`scale::bench`) consume `Reading`s. Pass targets scale with full-scale rating (200 kg bench, 2,000 kg car pad).

## Flash layout

From [`partitions.csv`](../firmware/pad/partitions.csv). It fits 4 MB, so both board types use it.

| Partition | Size | Use |
| --- | --- | --- |
| `nvs` | 20 KB | Reserved (radio) |
| `otadata` | 8 KB | Which app slot boots |
| `ota_0` / `ota_1` | 1.75 MB each | App slots for OTA updates |
| `calib` | 8 KB | Two-slot record with CRC: calibration + pad ID. Never touched by app updates. |

## Interfaces

| Interface | Format | Defined in |
| --- | --- | --- |
| Serial console | Text commands (`help` lists them) | `protocol::command` |
| CSV stream | `t_ms,ch,raw,filtered,kg,stable,temp_c` | `protocol::csv` |
| Pad ↔ pad | 19-byte `PadMsg`, ESP-NOW broadcast, channel 1 | `protocol::PadMsg` |
| Phone UI | WebSocket JSON | *planned (1b)* |

## Status

| Piece | State |
| --- | --- |
| Host crates + tests | ✅ Built |
| Pad firmware: sampling, console, calibration, bench tests, CSV | ✅ Builds; untested on hardware |
| ESP-NOW between pads, `pads` view | ✅ Builds; untested on hardware |
| Wi-Fi AP, phone UI | ⏳ Phase 1b |
| OTA updates | ⏳ Later (partition slots already in place) |
