# Release signing

Desk Switch signs its update manifest with the existing Ed25519 identity. This binds the hashes of the Windows EXE and Debian package to the public key pinned in installed clients. Historical v0.5.0 metadata also covers its experimental Apple Silicon/Intel macOS app ZIPs; that signed matrix is preserved. It does not provide Windows Authenticode, a trusted Windows publisher certificate, or macOS Developer ID signing/notarization.

## GitHub setup

Use the repository's **Settings → Environments → release** environment, with selected deployment refs: branch `main` and tag pattern `v*`. Keep the credential scoped to this repository and environment. Only the `sign` job declares this environment; build and publication jobs do not receive its key.

The environment requires one secret:

| Secret | Source |
| --- | --- |
| `DESK_SWITCH_UPDATE_SIGNING_KEY` | The existing `.release-private/update-signing-key.pem`, including PEM headers |

With Python 3.11+, OpenSSL, and an authenticated [GitHub CLI](https://cli.github.com/) available, run from any directory:

```sh
python3 tools/upload-signing-secret.py --check
python3 tools/upload-signing-secret.py
```

Use the absolute script path if the current directory is outside this checkout. The uploader checks the private/public key pair before any upload, fixes the destination to `siarheisinelnikau/desk-switch`, passes the value to `gh secret set` through stdin, and verifies the saved secret name. It does not print the key or put it in command arguments. Existing secrets require an explicit `--replace`; use that only to restore the same verified identity or recover from an interrupted upload.

On 2026-11-07, the `release` environment and its ref restrictions were configured, and `DESK_SWITCH_UPDATE_SIGNING_KEY` was saved through the existing authenticated GitHub browser session. The v0.4.9 release subsequently validated the complete build, signing and stable-promotion workflow. Its public manifest signature and installer digests were independently verified. The CLI uploader above remains an optional recovery/replacement path; missing or mismatched credentials deliberately stop publication.

## Release behavior

### Release from any computer

1. Commit a new package version in `Cargo.toml` and `Cargo.lock`, matching `release/publish-request.json` and `release/notes/VERSION.md`. Updating the request on `main` is the existing automatic release trigger.
2. For a manual run, open **Actions → Release → Run workflow** in GitHub and select `main`. Repository Actions access is required; no local SDK, private key, or CLI login is needed.
3. Leave **Publish a new version after build and signing checks** unchecked for a build-and-sign verification. The installers and public signed metadata are available as Actions artifacts; no release is created or changed.
4. Enable that option to publish a prepared new version. The workflow builds, signs, verifies the public installers, and promotes stable. Tags and previously published versions are preserved; prepare a new version instead of replacing an existing one.

The request file also accepts `"hold": true`. While held, its gate skips native builds, signing and publication, including manual workflow runs. Omitting `hold` or setting it to `false` allows the normal pipeline. A newer request on the same Git ref cancels an unfinished older run through workflow concurrency; the latest unpublished request supersedes it. Published releases are never rewritten by cancellation. Clear the hold only when the new source and release notes are ready; leave the manual publication option unchecked for a build-and-sign verification.

Current release builds run without signing credentials on GitHub-hosted Ubuntu, Windows and macOS runners. Both macOS architectures are built natively and their app ZIPs are part of the signed, published asset matrix. The signing job downloads every installer from the same workflow run, verifies versions, embedded build stamps, PE/ELF and Mach-O architectures, Debian and app-bundle metadata and the pinned public key using `tools/prepare_release.py`, then signs the complete manifest. Missing artifacts or mismatched keys stop the job. The private PEM is written with owner-only permissions under `RUNNER_TEMP`, removed on exit, and never included in artifacts. Actions in this workflow are pinned to verified full commit SHAs, and checkout does not retain Git credentials.

The public `update.json` is retained as the `desk-switch-signed-metadata` artifact for 30 days. The publication job receives it without receiving the key, publishes the tested native artifacts, and runs the stable-promotion verifier. That verifier independently checks the signature, public downloads, hashes, architectures, build stamps and package/bundle metadata before attaching the signed envelope to readable release notes and marking stable. Releases from 0.6.2 onward publish four versioned assets: the Windows EXE, the Debian package, and the Apple Silicon and Intel macOS app ZIPs. Version 0.5.0 keeps its historical four-artifact matrix and is never rewritten; releases 0.5.1 through 0.6.1 shipped only the Windows EXE and Debian package. The asset set is defined by `tools/release_assets.py`, and the signing and promotion verifiers reject matrix changes for an already published tag. Local signing with `tools/prepare_release.py` remains available; committing that metadata as `release/manifests/VERSION.json` still requests the separate promotion workflow. Do not overwrite existing tags or releases, or rerun an already published version to change its assets. The workflow retains versioned platform/architecture names and checks existing candidates without replacing their bytes.

## Backup and recovery

Keep an independent encrypted backup of the existing private key outside the development computer, with recovery access stored separately. Verify a restored key using the uploader's `--check` before replacing a GitHub secret. GitHub's UI and Secrets REST API do not return the saved value; code running in an authorized workflow can access it, so Secrets is not an independent backup vault.

Do not regenerate this Ed25519 key during routine setup: clients already trust its public counterpart. Loss or compromise needs an explicit client migration and key-rotation plan. Restrict who can change workflows or create release refs, review changes to release tooling, and keep signing secrets out of pull-request jobs and untrusted self-hosted runners. Environment ref restrictions complement repository branch/tag protection; they do not stop a maintainer with write access from changing trusted workflow code. No new reviewer gate or repository ruleset was added by this setup.

## References checked on 2026-11-07

- [GitHub: secure use of Actions](https://docs.github.com/en/actions/reference/security/secure-use): least privilege, individual secrets, reviewed dependencies, and immutable action references.
- [GitHub: managing environments](https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/manage-environments): environment secrets and deployment branch/tag restrictions.
- [GitHub CLI: secret set](https://cli.github.com/manual/gh_secret_set): values from stdin and environment-scoped secrets.
- [GitHub Secrets REST API](https://docs.github.com/en/rest/actions/secrets): metadata can be retrieved without returning saved values.
