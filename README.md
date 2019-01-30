# spsc-bip-buffer

[![Crates.io](https://img.shields.io/crates/v/spsc-bip-buffer.svg)](https://crates.io/crates/spsc-bip-buffer) [![Docs](https://img.shields.io/badge/docs-.rs-blue.svg)](https://docs.rs/spsc-bip-buffer)

| x86 | arm64 |
| --- | --- |
| [![Build Status](https://travis-ci.org/utaal/spsc-bip-buffer.svg?branch=master)](https://travis-ci.org/utaal/spsc-bip-buffer) | [![Build Status](https://drone.scw-arm-01.lattuada.me/api/badges/utaal/spsc-bip-buffer/status.svg)](https://drone.scw-arm-01.lattuada.me/utaal/spsc-bip-buffer) |

Inspired by [this article on codeproject](https://www.codeproject.com/Articles/3479/%2FArticles%2F3479%2FThe-Bip-Buffer-The-Circular-Buffer-with-a-Twist) and designed with [James Munns](https://github.com/jamesmunns) during a long night at [35c3](https://en.wikipedia.org/wiki/Chaos_Communication_Congress). Have a look at his `#[no_std]` implementation: [jamesmunns/bbqueue](https://github.com/jamesmunns/bbqueue).

## Cargo.toml

```
spsc-bip-buffer = "..."
```

## Performance

As of `e2a9fa8`, on a Intel(R) Xeon(R) CPU E5-2630 v3 @ 2.40GHz, it achieves 12.5M sends/sec and 3.2 GB/s with 255-byte long messages sent between two separate physical cores (see the `examples/perf.rs` experiment).

## License

Licensed under your choice of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))
