# Desk Switch

Two computers. One workspace. Share a keyboard, mouse, system audio and microphone between Windows and Ubuntu, with optional switching of a shared monitor's video input. Windows–Windows and Ubuntu–Ubuntu pairs use the same pairing flow.

![The green PC lends its keyboard and mouse to the purple PC](docs/desktop.png)

## Install

Get the **same version on both PCs** from [Releases](https://github.com/siarheisinelnikau/desk-switch/releases/latest). New releases contain exactly two downloads:

- **Ubuntu 24.04/26.04 x86-64:** `desk-switch-linux-x86_64.deb`.
- **Windows x86-64:** `desk-switch-windows-x86_64.exe`.

```sh
sudo apt install ./desk-switch-linux-x86_64.deb
```

Open **Desk Switch** from Ubuntu's application menu or run the Windows executable. New profiles use **English** and **Dark**. Existing language/theme choices are preserved. Russian, German, French, Spanish and Italian are also available. Language and theme changes apply immediately to the window and tray.

## Link your PCs

1. Turn on **Available on the network** on both PCs. Each advertises itself and discovers other computers at the same time. Different application/protocol versions remain visible with an update hint.
2. Click **Link** beside the other PC. On that PC, click **Accept**. The application creates and exchanges keys automatically. Both screens show the same optional confirmation code: no tokens, IP addresses or manual key copying.
3. Once settings are synchronized, one PC is purple and the other green. Both screens agree on those identities. **This computer is always on the left** of its own window. Everything stays grey before synchronization and after a lost connection.

For a routed network or when multicast is unavailable, open **Connect by IP address** and enter the other PC's IP (optionally `IP:port`, or `[IPv6]:port`). Link and Accept work the same way; an existing pinned identity cannot be replaced by changing an address. A route or VPN and firewall access are required across subnets. The default TCP ports are **45878** for pairing/settings, **45876** for input, **45877** for system audio and **45879** for the microphone. This does not provide NAT traversal.

Approval is required before either key is pinned. Existing peers reconnect without another prompt. **Unlink computers** removes a live pair on both PCs; an offline peer can also be unlinked locally. Disabling availability stops discovery, the control connection and shared devices. Allow Desk Switch on your private LAN if the operating system requests firewall permission.

## Devices follow the pair

- **Keyboard and mouse:** the PC with both physical devices supplies input. With devices on both PCs, each controls itself by default; choose a shared source if wanted. Devices are checked every three seconds, so temporarily connecting input for setup does not require changing roles manually. Detected devices light up even while the sharing service is starting or needs system access. Automatic capture supports relative mice, including composite gaming receivers with auxiliary volume axes, and keyboards with F11/F12; touchpads are not host pointers.
- **Shared speaker:** choose the PC with the shared speakers or headphones. Both PCs' sound plays together there. Two sliders, **This computer** and **Other computer**, each provide **0–100%** volume and mute. Each level belongs to its source PC, stays with it when the listening PC changes, and can be changed from either computer. Automatic speaker selection prefers a default headphone/Bluetooth output, then speakers, then HDMI/DisplayPort. Disable **Use shared speaker** to stop forwarding and restore native application levels. Audio stays independent of input switching and never forwards a remote mix back to its source. No virtual audio driver is required.
- **Switching:** click **Switch keyboard and mouse** or press **F12**. **F11** returns input to the physical keyboard PC. Optionally enable screen-edge crossing and select this PC's left or right edge. The return edge is configured automatically; the diagram keeps this computer on the left.
- **Colours:** a device retains its source PC's colour on the other PC. A lent keyboard/mouse becomes grey at the source after handoff is confirmed. Headphone colours identify the PC with the shared speaker; two coloured dots indicate a connected, unmuted remote audio stream. For a shared display, PC towers appear at the sides and one monitor in the middle. The monitor takes the colour of its confirmed active video input and stays grey while the input is unknown.

Shared choices made on either PC propagate automatically. Device inventories and selections are synchronized; monitor calibration stays local. Device errors provide a retry action and details under **Diagnostics**.

Keyboard, mouse, shared speaker and shared monitor can each be enabled separately. Unshared input stays local. The receiving PC must retain a keyboard and mouse, either its own devices or the shared ones: a handoff that would leave it without input is blocked. While its only input is being shared, the corresponding disable controls are locked on both PCs; return input first with F11. Changing an allowed input option releases held keys before reconfiguring the service.

The two audio gains are applied before the physical output's system volume. For example, a 50% source gain followed by a linear 90% output gain gives 45% amplitude; the physical system volume slider may use its own nonlinear curve. The system volume slider remains attached to its actual local output. Native source control uses application/session volumes, preserves existing channel balance and muted apps, and journals original levels for shutdown/crash recovery. Standard shared desktop audio is supported; exclusive or encoded passthrough streams are outside this mixer.

![Independent source volumes in the shared speaker settings](docs/shared-audio.png)

## Choose devices

**Settings → Device selection** keeps Auto as the default and lists each PC's devices. Either PC can choose the other PC's output or microphone. A missing explicit selection pauses the corresponding service instead of falling back. Linux keyboards and relative mice can be selected separately, including composite HID interfaces; Windows shares its keyboard/mouse group because the current hooks cannot reliably suppress one individual physical device. Return input before changing its source or bindings.

An explicit speaker/headphone output receives the other PC's sound there. Local applications retain their OS routing: select the same output in the OS/apps to hear both PCs. Native source volume and the two synchronized PC sliders remain separate from device selection.

## Shared microphone

**Shared microphone** is off by default. Enable it, leave the source on Auto or choose its PC, then select **Desk Switch microphone** in the receiving calling/recording application. Forwarded gain and **Mute microphone** synchronize from either PC; mute stops capture. Local applications on the source can still use the physical microphone normally.

Auto binds to one physical endpoint when sharing starts. Changing the OS default or unplugging that microphone never opens a different microphone automatically. Choose a replacement in Settings or disable/re-enable sharing to resolve Auto again. The microphone has its own authenticated transport and remains independent of the shared speaker, input and monitor. It is never played to ordinary speakers or selected again as a capture source.

- **Ubuntu receiver:** PipeWire creates a temporary virtual source. It disappears when sharing stops; Desk Switch does not set the OS default input.
- **Windows receiver:** install [VB-CABLE](https://vb-audio.com/Cable/) if needed. Desk Switch renders only to a detected cable playback endpoint; choose its paired **CABLE Output** recording endpoint in the calling app. Keep your regular headphones/speakers as the Windows playback output; select CABLE Output only as the calling app's microphone. The cable is excluded from shared-speaker selection. The application provides the setup link and does not silently install or redistribute the driver. Sending a physical Windows microphone to Ubuntu does not require this driver on Windows.

![Shared microphone and two source volume controls](docs/shared-microphone.png)

![Choose devices for either PC](docs/device-selection.png)

## Optional shared monitor

Connect the physical monitor to both PCs, using different HDMI/DisplayPort inputs. On **each** PC, expand **Shared monitor**, choose the local display, and enable **This is a shared monitor — switch it with input** while viewing that PC's input. The application reads and remembers that PC's current connection and exchanges it with its partner. There is no manual entry of the other PC's input code. Read-only status checks keep the central monitor's colour current without changing its input.

DDC/CI writes are used only when both PCs opted in, the monitor identities match, and their saved input values differ. Reading a connection never switches the display. Until setup is complete, F12 switches input devices only. Enable DDC/CI in the monitor's own menu if its connection cannot be read. Monitors without a unique serial need matching EDID data. Re-read the local connection after moving a cable. Other displays are not switched.

![This computer stays on the left; the central shared monitor shows its active input](docs/shared-monitor.png)

DDC/CI depends on the monitor and firmware. The previously tested LG setup reported “out of range” and is not considered validated. There is no automatic scan through possible video inputs or display mode changes.

## Window, tray and startup

**X** and **Quit application** stop services and exit. **Minimize to tray** is separate. Quit first requests cooperative input release and audio shutdown; an unresponsive child is terminated after six seconds. Closing the GUI command pipe also requests shutdown. Losing the tray brings a hidden window back.

Autostart is enabled after the first launch and can be disabled in **Settings**. It starts in the tray using the saved profile and availability choice. Ubuntu needs an AppIndicator/StatusNotifier host for a visible tray icon.

![Settings: English and Dark are the initial defaults](docs/appearance-dark.png)

## Updates

Installed Debian packages and the Windows executable use [`rust-auto-updator` 0.4.1](https://github.com/siarheisinelnikau/rust-auto-updator). The original core is included under `vendor/` with its licence and source hashes. The host additionally verifies a pinned Ed25519 signature covering each installer's exact URL, version, size and SHA-256. Signed metadata lives in the release description, keeping the download list limited to `.deb` and `.exe`.

Checks/downloads run at startup and every six hours. Click **Update and restart** to install; Ubuntu uses the standard system password dialog. Cancellation keeps the application open for retry. Settings and pairing survive updates. Both PCs must run the same application and protocol version before input or audio is accepted. Updates do not restart both machines simultaneously.

For the first upgrade from **0.3.x or older**, install the new `.deb` / replace the `.exe` once: older versions expect a separate metadata download. Subsequent updates use the two-file release format. Portable Linux binaries are no longer distributed. The `.deb` runs `/usr/lib/desk-switch/desk-switch` through its installed launcher so package ownership and future system updates work correctly.

## Platform access

Ubuntu uses the desktop RemoteDesktop/EIS portal. Approve the system dialog when it appears. A Linux keyboard host also needs read access to the selected `/dev/input/event*` devices. If access is denied, Desk Switch automatically requests administrator approval once for the selected devices. Approve the Ubuntu system dialog; **Allow keyboard and mouse access** remains available for a manual retry. Cancelling does not trigger repeated prompts on reconnect. The package's narrow polkit helper validates physical keyboard/relative-mouse interfaces and grants only that desktop user read access to the selected device nodes; it cannot authorize arbitrary files. No input-group membership is needed. Access lasts until unplug/recreation of those nodes, so a replug may need approval again. The input service retries without recreating the pair. Brief settings-channel disconnects keep the existing input session; Wayland restore tokens are stored privately and rotated after each successful start when the desktop supports persistent consent. The GUI runs as your desktop user; only the narrow access helper is elevated. Pairing alone never grants OS permissions. PipeWire/WirePlumber provide audio. The Debian package requires `ddcutil`, `pipewire-bin`, `pipewire-pulse`, `pulseaudio-utils`, `acl` and `python3`, so APT installs the monitor/audio tools automatically. Native audio gain uses PulseAudio-compatible application streams on PipeWire, excluding Desk Switch playback. Monitor switching remains opt-in and needs I²C access. Use **Set up shared monitor…** in either tray, then remember the current connection on each PC while its own picture is visible. **Switch shared monitor** is available independently of keyboard forwarding when both PCs have their own input devices. When input is shared, switching moves the monitor and input together; switch back from the destination PC’s tray. No unconfigured video input is tried. The package recommends the desktop portal and WirePlumber.

Windows uses native hooks, SendInput and WASAPI loopback/shared playback. Native source gain uses Core Audio session volumes and excludes every Desk Switch playback/microphone session using a shared grouping identity. DDC/CI uses Windows' built-in monitor API (`GetVCPFeatureAndVCPFeatureReply` / `SetVCPFeature` in `Dxva2.dll`), so no separate monitor utility is required. Input cannot control a higher-privilege application without matching system permission. Headphone/Bluetooth detection is a default-output heuristic; explicitly selecting the shared speaker PC overrides it everywhere.

## Development and release

Requires Rust 1.99+, OpenSSL/X11/Wayland development libraries on Linux, and `dpkg-dev` / `desktop-file-utils` for packaging.

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 tools/test_input_access.py
cargo build --locked --release
python3 tools/package_deb.py --binary target/release/desk-switch --out dist
python3 tools/verify_deb.py dist/desk-switch-linux-x86_64.deb
```

Tests use synthetic input/audio/monitor devices and loopback peer connections. They cover approval/rejection, automatic key exchange, manual IP connections, pin protection, reconnection from either side, simultaneous requests, paused-GUI event delivery, shared decisions, capability changes, monitor mappings, version rejection, signed metadata and input release. GUI screenshots use isolated fixture PCs, a private Xvfb display and private D-Bus; they never switch a real monitor or grab input. The portal regression test (`/usr/bin/python3 tools/test_portal_restore.py --binary target/debug/desk-switch`, requiring `python3-gi` and `dbus`) creates its own D-Bus and fake portal to verify persistent consent, token rotation and cancellation without touching the compositor. Native CI builds/tests on Ubuntu and Windows. `dbus-run-session -- python3 tools/test_microphone_pipewire.py` (PipeWire and WirePlumber 0.5+) validates microphone PCM forwarding, live gain and virtual-input/capture cleanup on a separate server using a synthetic tone, with hardware monitoring disabled. Real two-PC multicast, interactive Windows, VB-CABLE and monitor firmware still need target-machine validation. Use **Diagnostics → Copy diagnostics** to capture both PC states and recent service messages without private keys. The explicit CLI remains available through `--help`.

Set `release/publish-request.json` to the new version tag to build a prerelease. Download the two installers, then sign locally:

```sh
python3 tools/prepare_release.py --tag vVERSION --windows PATH.exe --deb PATH.deb --out dist/signed
```

Commit the metadata as `release/manifests/VERSION.json`. Promotion verifies the signature, public downloads, package metadata and both embedded build stamps before publishing stable. The signing key stays in the ignored `.release-private/` directory and is never sent to GitHub. Back it up. Existing tags and signed releases must not be reused.
