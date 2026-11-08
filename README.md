# Desk Switch

**Your computers. One workspace.** Share input, sound, microphones and clipboard across two, three, four or more computers. Manage every member from one place.

![Desk Switch workspace with four connected computers](docs/workspace-four-computers.png)

Download matching versions from [Releases](https://github.com/siarheisinelnikau/desk-switch/releases/latest) for **Windows x86-64**, **Ubuntu 24.04/26.04 x86-64**, or **macOS 13+ Apple Silicon/Intel**. Link and approve computers with **+ Add a computer**. Select a card to switch, **F12** to cycle, or **F11** to return input.

## What it does

- Keyboard/mouse sharing, arranged screen edges, crossing delay, protected corners and pause.
- All computers' sound on one output, with system and relative volume.
- One physical microphone delivered to virtual microphones.
- Shared text/images; file and folder copies or moves start when pasted, with progress, pause and cancel.
- Direct file sends with cancel and resume.
- Shared HDMI/DisplayPort monitor switching.
- Device choices, preferences and updates from any member.

A connected majority can replace an offline coordinator. Two computers need both online for shared changes. See the [setup, upgrades and limits](docs/guide.md).

## Quick controls

Left-click the tray icon for quick controls; right-click for the native menu. **X** hides the window in the tray; **Quit application** exits.

![Desk Switch tray popup with sharing controls and per-computer volume](docs/workspace-quick-controls.png)

On Ubuntu GNOME, enable **Desk Switch Tray** in **Extensions**.

<details>
<summary>Settings</summary>

![Settings for a selected workspace computer](docs/workspace-settings.png)

</details>

## Inspiration

A good friend shared this idea and showed me [Hotseat](https://github.com/muspelheim/hotseat), his project for switching monitor inputs on macOS. I built Desk Switch for my Ubuntu and Windows computers, extending the idea to keyboards, mice, speakers, headphones and microphones. Thank you, my friend, for the idea and inspiration.

macOS support is experimental and **has not been tested on physical hardware**. Native file paste into Finder is unavailable; use **Send files…** to receive files on a Mac. Bundles are ad-hoc signed, not notarized. **macOS testing and pull requests are warmly appreciated.**
