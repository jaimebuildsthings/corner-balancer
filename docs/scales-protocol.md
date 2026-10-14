# Scales protocol

How an app (for example a web page) reads the scales. One JSON object per line, the same on every transport.

**Contents:** [Transports](#transports) · [Frames](#weights-frame) · [Commands](#commands) · [Web Serial example](#web-serial-example) · [Versioning](#versioning)

## Transports

| Transport | Status | Works in |
| --- | --- | --- |
| USB serial, 115200 baud | ✅ Now | Chrome/Edge desktop via Web Serial; any serial terminal |
| Bluetooth LE | ⏳ Planned | Chrome on Android/desktop via Web Bluetooth (not iOS Safari) |

Connect to **any pad**: each one hears the others over ESP-NOW and reports all of them.

## Weights frame

Send `json on` and the pad prints a frame 5×/s:

```json
{"v":1,"type":"weights","t_ms":184200,"pads":[
  {"id":0,"kg":312.400,"stable":true,"calibrated":true,"age_ms":0},
  {"id":1,"kg":298.100,"stable":true,"calibrated":true,"age_ms":40}
]}
```

(One line on the wire; wrapped here.)

| Field | Meaning |
| --- | --- |
| `v` | Protocol version (1) |
| `type` | `"weights"` (the only type so far; ignore unknown types) |
| `t_ms` | Milliseconds since the sending pad booted |
| `pads[].id` | 0 = LF, 1 = RF, 2 = LR, 3 = RR |
| `pads[].kg` | Weight after calibration and tare. **Counts, not kg, if `calibrated` is false.** `null` if invalid |
| `pads[].stable` | The reading has settled. Wait for all four before trusting cross weight |
| `pads[].calibrated` | The pad has a calibration |
| `pads[].age_ms` | How old this pad's reading is. Pads silent for over 2 s are left out |

Corner math is left to the app: total, front %, left %, cross = (LF + RR) / total.

Other serial output (console replies, CSV) shares the port. **Parse only lines that start with `{`.**

## Commands

Send console commands as text lines ending in `\n`:

| Command | Effect |
| --- | --- |
| `json on` / `json off` | Start / stop frames |
| `tare` | Zero **the connected pad** at its next settled reading |
| `pads` | Human-readable table (not JSON) |

Taring every pad from one connection is not supported yet.

## Web Serial example

```js
const port = await navigator.serial.requestPort();
await port.open({ baudRate: 115200 });

const writer = port.writable.getWriter();
await writer.write(new TextEncoder().encode("json on\n"));
writer.releaseLock();

const lines = port.readable
  .pipeThrough(new TextDecoderStream())
  .pipeThrough(new TransformStream({
    start() { this.buf = ""; },
    transform(chunk, ctl) {
      this.buf += chunk;
      const parts = this.buf.split(/\r?\n/);
      this.buf = parts.pop();
      parts.forEach((l) => ctl.enqueue(l));
    },
  }));

for await (const line of lines) {
  if (!line.startsWith("{")) continue;
  const frame = JSON.parse(line);
  if (frame.v !== 1 || frame.type !== "weights") continue;
  const kg = Object.fromEntries(frame.pads.filter((p) => p.calibrated).map((p) => [p.id, p.kg]));
  // kg[0..3] = LF, RF, LR, RR
}
```

## Versioning

- New fields may be added within version 1; ignore fields you don't know.
- Removing or changing the meaning of a field bumps `v`.
- Source of truth: [`protocol::json`](../firmware/crates/protocol/src/json.rs).
