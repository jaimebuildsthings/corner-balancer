# corner-balancer

DIY corner-weight scales for cars, built on an ESP32-S3 and the HX711 load cell amplifier. Each pad is a self-contained instrument; one pad also hosts a phone UI over its own Wi-Fi access point, so the set works in any garage without a network.

> **Status: prototype.** Phase 1 (bench pads rated ~200 kg) is in progress. Nothing here is rated for a car yet. See [docs/plan.md](docs/plan.md) for the roadmap.

> [!WARNING]
> **A car on scales can roll or slip off a pad or ramp.** Chock the wheels, keep people clear while rolling on and off, and never get under a car resting on scales. Build and use these at your own risk; see the license for the warranty disclaimer.

## What's here

| Path | Contents |
| --- | --- |
| [`firmware/`](firmware/) | Rust firmware (`esp-hal`, Embassy). `crates/` hold the hardware-independent logic with host tests; `pad/` is the ESP32-S3 binary. |
| [`web/`](web/) | Phone UI source, with a mock data feed for development without hardware. |
| [`tools/`](tools/) | Host-side helpers for capturing and plotting test runs. |
| [`docs/`](docs/) | Plan, wiring, assembly, bill of materials and test protocol. |
| [`hardware/`](hardware/) | Mechanical drawings and CAD (future). |

## Quick start

Run the host tests (needs only stable Rust):

```sh
cd firmware
cargo test
```

Building and flashing the pad firmware is described in [firmware/README.md](firmware/README.md).

## License

- Code (`firmware/`, `web/`, `tools/`): [Apache-2.0](LICENSE)
- Documentation (`docs/`, READMEs): [CC-BY-SA-4.0](LICENSE-docs)
- Hardware designs (`hardware/`): [CERN-OHL-P-2.0](hardware/LICENSE)

Contributions are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md).
