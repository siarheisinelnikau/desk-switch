# Desk Switch

Use one Windows keyboard and mouse with a Windows PC and an Ubuntu Wayland PC on the same LAN. **F12** switches the shared monitor's physical HDMI/DP input and redirects keyboard and mouse. **F11** immediately returns input to Windows and, after a full switch, asks the available monitor adapters to restore Windows video. An optional screen edge can transfer input alone or input and video together.

## Build

Build on each PC with [rustup](https://rustup.rs/) and `cargo build --release --locked`. The repository pins Rust and dependencies. On Windows use the MSVC Rust toolchain and Visual Studio Build Tools with the C++ workload. For a static MSVC CRT, run `$env:RUSTFLAGS = '-C target-feature=+crt-static'` in PowerShell before building. On Ubuntu install `build-essential` for a native build. The executables are `target/release/desk-switch.exe` and `target/release/desk-switch`.

On Ubuntu, run as your logged-in desktop user. Full switching requires `ddcutil`, the `i2c-dev` driver and access to the selected `/dev/i2c-*` device. Use your distribution's package and device permission rules. The Wayland receiver needs the user session D-Bus, `xdg-desktop-portal`, and a backend with RemoteDesktop v2 and EIS, such as a compatible GNOME/Mutter session with `xdg-desktop-portal-gnome`. No X11 backend is included.

## Set up once

1. On each PC, run `desk-switch init`. Copy the displayed **public** key directly from each trusted PC to the other, and run `desk-switch pair OTHER_PUBLIC_KEY` on each. `desk-switch public-key` displays an existing key. Keep the `identity` file private. `desk-switch state-dir` prints the settings directory.
2. Turn on DDC/CI in the shared monitor if it has that setting. With the shared monitor showing Windows, run `desk-switch probe` on Windows. Identify the shared display from the screen name, position and model, then record its **Monitor ID** and current VCP `0x60` value. Select the Ubuntu input once on the monitor and run the same command on Ubuntu. Record the Ubuntu-side ID and current value. Use `probe 'MONITOR_ID'` to query just the selected display. Return the shared monitor to Windows for the first run.
3. Put `settings.toml` in the state directory on **both** PCs. Use each PC's local monitor ID and the same two input values on both PCs. These example values are placeholders.

   ```toml
   [monitor]
   id = 'paste the Monitor ID for this PC'
   windows = 0x11
   ubuntu = 0x0f
   ```

`probe` lists advertised input codes when available. Monitor capabilities can omit inputs and do not identify which PC is connected to each port. The two active-input readings establish that mapping. Windows pins the monitor interface and EDID fingerprint. Ubuntu pins the DRM connector and EDID fingerprint and resolves its I²C bus at the time of use. An unavailable or ambiguous identity blocks the operation. After moving cables or changing display topology, run `probe` again and update the ID if it changed.

## Run

Start the Ubuntu receiver from a terminal in its Wayland session, using its actual LAN address. Approve keyboard and pointer access in the portal dialog. Desk Switch keeps that consent session open across Windows reconnects. A new session may prompt again after restart or compositor revocation. The receiver opens only the input portion of RemoteDesktop.

```sh
./target/release/desk-switch run 192.168.1.20:45876
```

Then start Windows with the **same Ubuntu LAN address**. If you use a firewall, allow this TCP port on Ubuntu from the Windows PC.

```powershell
.\target\release\desk-switch.exe run 192.168.1.20:45876
```

Wait for the peer authentication message. Release any held keys or buttons, then press **F12**. Press **F12** again to return. Press **F11** for emergency recovery. With Windows input active, Ctrl+C stops either instance from its own terminal.

## Optional edge mode

Add an `[edge]` section on both PCs. Use the same mode on both and choose each side separately. For a Windows screen to the left of Ubuntu, select `right` on Windows and `left` on Ubuntu.

```toml
[edge]
mode = "input"           # off, input, or video
screen = '\\.\DISPLAY2'   # Windows example, exact Screen name from probe
side = "right"
```

On Ubuntu, `desk-switch portal-check` queries capabilities without requesting consent. `desk-switch portal-check --consent` opens a temporary keyboard/pointer session and prints available EIS screen IDs such as `0,0,1920,1080`, without injecting events. Use that exact ID in the Ubuntu `screen` field. Bidirectional edge mode requires the selected EIS absolute pointer region. Recheck the region after changes to display layout or scaling.

Only the configured screen edge transfers between PCs. Dragging or holding a key prevents edge entry. Edge-originated transfers place the cursor just inside the destination edge at the matching proportional position. Leaving through the configured Ubuntu return edge reverses that transfer. The `input` mode makes no DDC calls and works without a `[monitor]` section. F12 while using input-only mode upgrades to a full transfer to Ubuntu if video is configured. F12-initiated transfers use relative motion and return with F12 or F11.

## Optional audio from Ubuntu

Keep your headphones connected to Windows. Windows plays the Ubuntu stream on the same output as local apps through shared-mode WASAPI. The audio feature starts off. Running Desk Switch for input and video does not start it.

1. Reuse the peer keys already pinned on both PCs. In a separate terminal in the Ubuntu desktop session, run `./desk-switch audio sources` to list playback sinks. Then explicitly enable the sender.

   ```sh
   ./desk-switch audio serve 192.168.1.20:45877
   # To select a particular sink monitor
   ./desk-switch audio serve 192.168.1.20:45877 --source 'SINK_NAME'
   ```

2. On Windows, use `.\desk-switch.exe audio outputs` to list active outputs and their endpoint IDs. Start the receiver in a separate terminal. With no `--output`, it uses the default Windows playback device.

   ```powershell
   .\desk-switch.exe audio play 192.168.1.20:45877 --volume 50
   # To select a particular playback device
   .\desk-switch.exe audio play 192.168.1.20:45877 --output 'ENDPOINT_ID' --volume 50
   ```

Use a different TCP port from input and video. If you use a firewall, allow the audio port on Ubuntu from the Windows PC. The sender opens capture only after the paired Windows receiver requests audio. The stream runs independently of F11, F12, and edge switching.

In the Windows audio terminal, enter `volume 0` through `volume 100`, `mute`, or `unmute` to adjust only Ubuntu audio. `status` shows the connection and volume. `off` closes the stream and releases the output, and `on` starts a fresh connection. `quit` or Ctrl+C exits. Ctrl+C in the Ubuntu audio terminal disables the sender. Volume and mute settings last for that receiver process. You can pass `--mute` on startup.

Ubuntu audio needs `pipewire-bin` (`pw-dump` and `pw-record`) and a running PipeWire session with WirePlumber linking policy. It captures the monitor of the selected playback sink. Choose a sink that stays present during video switching. If your HDMI audio sink disappears when the monitor changes input, use a persistent output and route your Ubuntu apps to it. Ubuntu may also play the audio on that sink. The sender does not change system audio routing.

The LAN stream is one-way, 48 kHz stereo PCM, approximately 1.54 Mbit/s before framing. Audio uses its own authenticated connection and small buffers. On a stalled or disconnected link the receiver discards queued audio and reconnects while enabled. Local Windows playback stays on the normal shared output. [WirePlumber linking policy](https://pipewire.pages.freedesktop.org/wireplumber/policies/linking.html) · [WASAPI shared streams](https://learn.microsoft.com/en-us/windows/win32/coreaudio/rendering-a-stream)

## Compatibility and recovery

- Full switching needs readable and writable VCP `0x60` from both PCs while each PC is displaying on the shared monitor. Some monitors block DDC from inactive inputs, so the source PC writes and the destination PC reads back the selected input. Firmware that reports a stale or incorrect value can still prevent a reliable handoff. Verify both directions with the actual hardware.
- A failed handoff, broken authenticated link, saturated input queue, or expired lease restores local Windows input. The Ubuntu receiver releases its emulated keys and buttons. Monitor recovery is best effort. If the active PC crashed or its DDC adapter cannot reach the monitor, select the Windows input in the monitor menu.
- Windows captures normal input in the interactive user desktop. Secure desktops and apps with exclusive device capture require separate validation. Common physical key and mouse button mappings use the Ubuntu keyboard layout. F11 and F12 are reserved while Desk Switch is running.

## Verify

`cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo test --locked`. Tests use in-memory devices, synthetic PCM, and loopback TCP. They cover audio off, reconnect, loss, stale frames, mute, and audio during a control round trip. On hardware, first read `0x60` on both active inputs. Then check F12 in both directions with the second monitor visible, F11 and network-loss recovery with a held key, and each edge mode in both directions with your actual layout and scaling.

[ddcutil troubleshooting](https://www.ddcutil.com/faq/), [setvcp](https://www.ddcutil.com/command_setvcp/), [Win32 SetVCPFeature](https://learn.microsoft.com/en-us/windows/win32/api/lowlevelmonitorconfigurationapi/nf-lowlevelmonitorconfigurationapi-setvcpfeature), [Wayland RemoteDesktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html).

MIT licensed.
