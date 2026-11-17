# Noise-Java

Unmodified MIT-licensed sources from [rweather/noise-java](https://github.com/rweather/noise-java), commit `49377b6dfc6a1e75740bce2318118291a57c0d6e`. Only `src/main/java` and `LICENSE.txt` are included. No custom cryptographic implementation is used by Desk Switch.

Desk Switch uses `Noise_XX_25519_ChaChaPoly_BLAKE2s` for pairing and `Noise_KK_25519_ChaChaPoly_BLAKE2s` for the pinned input connection, matching the desktop implementation. Compatibility is checked against the desktop Rust/snow implementation over isolated sockets.
