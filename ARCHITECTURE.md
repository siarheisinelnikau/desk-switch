# Architecture

Desk Switch shares monitors, keyboard/mouse input, audio, microphone, clipboard and files between Windows, Ubuntu and macOS computers. It is a **single self-contained Rust binary**: the same executable runs the desktop GUI, every long-running service, and every short CLI command. There is no daemon to install separately; services are threads inside the GUI process or the same binary re-executed as a child worker.

## 1. Process model

`src/main.rs` dispatches on the argument vector. On Windows the binary uses the GUI subsystem and attaches to the parent console only for explicit CLI commands (`main.rs:48-62`).

**User-facing commands** (`main.rs:75-205`): `gui [--tray] [--state-dir PATH]`, `init`, `public-key`, `pair PUBLIC_KEY`, `state-dir`, `probe [MONITOR_ID|--json]`, `monitor-select ID INPUT`, `monitor-read ID`, `portal-check [--consent]`, `run [--role host|receiver] IP:PORT`, `audio …`, `microphone …`, `input-access …`, plus `--licenses`, `--build-info`, `--version`.

**Internal helper commands** (spawned by the app, not for interactive use): `monitor-service`, `internal-update-apply`, `--apply-update`, `gui … --cleanup-update DIR`, `macos-export-helper`, `internal-macos-update`, `gui-preview` (debug builds only).

Two execution shapes:

- **In-process services (threads).** Started by the GUI: `workspace::net::Service` (legacy pair), `group::net::Service` (workspace mesh), `group::file_transfer::Service`, `discovery::Discovery` (mDNS), `workspace::hardware::Probe`, `group::clipboard::Clipboard`, the tray event thread, and updater job threads.
- **Child workers (same binary re-executed).** `gui/worker.rs:81-120` spawns `current_exe()` with piped stdin/stdout/stderr, `DESK_SWITCH_STATE` and `DESK_SWITCH_MANAGED` env vars, and `CREATE_NO_WINDOW` on Windows. Kinds: `Input`, `Monitor`, `AudioSend`, `AudioReceive`, `MicrophoneSend`, `MicrophoneReceive`, `Query`. Long-running children (`run`, `monitor-service`, `audio serve|play`, `microphone serve|play`) read commands from stdin and emit structured events on stdout.

`src/lifecycle.rs` is the control bus between parent and child: a line-oriented stdin command protocol (`switch`, `switch-edge`, `enter-edge`, `routes`, `switch-display`, `enter-display`, `setup-switch`, `emergency`, `return-input`, `volume`, `stop`) and structured `@desk-switch …` JSON events on stdout. `lifecycle::shutdown()` handles Ctrl-C and stop requests.

External programs used at runtime: `pkexec`/`pkcheck` (Linux privileged input access and package install), `ddcutil` (Linux DDC/CI), `pactl`/PipeWire tools, `dbus-run-session`, `xdg-open`/`explorer.exe`/`open` (reveal files), and the embedded macOS helpers `desk-switch-native` / `desk-switch-ddc` extracted by `macos_native.rs`.

## 2. Network

All listeners are wildcard, dual-stack where the OS allows it (`src/listen.rs`, `group/net.rs:337-362`). There is no NAT traversal and no UDP socket of our own; UDP is only used internally by `mdns-sd`.

| Port | Purpose | Defined in |
| --- | --- | --- |
| 45876/TCP | Input/video data link; also the mDNS service port | `workspace/mod.rs` `INPUT_PORT` |
| 45877/TCP | System audio | `workspace/mod.rs` `AUDIO_PORT` |
| 45878/TCP | Legacy two-PC pair/control; advertised as `control-port` | `workspace/mod.rs` `PORT` |
| 45879/TCP | Legacy two-PC microphone | `workspace/mod.rs` `MICROPHONE_PORT` |
| 45880/TCP | Workspace (group) control mesh, protocol 3 | `group/model.rs` `PORT` |
| 45881/TCP | File transfer | `group/file_transfer/mod.rs` `PORT` |
| 49000+N/TCP | Microphone fan-out per sorted member index | `gui/group_data.rs:205-211` |

Discovery is mDNS `_desk-switch._tcp.local.` (`src/discovery.rs`) with TXT properties `app`, `name`, `role`, `version`, `protocol`, `audio-port`, `control-port`, `control-protocol`, `key`. Parsed addresses are bounded (max 8; loopback/unspecified/multicast dropped).

## 3. State and storage

`config::state_dir()` honours `DESK_SWITCH_STATE`, then `%LOCALAPPDATA%\desk-switch` (Windows), `~/Library/Application Support/desk-switch` (macOS), `$XDG_CONFIG_HOME/desk-switch` or `~/.config/desk-switch` (Linux). Directories are created `0o700`, files `0o600`, and writes go through `config::write_atomic` (unique temp file + rename + directory fsync).

| File | Contents |
| --- | --- |
| `identity` | This computer's Ed25519/Noise keypair |
| `peer` | Pinned public key of the legacy paired computer |
| `settings.toml` | Monitor, edge, route and capture settings |
| `desktop.toml` | GUI `Preferences` (language, theme, devices) |
| `desktop.lock`, `run.lock`, `audio.lock`, `microphone.lock`, `portal.lock` | Singleton locks |
| `workspace.json` | Cached workspace/group state |
| `workspace-consensus.json`, `workspace-admission.json`, `workspace-locations.json` | Durable consensus hard state, admission and endpoints |
| `group-input` | Marker that selects group input workers |
| `native-audio-restore.json` | Mixer restore journal |
| `updates/pending.json`, `updates/last-error.txt`, `updates/apply-helper*` | Update ticket, failure report and self-copy helper |
| `file-transfer/`, `native-cache/` | Transfer jobs and the private native paste cache |

Beside the executable the updater stages `.desk-switch-download`, `.desk-switch-next`, `.desk-switch-previous` and `.desk-switch-update.lock`. Autostart lives in the OS-specific location (`src/autostart.rs`): Windows `Run` registry value, Linux `~/.config/autostart/*.desktop`, macOS `~/Library/LaunchAgents`. User-facing downloads go to `~/Downloads/Desk Switch`.

## 4. Module map

| Module | Responsibility |
| --- | --- |
| `audio/` | System-audio and microphone capture/playback/mixing per OS |
| `autostart.rs` | Login autostart registration |
| `cable.rs` | Bundled official VB-CABLE installers |
| `compatibility.rs` | Version/protocol constants and the `DESKVER2` handshake codec |
| `config.rs` | Identity, peer keys, state dir, atomic writes, file locks, Noise pattern |
| `controller.rs` | Pure state machine for the input/monitor switch handshake |
| `discovery.rs` | mDNS advertisement and peer discovery |
| `edge.rs` | Screen geometry, edges, crossings and route policy |
| `gate.rs` | Atomic input routing gate with lease expiry |
| `group/` | N-computer workspace: consensus, mesh, clipboard, file transfer, layout |
| `gui/` | eframe/egui desktop app, tray, workers, views, update UI |
| `i18n.rs` | Six embedded JSON catalogs and `tr()`/`ui_text()` |
| `keys.rs` | Windows scan-code to evdev key-code mapping |
| `licenses.rs` | Embedded third-party notices for `--licenses` |
| `lifecycle.rs` | Stdin command bus, structured stdout events, shutdown |
| `listen.rs` | Dual-stack TCP listener helper |
| `macos_native.rs` | Verified extraction and launch of the macOS native helpers |
| `platform/` | Native input/monitor backends per OS |
| `transport.rs` | Noise-authenticated `Pipe` for `wire::Message` |
| `updater.rs` | Signed update metadata, download, staging, apply, background jobs |
| `video.rs` | DDC/CI monitor input enumeration and selection |
| `wire.rs` | `Mode`, `Input`, `Message` for the input/video data link |
| `workspace/` | Legacy two-PC pair model, pairing service, devices, diagnostics |

`group/`: `consensus.rs` (durable majority replication), `model.rs` (members, settings, workspace state, ports), `net.rs` (authenticated mesh links), `net/coordination.rs` (majority-committed control), `layout.rs`, `clipboard.rs`, `clipboard_files/` (native file clipboard: `windows.rs` + `windows/cloud.rs`, `linux.rs`/`linux_vfs.rs`/`linux_x11.rs`, `macos.rs`), `file_transfer/` (`protocol.rs`, `transport.rs`, `storage.rs`, `storage/source_move.rs`, `lazy.rs`, `source_trash.rs`).

`gui/`: `mod.rs` (`Desktop`), `view.rs` (rendering/theme), `model.rs` (`Preferences`, argv builders), `session.rs` (legacy pair), `group*.rs` (workspace UI and native worker planner), `software.rs` (Windows CPU-rasterized egui host), `linux_window.rs`, `linux_activation.rs`, `tray.rs`, `popup.rs` (quick controls), `transfer.rs`, `file_progress.rs`, `worker.rs`, `management.rs`, `paired_update.rs`, `preview.rs`.

## 5. `workspace/` versus `group/`

The product calls the linked set of computers a **workspace**. In code the word is used for two different layers:

- `src/workspace/` is the **legacy two-PC pair**. `workspace::Node::validate` requires exactly two identities. It uses `Noise_XX` with a fresh handshake and derives the short pairing code from the handshake hash. It still carries the legacy pairing, remote-management and update bridge.
- `src/group/` is the **N-computer workspace**. `group::model::Workspace` is the replicated state: `hub`, optional `Authority{workspace_id, term, index}`, `revision`, `Settings`, `members`, `active_input`, `active_monitor`, `layout`. A legacy pair is migrated into the first member on load.

## 6. Consensus

`src/group/consensus.rs` is a Raft-like engine with durable `HardState` (`workspace_id`, `term`, `voted_for`, `Configuration`, `learners`, `snapshot_index/term`, `commit_index`, `entries`), pre-vote, joint (two-set) member admission and removal, barriers and snapshots. Timing: 400 ms heartbeats, 2–4 s election timeout, max 256 members, 8 MiB max entry data, compaction after 32 entries or 8 MiB.

`Effect::{Send, Committed, Leadership}` is the only output. The caller must persist `hard_state()` atomically **before** delivering effects, and only `Committed` payloads reach application code. RPC identity comes from the pinned, Noise-authenticated link.

`group/net/coordination.rs` ties the elected consensus **leader** to the workspace **`hub`**: settings changes are serialized through the hub and require a majority of the saved membership (2 of 3, 3 of 4, …). Another coordinator is elected on failure, while device data connections stay direct and independent of the coordinator.

## 7. Wire formats and encryption

- `compatibility.rs`: `PROTOCOL = 9` for the native device link, `MAGIC = b"DESKVER2"`, and `Version::require()` requires native protocol equality and allows adjacent stable patches within a minor line starting at 0.9. Workspace protocol is 3 (`group/model.rs`), management protocol 1, file-transfer protocol 2 / manifest 1.
- **Noise** (`snow`): `Noise_KK_25519_ChaChaPoly_BLAKE2s` for the pinned input/audio/monitor/file-transfer data links, and `Noise_XX_25519_ChaChaPoly_BLAKE2s` for pair/group control links. The version payload travels inside the first handshake messages, so incompatible peers are rejected before any application data.
- `transport.rs`: 1-byte length record framing (max 128 bytes), `TCP_NODELAY`, 1750 ms lease, 250 ms ping, 500 ms I/O timeout; `bincode` with an explicit size limit and `reject_trailing_bytes`.
- `group/net.rs` and `workspace/net.rs`: JSON frames over the authenticated link with 2-byte length prefixes.
- `group/file_transfer/protocol.rs`: metadata-only `Description` first, then a `Manifest` with per-file SHA-256 (max 10 000 entries, 64 GiB total, 4 MiB manifest, depth 64, reserved Windows names rejected). Windows of up to eight 48 KiB chunks with explicit `Offer/Accepted/Ready/Chunk/Ack/Finish/Complete` packets and separate clipboard-offer/lease/native-result messages.

## 8. Platform backends

- **Windows**: custom CPU-rasterized egui host (`gui/software.rs`, `vendor/egui_software_backend`), `platform/win_input.rs`, `win_receive.rs`, `win_monitor.rs`, `win_cursor.rs`, `keys.rs`, WASAPI audio, and native clipboard files through Cloud Files (`group/clipboard_files/windows/cloud.rs`) plus Shell destination discovery.
- **Linux**: X11/XWayland and Wayland through the desktop portal (`platform/portal.rs`, `gui/linux_window.rs`), `ddcutil` for DDC/CI, evdev/reis for input, a FUSE demand-backed file system for native paste (`group/clipboard_files/linux_vfs.rs`), and the GNOME Tray companion (`packaging/linux/gnome-extension`).
- **macOS**: `objc2`/`objc2-app-kit` backends, embedded Swift/C native helpers built by `build.rs`, and the bundled `m1ddc` DDC transport.

## 9. Update and signing

The client reads the latest stable GitHub release, extracts the signed envelope between `<!-- desk-switch-update:start/end -->` markers, and verifies the Ed25519 signature against the public key compiled into the binary (`release/update-public-key.hex`). Only then does it download the platform installer, check URL/size/SHA-256 and the embedded build stamp, stage it beside the executable and hand off to a helper. Windows and Linux updates share the vendored `rust-auto-updator` crate; macOS has its own bundle replacement path. Both update checks and downloads use the same rustls TLS stack with WebPki roots.

`tools/prepare_release.py` verifies native artifacts (PE/ELF/Mach-O architecture, embedded build stamp, public key) and signs the manifest; `tools/ci_sign_release.py` runs it inside the protected `release` environment; `tools/promote_release.py` independently re-verifies downloads before marking a release stable. See [docs/signing.md](docs/signing.md).

## 10. Verification

`cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, `cargo test -p rust-auto-updator --lib`, `cargo build --locked --release`, plus native proofs (`tools/test_linux_file_paste.py`, `tools/test_windows_file_paste.py`, tray lifecycle, portal restore, Debian packaging). Contribution rules and the unsafe documentation policy are in [CONTRIBUTING.md](CONTRIBUTING.md) and [docs/unsafe-audit.md](docs/unsafe-audit.md); planned structural work is listed in [docs/ROADMAP.md](docs/ROADMAP.md).
