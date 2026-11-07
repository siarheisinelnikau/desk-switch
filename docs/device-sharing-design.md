# Device selection and microphone sharing

The default remains automatic: make both computers available, Link, and Accept.
Advanced device choices belong to the computer that owns the hardware and are
synchronized with the pair. Either computer can change those choices.

## Device choices

- Playback: Auto follows the system output. An explicit output routes received
  audio there; existing local applications keep their OS output selection. The
  UI explains that local applications must use that output to hear both PCs.
- Microphone: Auto selects the default physical capture endpoint when enabled.
  Once capture starts it stays on that endpoint. A missing selected endpoint
  pauses sharing instead of falling back to a webcam or another microphone.
- Linux input: identify individual event interfaces by a stable by-id/by-path
  link. Apply keyboard and mouse filters separately, including composite HID
  interfaces. Release the previous capture before changing device bindings.
- Windows input: the existing low-level hooks share the keyboard/mouse group.
  They cannot reliably suppress just one physical device. Do not offer a
  misleading per-device picker or install a kernel interception driver.
- Do not change an input source or its binding while the receiving computer
  depends on that shared input. Return control before reconfiguring it.

## Microphone

Microphone sharing is off by default and independent of system audio, monitor
and input ownership. Choose one source PC, Auto or a specific microphone, and
adjust the forwarded microphone level/mute from either PC. Local applications
on the source can continue using its physical microphone normally.

Use the existing bounded PCM transport, with a separate authenticated Noise
purpose and TCP port. An audio playback connection must not authenticate as a
microphone connection. Open capture only after the paired receiver requests it.
Do not listen to the microphone on speakers, change the OS default input, or
capture Desk Switch's virtual input again. Applications select the virtual
input once in their own settings.

On Linux, a child-owned PipeWire loopback exposes **Desk Switch microphone**.
Both its private feed and virtual source disappear when sharing stops. No
system configuration files or default devices are changed.

On Windows, receive through an already installed virtual cable (for example
VB-CABLE). Render only to the explicitly identified cable playback endpoint;
applications select its paired recording endpoint. Never send microphone data
to an ordinary speaker. Offer the official setup link when the bridge is
missing; do not redistribute or silently install a third-party driver.

## Manual addresses

Discovery remains available. A manual IP (optionally with control port) uses the
same Noise identity exchange and recipient approval as discovery. An existing
pin must match. Save the authenticated advertised control address for reconnect.
Different subnets work when routing/firewalls permit all service ports; this
does not add NAT traversal. Manual connection must work when multicast fails.

## Validation

Use synthetic device inventories, loopback network peers and private desktop /
PipeWire sessions. Never switch a physical monitor or capture the user's input,
playback or microphone as part of automated validation.

Primary references:

- [PipeWire loopback](https://docs.pipewire.org/page_module_loopback.html)
- [pw-loopback](https://docs.pipewire.org/page_man_pw-loopback_1.html)
- [Windows low-level keyboard hook fields](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-kbdllhookstruct)
- [Windows Raw Input](https://learn.microsoft.com/en-us/windows/win32/inputdev/about-raw-input)
- [VB-CABLE routing and setup](https://vb-audio.com/Cable/)
