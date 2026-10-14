# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `store::paths`, which gives the two files of a store, and
  `store::Stored::paths`, which gives them for an open store.

### Changed

- On Unix, a store's files are created with mode `0600`: readable and
  writable by their owner only. Files that exist keep their mode. A store
  that is created by one user and opened by another now needs its mode or
  owner changed after `store::Stored::create`.

## [0.2.0] - 2026-10-08

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
