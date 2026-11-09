# Roadmap

Planned work that is not part of a release, ordered by value. Each item names the concrete change and how it will be verified.

## 1. Split the binary crate into a library and a thin binary

`src/main.rs` declares 25 modules and the whole program is one binary crate, so every test is an internal unit test and no integration test or benchmark can exist.

- Move the modules into a `src/lib.rs` and keep `main.rs` as a thin entry point that parses arguments and calls the library.
- The `#[cfg(windows)]`, `#[cfg(target_os = "macos")]` and `#[cfg(any(windows, test))]` gates move with their modules.
- `src/keys.rs` is currently compiled on Windows and in tests only; keep that rule explicit in the library.

Verification: the existing test suite must keep passing unchanged, `cargo test --locked` must report the same number of tests, and the new `tests/` directory must hold the first integration test.

## 2. Fuzz the parsers that accept network data

The highest-value fuzz targets take untrusted bytes and already carry manual limits:

| Target | Why |
| --- | --- |
| `compatibility::Version::decode` | reads a length-prefixed version from a peer |
| `config::key` | parses a pinned public key |
| `file_transfer::protocol::Manifest::validate` | path depth, entry count and total size limits |
| `file_transfer::protocol::Description::validate` | the metadata-only offer |
| `file_transfer::transport::receive` | length-framed packets |
| `wire::Message` decoding | the bincode record decoder |

Use `cargo-fuzz` with the library from item 1. Each target asserts that decoding either fails with an error or produces a value that passes the corresponding `validate()`.

## 3. Model-based tests for the consensus engine

`src/group/consensus.rs` is a Raft-like engine with pre-vote, joint membership, barriers and snapshots. Unit tests cover the transitions, but not the interaction of partitions, message loss, restarts and split-brain.

- Build a deterministic simulator that drives the engine with a virtual clock and a controllable network.
- Model the invariants that matter: at most one writable leader per term, the commit index never decreases, an entry is only applied after it is committed, and a node that has voted never votes again in the same term.
- Scenario sets: partition in one direction, partition healed, delayed messages, node restart with a durable hard state, and a membership change that spans two configurations.

The simulator is slow and belongs in a separate test target, not in the unit test suite.

## 4. Decompose the largest modules

The following files are large enough that a change is easy to miss and easy to conflict with:

| Module | Lines |
| --- | --- |
| `group/file_transfer/storage.rs` | ~3000 |
| `group/consensus.rs` | ~2800 |
| `gui/view.rs` | ~2700, three separate `impl Desktop` blocks |
| `group/clipboard_files/windows/cloud.rs` | ~2500 |
| `group/file_transfer/storage/source_move.rs` | ~2400 |
| `group/file_transfer/mod.rs` | ~2300 |
| `group/net.rs` | ~2200 |

- Split `gui/view.rs` by page, so each page owns its own rendering and layout.
- Give `Desktop` a state struct per concern (`UpdateState`, `CableState`, `PairingState`, `GroupState`, `TransferState`) instead of ~64 fields and a hand-written constructor.
- In the dense `select!` branches (`workspace/net.rs:549-611`, `group/net.rs:849-862`), move each arm into a named `handle_candidate(...)` or `handle_command(...)` function so the branch reads as a list of cases.

Each extraction is a separate change with the test suite as the safety net; no behaviour change is intended.

## 5. Typed errors at the user interface boundary

Lower layers still produce free-form English messages that reach the interface through `tr(error)`. `src/cable.rs`, `src/edge.rs` and `src/updater_macos.rs` are the remaining examples; their strings are now catalog entries, but matching text is fragile.

- Give each failure a stable code, for example `Cable::InstallerTooLarge`.
- Translate the code at the interface boundary and keep the free-form text for logs and diagnostics.

## 6. Accessibility and platform signing

- `eframe` is built with `default-features = false` and without the `accesskit` feature, and the Windows host is a custom software renderer, so there is no screen reader integration. Enabling `accesskit` needs a decision about the software host.
- The installers are not Authenticode-signed and the macOS apps are not notarized. Both are distribution problems rather than code problems and need certificates and a developer account.

## 7. Typed protocol errors instead of text matching

`i18n::translate` still falls back to matching a `"<key>: <value>"` prefix for dynamic messages. Typed error codes (item 5) remove the need for that fallback.
