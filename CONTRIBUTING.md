# Contributing

Thanks for helping. This project is a prototype, so issues describing what you built, what you measured and what went wrong are as valuable as code.

## Ground rules

- **Dependencies:** every new crate, package or library needs a permissive, Apache-2.0-compatible license (MIT, Apache-2.0, BSD, ISC, Zlib). No GPL, LGPL or other copyleft code. Say which license it uses in the pull request.
- **Hardware-independent logic goes in `firmware/crates/`** and gets host tests. Keep those crates `no_std` and free of ESP-specific dependencies.
- **Measurements:** when a change affects readings (filtering, timing, calibration), include before/after data from the test protocol in [docs/test-protocol.md](docs/test-protocol.md).
- **Safety:** don't weaken the safety guidance in the docs. Anything that puts a car on a pad must say so plainly.

## Before you open a pull request

```sh
cd firmware
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

For firmware changes, also build `firmware/pad` (see [firmware/README.md](firmware/README.md)).

## Licensing of contributions

By contributing, you agree that your contribution is licensed under the license of the part of the repository it touches: Apache-2.0 for code, CC-BY-SA-4.0 for documentation, CERN-OHL-P-2.0 for hardware designs.
