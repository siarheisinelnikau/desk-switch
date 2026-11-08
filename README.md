# Desk Switch

**Your computers. One workspace.** Share a keyboard, mouse, headphones or speakers, microphone and clipboard across two, three, four or more computers. Manage the workspace and every computer's settings from whichever one you are using.

![Desk Switch workspace with four connected computers](docs/workspace-four-computers.png)

## Get started

Download the same version for every computer from [Releases](https://github.com/siarheisinelnikau/desk-switch/releases/latest). Current releases provide **Windows x86-64** and **Ubuntu 24.04/26.04 x86-64** installers.

Run the Windows executable or install the Ubuntu `.deb` with APT. Open **+ Add a computer**, find a nearby computer or enter its **Manual IP**, and approve the connection on that computer. Repeat for the rest of your workspace.

Choose where your keyboard, mouse and sound come from. Click **Switch here** to control a computer, press **F12** to cycle, or **F11** to return input to its source. Keep the coordinating computer running; update it last.

## What it does

- Share one keyboard and mouse across connected computers.
- Arrange screens around your desk and move between computers at touching edges.
- Hear every computer through one set of headphones or speakers, with independent volume and mute. The listening computer controls system volume; other levels are relative to it.
- Share a physical microphone through a virtual microphone on receiving computers.
- Optionally synchronize clipboard **text and images**.
- Switch one shared monitor between its configured HDMI/DisplayPort inputs.
- Change devices, preferences and updates for any connected computer from one place.

## Quick controls

Left-click the tray icon to switch computers, toggle sharing and adjust volume. Right-click opens the native menu. Closing the main window keeps Desk Switch in the tray; **Quit application** exits.

![Desk Switch tray popup with sharing controls and per-computer volume](docs/workspace-quick-controls.png)

On Ubuntu GNOME, enable the bundled **Desk Switch Tray** companion in **Extensions** for the left-click popup and clipboard sharing.

<details>
<summary>Settings</summary>

![Settings for a selected workspace computer](docs/workspace-settings.png)

![A four-computer display arrangement with one shared monitor and three ordinary displays](docs/display-arrangement.png)

</details>

See the [guide](docs/guide.md) for setup, permissions, virtual microphones, monitor compatibility and development. [VB-CABLE](https://vb-audio.com/Cable/) is donationware by VB-Audio Software.

## Inspiration

A good friend shared this idea and showed me [Hotseat](https://github.com/muspelheim/hotseat), his project for switching monitor inputs on macOS. I built Desk Switch for my Ubuntu and Windows computers, extending the idea to keyboards, mice, speakers, headphones and microphones. Thank you, my friend, for the idea and inspiration.

macOS support remains experimental, has not been tested on physical hardware and is excluded from current releases. **macOS testing and pull requests are warmly appreciated.**
