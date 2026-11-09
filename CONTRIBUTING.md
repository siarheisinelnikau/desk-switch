# Contributing

## Prerequisites

- Rust 1.99 or newer (`rustup show active-toolchain`).
- Linux: `pkg-config libssl-dev dpkg-dev desktop-file-utils python3-gi dbus udev openssl polkitd fuse3 xvfb openbox wmctrl x11-utils nautilus xdotool libgl1-mesa-dri libxkbcommon-x11-0`, plus Node.js for the GNOME companion check.
- Windows: MSVC build tools, Python 3 and PowerShell 7.
- macOS: Xcode command line tools; both `aarch64-apple-darwin` and `x86_64-apple-darwin` targets.

## Build and check

Run the same commands as CI before opening a pull request:

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo test --locked -p rust-auto-updator --lib
cargo build --locked --release
```

`Cargo.lock` is committed. If a dependency change is intentional, regenerate the lock file and commit it together with `Cargo.toml`; never push a branch that only builds with an unlocked resolution.

Platform native proofs:

```sh
# Ubuntu, isolated desktop
/usr/bin/python3 tools/test_linux_file_paste.py --out target/native-file-paste-proof
# Windows, hosted runner only
python tools/test_windows_file_paste.py --out target/windows-native-file-paste-proof
```

ACL and recoverable-recycle proofs are required on Windows CI; the Explorer and Cloud Files proofs need an interactive shell and a quiet clipboard, so they are advisory on hosted runners. Do not weaken those gates to make a local run pass.

## Conventions

- **Formatting and lints.** `cargo fmt --all` and `cargo clippy -- -D warnings` are mandatory. Do not add `#[allow(...)]` without a comment explaining why.
- **`unsafe`.** Every `unsafe` block must carry a `// SAFETY:` comment stating the invariant (handle ownership, pointer lifetimes, threading/COM apartment, alignment). Prefer a narrow safe wrapper over a raw `windows-sys` call. See [docs/unsafe-audit.md](docs/unsafe-audit.md).
- **Localization.** User-visible strings go through `i18n::tr`/`ui_text`. Catalogue keys are source strings; a new key must be added to **all six** files in `locales/` with matching placeholders. `tools/check_i18n_keys.py` verifies that literals passed to translation helpers exist in every catalogue. Prefer typed error codes translated at the UI boundary over translating free-form error text.
- **Third-party notices.** Any dependency change requires regenerating `THIRD-PARTY-NOTICES.md` with `python3 tools/gen_third_party_notices.py`. The file is embedded in `--licenses`, shipped in the Debian package and in the macOS app bundle.
- **No secrets.** Never commit private keys, signing material, tokens or profile state. Release signing uses the protected `release` environment only.
- **Commits.** Keep commits small and focused, with a subject that says what changed and why. One logical change per commit is easier to review and to revert.

## Pull requests

Open PRs against `main`. CI runs the Ubuntu and Windows checks, plus the macOS matrix on relevant changes. All checks must be green before merge; if a platform-specific test cannot run locally, say so explicitly in the PR description rather than disabling it.

Describe user-visible changes and update `release/notes/` only when preparing an actual release. Release mechanics, signing and promotion are documented in [docs/signing.md](docs/signing.md); the module map is in [ARCHITECTURE.md](ARCHITECTURE.md).
