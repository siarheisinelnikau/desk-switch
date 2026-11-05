# Desk Switch

Share one keyboard and mouse, switch one selected monitor's HDMI/DP input, and listen to the other PC's system audio. Windows ↔ Windows, Ubuntu Wayland ↔ Ubuntu Wayland and mixed pairs use the same protocol. The **host** keeps the physical keyboard and mouse attached. The other PC is the **receiver**.

**F12** switches video and input in both directions. **F11** immediately restores host input and attempts to restore host video. A selected screen edge can transfer input alone or input with video. Audio has separate on/off and volume controls and continues across input and monitor switches.

## Build

Install [rustup](https://rustup.rs/) and run the following on each PC.

~~~sh
cargo build --release --locked
~~~

Rust and dependencies are pinned. The binaries are target/release/desk-switch.exe and target/release/desk-switch.

Windows needs Windows 10/11. Native MSVC builds need Visual Studio Build Tools with the C++ workload. For a static MSVC CRT set the RUSTFLAGS environment variable to -C target-feature=+crt-static before building.

Ubuntu needs build-essential to build. Run as your logged-in desktop user. Input uses the user session D-Bus, xdg-desktop-portal and a backend with RemoteDesktop v2 and EIS, such as compatible GNOME/Mutter with xdg-desktop-portal-gnome. Audio needs pipewire-bin and WirePlumber policy.

Video switching on Ubuntu additionally needs ddcutil, i2c-dev and access to the selected /dev/i2c-* device. Use your distribution's package and device permission rules.

## Set up once

1. On each PC run desk-switch init. Copy each public key directly from the trusted PC to the other and run desk-switch pair OTHER_PUBLIC_KEY on both. Keep the identity file private. The public-key command displays an existing public key, and state-dir prints the settings directory.
2. Choose the host, regardless of OS.
3. Enable DDC/CI in the shared monitor. With it showing the host, run desk-switch probe on the host and record the selected monitor's ID and current VCP 0x60 value. Select the receiver input once during setup, run probe on the receiver and record its local ID and current value. Return the monitor to the host input.
4. Put settings.toml in the state directory on both PCs. Use each PC's local monitor ID and the same two input codes on both.

~~~toml
[monitor]
id = 'local Monitor ID for the shared physical monitor'
host = 0x11
peer = 0x0f
~~~

The example input codes are placeholders. Probe lists advertised inputs when available. Those codes do not identify which PC is connected to each port, so the two active-input readings establish that mapping. Passing a Monitor ID to probe queries just that display.

Windows pins the monitor interface and EDID fingerprint. Ubuntu pins the DRM connector and EDID fingerprint and resolves its I²C bus at use time. An unavailable or ambiguous selection blocks DDC. The second monitor keeps its input. Recheck IDs after moving cables or changing topology.

Existing windows and ubuntu settings remain accepted as aliases for host and peer, respectively. Use role names for new setups.

## Run either OS in either role

Start the receiver first using its LAN address. Use the same address on the host. On Windows replace ./desk-switch with .\desk-switch.exe.

~~~sh
# PC receiving input, Windows or Ubuntu
./desk-switch run --role receiver 192.168.1.20:45876

# PC with the physical keyboard and mouse, Windows or Ubuntu
./desk-switch run --role host 192.168.1.20:45876
~~~

Allow the receiver's TCP port from the host if using a firewall. Wait for authentication, release held keys/buttons and press F12. F12 returns, and F11 provides emergency recovery.

The shorter run IP:PORT command preserves the original defaults of Windows host and Ubuntu receiver. Explicit roles support same-OS pairs and reversed mixed pairs. Roles stay fixed during a run while the input destination changes.

Approve keyboard/pointer access in each Ubuntu portal dialog. Consent stays open during reconnects and may be requested again after restart or compositor revocation.

## Ubuntu Wayland host

The Ubuntu host reads only the selected event devices and reinjects local input through its consented EIS session. Select a relative USB/Bluetooth mouse and a keyboard with F11/F12. Absolute touchpads and tablets are outside this capture backend.

Add stable event paths to the host's settings.toml.

~~~toml
[capture]
devices = [
    "/dev/input/by-id/your-keyboard-event-kbd",
    "/dev/input/by-id/your-mouse-event-mouse",
]
~~~

Your desktop user needs read access to these selected devices. Grant access during setup using your distribution's ACL or udev mechanism. Desk Switch checks access and device types before grabbing input. Start with all keys/buttons released. It does not grant permissions.

While this host runs, the chosen devices feed local EIS or the authenticated receiver. Portal failure, device removal or process exit releases kernel grabs. F11 and an expired peer lease return routing to local EIS. Events queued during portal consent are discarded before capture starts.

The InputCapture portal currently permits activation only through compositor triggers, so it cannot implement immediate F12 takeover by itself. [InputCapture lifecycle](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.InputCapture.html)

## Optional screen edge

Add an edge section on both PCs. Use the same mode and select each screen/side separately. For a host to the left of the receiver, choose right on the host and left on the receiver.

~~~toml
[edge]
mode = "input"           # off, input, or video
screen = '\\.\DISPLAY2'   # Windows Screen name from probe
side = "right"
~~~

On Ubuntu, portal-check --consent opens a temporary input session and lists EIS screen IDs such as 0,0,1920,1080. It requests consent and sends no input. Use the exact ID in the screen setting. The portal-check command without --consent only queries capabilities. Bidirectional edges require an EIS absolute pointer region. Recheck it after layout/scaling changes.

Only the chosen edge transfers between PCs. Other edges allow normal local screen movement. Holding a key or dragging prevents edge entry. A transfer places the cursor just inside the destination edge at the matching proportional position. The receiver's return edge reverses an edge-originated transfer.

Input mode makes no DDC calls and works without a monitor section. F12 during input-only use upgrades to a full transfer if video is configured. F12-originated transfers return using F12/F11.

On an Ubuntu host with edge mode enabled, the software controls absolute pointer placement across EIS regions. Startup centres the local cursor on the selected host screen.

## Optional sound from the other PC

Either OS can send or receive audio independently of the keyboard host role. Keep headphones on the audio receiving PC. Its local sound mixes with the remote stream through shared-mode WASAPI on Windows or normal PipeWire playback on Ubuntu. Audio starts off.

On the PC whose system sound you want to hear, list audio sources, then start the sender.

~~~sh
./desk-switch audio sources
./desk-switch audio serve 192.168.1.30:45877
# Optional explicit source
./desk-switch audio serve 192.168.1.30:45877 --source 'PLAYBACK_SOURCE'
~~~

On the headphone PC, list audio outputs and connect to the sender.

~~~sh
./desk-switch audio outputs
./desk-switch audio play 192.168.1.30:45877 --volume 50
# Optional explicit output
./desk-switch audio play 192.168.1.30:45877 --output 'PLAYBACK_OUTPUT' --volume 50
~~~

Use .\desk-switch.exe on Windows. Ubuntu IDs are PipeWire sink names. Windows IDs are active playback endpoint IDs. The default is the local default playback output. Sources always refer to a playback device's system mix.

Use a different TCP port from input/video and allow it from the paired audio receiver if using a firewall. Capture starts only after that receiver explicitly requests audio. The receiving audio terminal accepts on, off, volume 0 through volume 100, mute, unmute, status and quit. These adjust only remote audio. Volume/mute last for that process. Pass --mute to start muted. Ctrl+C exits either audio instance.

The stream is one-way per session, 48 kHz stereo PCM, about 1.54 Mbit/s before framing. Separate authenticated connections, bounded credits and short playout buffers isolate audio from input. Stale/broken streams are cleared before reconnecting. Avoid routing a received stream back into another sender's system mix.

Ubuntu captures a selected sink monitor with device fallback disabled. Windows uses shared WASAPI loopback on a render endpoint. Choose a playback sink that stays present during video switching. HDMI audio can disappear when the monitor changes input, so use a persistent output or virtual sink in that case. Local sound on the sending output may also play. [WirePlumber policy](https://pipewire.pages.freedesktop.org/wireplumber/policies/linking.html), [WASAPI loopback](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording)

## Compatibility and validation

- Full switching needs readable/writable VCP 0x60 from each PC while its input is active. The source writes and the destination reads the result before forwarding begins. Firmware reporting stale/incorrect values can prevent reliable handoff.
- Failed handoffs, link loss, queue overflow or expired leases restore host input. The receiver releases its keys/buttons. Video recovery is best effort when the active PC or DDC adapter becomes unavailable.
- Windows uses the normal interactive desktop. Validate elevated apps, secure desktops and exclusive device capture separately. Physical key codes use the receiving keyboard layout. F11/F12 are reserved on the host.

~~~sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
~~~

Tests use synthetic devices, PCM and loopback TCP. On each real pair validate full switching with the second monitor visible, F11/link loss with held keys/buttons, edges/scaling, and simultaneous local/remote audio on the selected headphones. Ubuntu host device access and portal consent also need validation on the target PC.

[ddcutil troubleshooting](https://www.ddcutil.com/faq/), [setvcp](https://www.ddcutil.com/command_setvcp/), [Win32 SetVCPFeature](https://learn.microsoft.com/en-us/windows/win32/api/lowlevelmonitorconfigurationapi/nf-lowlevelmonitorconfigurationapi-setvcpfeature), [RemoteDesktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html).

MIT licensed.
