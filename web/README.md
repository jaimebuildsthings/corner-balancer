# Web UI

The page the host pad serves on its own Wi-Fi network: live corner weights, total, front %, left %, cross weight, tare, corner assignment (press each pad in turn) and CSV logging to the phone.

- One file, [`index.html`](index.html), no build step and no dependencies. The firmware embeds it at build time.
- **Develop without hardware:** open the file directly or add `?mock` to the URL. It then runs on simulated pads.
- Live data comes over a WebSocket at `/ws` as [scales-protocol](../docs/scales-protocol.md) frames; the page sends console commands (`tare all`, `assign LF 9a91a8`, `blink LF`) back as text.

## Using it

1. Make one pad the host: `host on` at its console, or hold its **BOOT** button for 5 s (two long LED blinks = host on, one = off). It restarts.
2. On the phone, join Wi-Fi **CornerScales-xxxxxx** (open network). A welcome page pops up ([`welcome.html`](welcome.html)).
3. Tap **Continue**, then **Done**. If the phone asks, stay connected without internet.
4. Open the browser at `http://192.168.4.1/` (add it to the Home Screen for next time).

Why the extra step: the pop-up sheet phones use for Wi-Fi logins is a cut-down browser (no CSV downloads, and closing it can drop the network). Until someone taps Continue, the pad answers the phone's connectivity check with the welcome page; after that it answers the way the phone expects, so the phone keeps the network and the full page works in the real browser. The flag resets when the pad restarts.
