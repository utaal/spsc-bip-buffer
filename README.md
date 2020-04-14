# spsc-bip-buffer

[![Crates.io](https://img.shields.io/crates/v/spsc-bip-buffer.svg)](https://crates.io/crates/spsc-bip-buffer) [![Docs](https://img.shields.io/badge/docs-.rs-blue.svg)](https://docs.rs/spsc-bip-buffer)

| x86 |
| --- |
| [![Build Status](https://travis-ci.org/utaal/spsc-bip-buffer.svg?branch=master)](https://travis-ci.org/utaal/spsc-bip-buffer) |

`spsc-bip-buffer` is a single-producer single-consumer circular buffer that always supports writing a contiguous chunk of data. Write requests that cannot fit in an available contiguous area will wait till space is newly available (after the consumer has read the data).

`spsc-bip-buffer` is lock-free and uses atomics for coordination.

Here's a simple example:

```rust
use spsc_bip_buffer::bip_buffer_with_len;
let (mut writer, mut reader) = bip_buffer_with_len(256);
let sender = std::thread::spawn(move || {
    for i in 0..128 {
        let mut reservation = writer.spin_reserve(8);
        reservation.copy_from_slice(&[10, 11, 12, 13, 14, 15, 16, i]);
        reservation.send(); // optional, dropping has the same effect
    }
});
let receiver = std::thread::spawn(move || {
    for i in 0..128 {
        while reader.valid().len() < 8 {}
        assert_eq!(&reader.valid()[..8], &[10, 11, 12, 13, 14, 15, 16, i]);
        reader.consume(8);
    }
});
sender.join().unwrap();
receiver.join().unwrap();
```

Usage documentation is at [docs.rs/spsc-bip-buffer](https://docs.rs/spsc-bip-buffer).

`spsc-bip-buffer` is inspired by [this article on codeproject](https://www.codeproject.com/Articles/3479/%2FArticles%2F3479%2FThe-Bip-Buffer-The-Circular-Buffer-with-a-Twist) and was designed with [James Munns](https://github.com/jamesmunns) during a long night at [35c3](https://en.wikipedia.org/wiki/Chaos_Communication_Congress). Have a look at his `#[no_std]` implementation: [jamesmunns/bbqueue](https://github.com/jamesmunns/bbqueue).

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
