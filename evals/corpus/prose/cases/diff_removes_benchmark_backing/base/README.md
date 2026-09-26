# quickparse

quickparse is a streaming JSON parser for Rust.

## Performance

quickparse is the fastest streaming JSON parser for Rust. On the
`twitter.json` corpus it parses 2.1 GB/s on one core, against 0.9 GB/s for the
next fastest crate we measured; see
[benches/RESULTS.md](benches/RESULTS.md) for the full table.

## Install

Add the crate to your project with `cargo add`:

```sh
cargo add quickparse
```
