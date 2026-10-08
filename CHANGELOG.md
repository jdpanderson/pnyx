# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `store::Stored::open_existing`, which opens a store and returns `None`,
  creating nothing, if neither of its files exists.
- `store::remove`, which deletes a store. A crash during the removal never
  leaves an older state: the store opens as the state from before, or as no
  store.

### Changed

- `store::Stored::learn` returns `io::Result<bool>`: `true` if the value was
  learned and saved, `false` if the acceptor ignored it. This is a breaking
  change.

## [0.1.0] - 2026-10-08

### Added

- CASPaxos with reconfiguration.
