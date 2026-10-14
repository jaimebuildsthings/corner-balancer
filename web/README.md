# Web UI

The page the host pad serves on its own Wi-Fi network: live corner weights, total, front %, left %, cross weight, tare, corner assignment (press each pad in turn) and CSV logging to the phone.

- One file, [`index.html`](index.html), no build step and no dependencies. The firmware embeds it at build time.
- **Develop without hardware:** open the file directly or add `?mock` to the URL. It then runs on simulated pads.
- Live data comes over a WebSocket at `/ws` as [scales-protocol](../docs/scales-protocol.md) frames; the page sends console commands (`tare all`, `assign LF 9a91a8`, `blink LF`) back as text.

## Using it

1. Make one pad the host: `host on` at its console, or hold its **BOOT** button for 5 s (two long LED blinks = host on, one = off). It restarts.
2. On the phone, join Wi-Fi **CornerScales-xxxxxx** (open network). The page should pop up; if not, open `http://192.168.4.1/`.
3. Phones may warn that the network has no internet. Stay connected.
