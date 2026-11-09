# Security policy

## Supported versions

Security fixes are prepared for the latest released version. Upgrade every computer in a workspace to the same version before resuming sharing; mixed protocol versions deliberately wait for a compatible update.

| Version | Supported |
| --- | --- |
| 0.8.x | Yes |
| < 0.8 | No |

## Reporting a vulnerability

Please do not open a public issue for a vulnerability that could be exploited before a fix ships. Use GitHub's **Security → Report a vulnerability** (private advisory) on this repository. If that is unavailable, open a normal issue containing only a short, non-exploitable description and ask for a private channel.

Include:

- affected version and operating systems;
- the trust boundary involved (pairing, network transport, update verification, local state, input injection, file transfer);
- reproduction steps and observed impact;
- whether you have a proof of concept (do not attach a working exploit to a public issue).

You should receive an acknowledgement within a week. Coordinated disclosure is preferred; please allow time for a fix and a signed release before publishing details.

## Trust model

- **Pairing** pins the other computer's Ed25519 public key. Connections are encrypted and authenticated with the Noise protocol (`snow`), and messages are length-delimited with explicit limits.
- **Updates** are verified against an Ed25519-signed `update.json` whose public key is compiled into installed clients. The updater is HTTPS-only, follows a bounded number of redirects, and checks platform, architecture, hashes and build stamps before installing. The private signing key lives only in the protected `release` GitHub environment; see [docs/signing.md](docs/signing.md).
- **Local state** (identity, settings, pending updates) is stored in a per-profile directory created with owner-only permissions on Unix. A lock file prevents two GUI instances from sharing one profile.
- **File transfer** uses a metadata-only offer first; source files are read only after a real paste request, and originals are removed after a Move only once the destination proves a committed, verified paste.
- **Input access** on Linux goes through the desktop portal and udev rules; the app never opens a microphone or input device that the user has not selected.

## Known limitations

- Windows binaries are not Authenticode-signed and macOS apps are not notarized with a Developer ID. Operating-system warnings on first download are expected. The Ed25519 update signature is separate from platform code-signing.
- The Windows CPU renderer and the native shell/cloud integrations use a large amount of `unsafe` FFI. These paths are covered by the native proof tests in CI; see [docs/unsafe-audit.md](docs/unsafe-audit.md) for the ongoing audit policy.
- Logs and diagnostics are held in memory with a bounded history. They may contain computer names and paths; review them before sharing.

## Scope

Feature requests, usability problems and general bugs belong in the issue tracker. Do not report missing platform code-signing certificates as a vulnerability — it is a documented distribution limitation, not a defect in the transport or update verification.
