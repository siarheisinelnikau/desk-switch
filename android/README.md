# Desk Switch phone remote

Use an Android phone as a keyboard and pointing device for your Desk Switch workspace. Several phones can be paired at once. Check **Shared** beside a phone in **Sharing & volume** to let it follow the selected computer. Uncheck it to use that phone with its paired computer only.

![Desk Switch workspace with connected and saved phones](../docs/android-desktop-workspace.png)

<details>
<summary>Phone routes in the tray popup</summary>

![Desk Switch tray popup with multiple phone remotes](../docs/android-desktop-popup.png)

</details>

1. Open the experimental build of Desk Switch on the computer and the Android app on the phone. Join the same local network on both devices.
2. Select the computer on the phone and confirm the matching code on both screens. Desk Switch remembers approved computers and phones.
3. Tap the pointer mode icon beside **Controls** to switch between **Touchpad** and **Joystick**. Tap **Keyboard** to type in the computer's focused app. **Controls** saves the speed of each mode separately. Use the power icon in the top bar to disconnect.

The pointing surface fades its hint when you use the remote. It brings the hint back after 15 seconds of inactivity.

Under **Controls → Appearance**, choose a light, dark, or system theme and use the system language or choose English, Russian, German, French, Spanish, or Italian. Appearance settings are saved on the phone.

Use two fingers to scroll, or tap once and drag on the second touch. A second quick tap still double-clicks. Hold the **Scroll** joystick between the left and right mouse buttons and move up or down for continuous scrolling. Moving farther changes the speed. **Controls** lets you disable double-tap scrolling, set the gesture and joystick speeds separately, and reverse the scroll direction.

<p>
  <img src="../docs/android-remote.png" width="250" alt="Phone remote with the touchpad and keyboard controls">
  <img src="../docs/android-joystick.png" width="250" alt="Phone remote in joystick mode">
  <img src="../docs/android-settings.png" width="250" alt="Android controls for updates, scrolling, and pointer speed">
</p>

<details>
<summary>Light theme and landscape layout</summary>

<p>
  <img src="../docs/android-light-remote.png" width="250" alt="Light-theme phone remote in Russian">
  <img src="../docs/android-light-settings.png" width="250" alt="Light-theme controls screen in Russian">
</p>

![Landscape phone remote with compact title row and pointer mode toggle](../docs/android-landscape.png)

</details>

<details>
<summary>Finding and pairing a computer</summary>

<p>
  <img src="../docs/android-nearby.png" width="250" alt="Nearby Desk Switch computers discovered over Wi-Fi">
  <img src="../docs/android-pairing.png" width="250" alt="Confirmation code for pairing an Android phone with a PC">
</p>
</details>

Enable **Cursor preview** in the phone's **Controls** and set a rate from 1 to 30 FPS. The area around the cursor fills the pointing surface, which still accepts gestures. The computer captures the preview only when a paired phone requests it. The preview pauses during a workspace switch and stops when the phone leaves the app or turns preview off. Frames are encrypted in transit and held only in memory.

![Phone remote and cursor preview settings in Desk Switch](../docs/android-desktop-settings.png)

The Android app requires Android 8 or later. Install the experimental desktop build on each computer that should receive shared phone input. On Ubuntu, the `ubuntu-phone-remote-experiment` artifact includes a `.deb` named with `android-experiment`. It updates the existing Desk Switch installation and keeps the workspace settings. After restarting Desk Switch, check for **Phone remotes · Experimental** in its settings. Connect to Ubuntu directly from the phone, or mark the phone **Shared** and select Ubuntu in the desktop workspace. Approve the Wayland remote desktop prompt when it appears. Typed text pastes through the target computer's clipboard, and preview on GNOME Wayland uses the companion bundled with the Ubuntu package. If automatic discovery is unavailable, enter the computer's local IP address in the Android app.

## Updates and compatibility

Open **Controls → Updates** to see the installed app version, the device protocol, and the workspace protocol. A phone and computer can use different app versions when their device and workspace protocol versions match.

The app checks for newer experimental Android prereleases in the background on startup. Turn off **Check for updates on startup** to disable that request. **Check for updates** remains available at any time. Use **Download and install** to download the signed APK and approve installation in Android. On the first update, Android might ask you to allow installs from Desk Switch Remote.

Signed updates use the same certificate across releases. A local debug APK can update in place when it uses that certificate. The separate debug builds from GitHub Actions may use a different development certificate. See the [Android release setup](release/README.md) for build and signing details.

<img src="../docs/android-updates.png" width="250" alt="Android update controls with the installed app and protocol versions">

## Build and verify

Use the repository's Rust toolchain, JDK 23, Android SDK Platform 35, Android Build Tools 35.0.0, and Android NDK r28c (`28.2.13676358`). Experimental APK, Windows, and Ubuntu build artifacts are available from [GitHub Actions](https://github.com/siarheisinelnikau/desk-switch/actions/workflows/checks.yml?query=branch%3Aexperiment%2Fandroid-remote).

```sh
cargo build --locked --release --features android-remote
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android
cd android
./gradlew :core:test :app:testDebugUnitTest :app:lintDebug :app:assembleDebug --max-workers=2
cd ..
python tools/test_android_remote.py
```

On Windows use `gradlew.bat`. The APK is at `android/app/build/outputs/apk/debug/app-debug.apk`. For in-place upgrades, build with the debug signing key used by the installed APK. The interoperability check runs the Android transport against an isolated desktop input sink. It exercises pairing, pointer and wheel events, Unicode text, cursor preview, and release on disconnect.

Android uses one Activity and a ViewModel with `StateFlow` for connection and controls state. Gesture rules are Android-independent, preferences are stored by the data adapter, and the Noise handshake uses the same Rust `snow` protocol implementation as the desktop. Existing phone keys and paired computers carry over.

The desktop build uses the existing profile and Wi-Fi announcement. For a temporary Windows test, `DeskSwitch-v0.11.3-android-experiment.exe gui --experimental-mobile` opens the ordinary interface and preserves the previous login startup command. The desktop device and workspace protocols remain 9 and 3. Cursor preview negotiates optional records in the encrypted control stream.
