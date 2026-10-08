# pnyx

[![crates.io](https://img.shields.io/crates/v/pnyx.svg)](https://crates.io/crates/pnyx)
[![docs.rs](https://img.shields.io/docsrs/pnyx)](https://docs.rs/pnyx)
[![CI](https://github.com/jdpanderson/pnyx/actions/workflows/ci.yml/badge.svg)](https://github.com/jdpanderson/pnyx/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/crates/msrv/pnyx)](#minimum-rust-version)
[![License](https://img.shields.io/crates/l/pnyx)](#license)

The [Pnyx](https://en.wikipedia.org/wiki/Pnyx)  is the hill where the Athenian assembly met. It
stood for *isēgoría*, the equal right of every citizen to speak. 

[CASPaxos](https://arxiv.org/abs/1802.07000) in Rust: agreement on one value,
changed by compare-and-set, with no leader and no log. In CASPaxos
there is no leader, and every node can propose a change.

- **Reconfiguration.** The set of acceptors can change while the cluster runs.
- **No I/O in the protocol.** The acceptor and the proposer are state machines,
  so they can be model-checked. The tests check them with
  [Stateright](https://crates.io/crates/stateright).
- **Any transport.** `propose` runs one change over a `Transport` that you
  implement.
- **Durable acceptors.** The `store` module keeps an acceptor's state on disk,
  in two copies, so that a crash during a save leaves the state before it.

The crate is generic over the node ID and the value. It is safe when nodes
crash and messages are lost, but not when nodes lie. It has the parts that a
layer for one faulty acceptor needs from the acceptor's saved state; see
*Byzantine faults* in the [documentation](https://docs.rs/pnyx).

## Usage

```toml
[dependencies]
pnyx = "0.2"
```

| Feature | Default | What it adds |
| ------- | ------- | ------------ |
| `tokio` | yes | `propose`, which runs one change over a `Transport` on Tokio. |
| `store` | yes | `store::Stored`, an acceptor whose state is kept in files. |

Without either feature, pnyx is the two state machines, and depends only on
`serde` and `thiserror`.

The [crate documentation](https://docs.rs/pnyx) has examples of the state
machines and of `propose`. [`examples/cluster.rs`](examples/cluster.rs) runs a
cluster of three nodes in one process, with acceptors in files:

```sh
cargo run --example cluster
```

## Platforms

pnyx works on Linux, macOS and Windows. On Windows, a power loss right after
a store is first created can undo the creation of its files, because Windows
has no documented way to sync a directory. Saves after that are as safe as on
Unix.

## Minimum Rust version

Rust 1.91.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
