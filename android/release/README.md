# Android prereleases

## Configure signing

Create the `android-release` environment in the canonical GitHub repository. Set its deployment rule to allow only tags matching `android-remote-v*`. Restrict that tag pattern to trusted maintainers if other contributors can push tags. If the environment uses reviewers, review the tagged commit and workflow before approving signing.

Use the certificate and private key that signed the distributed experimental APK so Android can preserve existing settings and pairings on upgrade. Import that key into a password-protected PKCS#12 keystore under the alias `deskswitch-remote`. Verify the certificate SHA-256 fingerprint against the existing APK using `apksigner verify --print-certs`. Commit only the lowercase hexadecimal fingerprint in `android/release-signing.sha256` and keep an encrypted offline backup of the keystore.

Add both secrets to the `android-release` environment.

| Secret | Value |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | PKCS#12 file encoded as a single Base64 line |
| `ANDROID_KEYSTORE_PASSWORD` | Single-line password for the PKCS#12 store and private key |

## Publish an update

1. Increase `versionCode` in `android/app/build.gradle.kts` above every previously distributed Android build. Set `versionName` to `X.Y.Z-experimental` and add English notes at `android/release/X.Y.Z.md`.
2. Create the tag `android-remote-vX.Y.Z` on the reviewed commit in the canonical repository. The workflow runs the Android unit, lint, and desktop interoperability checks and builds an unsigned release APK.
3. Once the build passes, the signing job verifies the APK identity and certificate and passes only the signed APK to the publisher. The publisher checks its SHA-256 digest and uploads it with the prerelease.
4. Verify the prerelease lists `DeskSwitchRemote-vX.Y.Z-experimental.apk`. The Android app discovers this experimental prerelease via GitHub Releases.

The Android workflow uses a separate tag prefix and publishes its releases with GitHub's Latest flag disabled.
