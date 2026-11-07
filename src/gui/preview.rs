//! Developer-only native screenshots. No discovery, device probe or child service.
//! This module and CLI entry point are absent from release builds.
use super::{Action, Desktop, Page};
use crate::{
    lifecycle::InputStatus,
    workspace::{Hardware, Node, Plan, Shared, net::Request},
};
use anyhow::Result;
use eframe::egui;
use std::path::PathBuf;

pub fn run(case: &str, dir: PathBuf) -> Result<()> {
    anyhow::ensure!(
        dir.is_absolute(),
        "preview needs an isolated absolute directory"
    );
    std::fs::create_dir_all(&dir)?;
    let _lock = crate::config::lock(&dir, "desktop.lock")?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Desk Switch")
            .with_inner_size([1080.0, 760.0])
            .with_min_inner_size([880.0, 650.0]),
        ..Default::default()
    };
    let case = case.to_owned();
    eframe::run_native(
        "Desk Switch",
        options,
        Box::new(move |cc| {
            let mut app = Desktop::new(cc.egui_ctx.clone(), dir, false);
            app.fixture(&case);
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}

impl Desktop {
    fn fixture(&mut self, case: &str) {
        self.prefs.auto_update = false;
        self.prefs.autostart = false;
        self.startup_pending = false;
        let own_green = case.starts_with("shared-") || case == "detected";
        self.public_key = if own_green { "22" } else { "11" }.repeat(32);
        let mut local = Node {
            key: self.public_key.clone(),
            name: "Studio PC".into(),
            hardware: Hardware {
                known: true,
                audio_score: 40,
                audio_name: "USB Headphones".into(),
                ..Default::default()
            },
            devices: Default::default(),
            monitor: Default::default(),
            shared: Shared::default(),
            input: Some(InputStatus::Here),
            monitor_input: None,
            monitor_error: None,
            audio_playing: true,
            microphone_active: false,
            microphone_error: None,
            audio_error: None,
            input_error: None,
            ack: String::new(),
            diagnostics: Default::default(),
        };
        let mut remote = local.clone();
        remote.key = if own_green { "11" } else { "22" }.repeat(32);
        remote.name = "Laptop".into();
        remote.hardware.keyboard = true;
        remote.hardware.mouse = true;
        remote.hardware.audio_score = 20;
        remote.hardware.audio_name = "Speakers".into();
        if case == "linked" {
            local.input = Some(InputStatus::Peer);
            remote.input = Some(InputStatus::Here);
        } else {
            local.input = Some(InputStatus::Here);
            remote.input = Some(InputStatus::Peer);
        }
        if own_green {
            local.hardware.keyboard = true;
            local.hardware.mouse = true;
            remote.hardware.keyboard = false;
            remote.hardware.mouse = false;
            local.input = Some(InputStatus::Here);
            remote.input = Some(InputStatus::Peer);
        }
        if case == "detected" {
            local.input = None;
            remote.input = None;
        }
        if case.starts_with("shared-") {
            for (node, input) in [(&mut local, 17), (&mut remote, 15)] {
                node.shared.monitor_enabled = true;
                node.monitor.enabled = true;
                node.monitor.id = format!("fixture-{}", node.name);
                node.monitor.fingerprint = "preview-shared-monitor".into();
                node.monitor.input = Some(input);
                node.monitor.name = "Shared display".into();
            }
            self.monitors.push(crate::platform::MonitorInfo {
                id: local.monitor.id.clone(),
                name: "Shared display".into(),
                screen: "auto".into(),
                current: Some(17),
                inputs: vec![17, 15],
                error: None,
                fingerprint: local.monitor.fingerprint.clone(),
            });
            let input = match case {
                "shared-pending" => None,
                "shared-peer" => Some(15),
                _ => Some(17),
            };
            local.monitor_input = input;
            remote.monitor_input = input;
            if ["shared-peer", "shared-input-only"].contains(&case) {
                local.input = Some(InputStatus::Peer);
                remote.input = Some(InputStatus::Here);
            }
            self.prefs.workspace.monitor = local.monitor.clone();
        }
        if case == "shared-independent" {
            remote.hardware.keyboard = true;
            remote.hardware.mouse = true;
            local.input = None;
            remote.input = None;
        }
        if case == "shared-setup" || case == "shared-borrowed-setup" {
            local.monitor.enabled = false;
            local.monitor.input = None;
            self.prefs.workspace.monitor = local.monitor.clone();
        }
        if case == "shared-borrowed-setup" {
            local.hardware.keyboard = false;
            local.hardware.mouse = false;
            remote.hardware.keyboard = true;
            remote.hardware.mouse = true;
            local.input = Some(InputStatus::Here);
            remote.input = Some(InputStatus::Peer);
        }
        if case == "shared-denied" {
            local.input = None;
            remote.input = None;
            local.input_error = Some("Permission denied (os error 13)".into());
            self.pair.input_failed = true;
        }
        if case == "shared-separate" {
            local.shared.monitor_enabled = false;
            remote.shared.monitor_enabled = false;
        }
        if case == "shared-unconfigured" {
            remote.monitor.input = None;
        }
        if case == "shared-diagnostics" {
            remote.diagnostics.platform = "windows".into();
            remote
                .diagnostics
                .line("Monitor", "This PC remembered its DisplayPort input (0x0F)");
            remote
                .diagnostics
                .line("Monitor", "Target input confirmed; video switch completed");
            remote
                .diagnostics
                .line("Audio", "Playing remote audio at 48 kHz stereo");
            self.page = Page::Diagnostics;
        }
        if case == "shared-update-confirmation" {
            self.page = Page::Application;
            self.update_confirmation = true;
        }
        for (node, individual) in [(&mut local, true), (&mut remote, false)] {
            use crate::audio::AudioDevice;
            node.devices.individual_input = individual;
            if node.hardware.keyboard {
                node.devices.keyboards.push(AudioDevice {
                    id: "usb-keyboard".into(),
                    name: "USB Keyboard".into(),
                    default: true,
                });
            }
            if node.hardware.mouse {
                node.devices.mice.push(AudioDevice {
                    id: "usb-mouse".into(),
                    name: "USB Mouse".into(),
                    default: true,
                });
            }
            node.devices.speakers.push(AudioDevice {
                id: "usb-headset".into(),
                name: node.hardware.audio_name.clone(),
                default: true,
            });
            node.devices.speakers.push(AudioDevice {
                id: "hdmi-output".into(),
                name: "HDMI Speakers".into(),
                default: false,
            });
            node.devices.microphones.push(AudioDevice {
                id: "headset-mic".into(),
                name: "Headset Microphone".into(),
                default: true,
            });
            node.devices.microphones.push(AudioDevice {
                id: "webcam-mic".into(),
                name: "Webcam Microphone".into(),
                default: false,
            });
            node.devices.microphone_bridges.push(AudioDevice {
                id: "virtual-input".into(),
                name: if individual {
                    "Desk Switch microphone"
                } else {
                    "CABLE Input (VB-Audio Virtual Cable)"
                }
                .into(),
                default: true,
            });
            node.hardware.microphone_receive = true;
        }
        if case == "shared-microphone" {
            local.hardware.microphone_score = 80;
            local.hardware.microphone_name = "Headset Microphone".into();
            local.shared.microphone_enabled = true;
            local.shared.microphone_level.volume = 65;
            local.shared.microphone_binding = Some(crate::workspace::devices::Binding {
                computer: local.key.clone(),
                device: "headset-mic".into(),
            });
            local.microphone_active = true;
            remote.microphone_active = true;
        }
        if case == "shared-input-access" {
            local.input_error =
                Some("Permission denied reading the selected keyboard and mouse".into());
            self.pair.input_failed = true;
            self.prefs.role = "host".into();
            self.form.capture_devices =
                "/dev/input/fixture-keyboard\n/dev/input/fixture-mouse".into();
        }
        if case == "linking" {
            self.pair.busy = true;
        }
        let output = Plan::build(&local, &remote).audio_output;
        local
            .shared
            .prepare_audio_levels(&local.key, &remote.key, output.as_deref());
        remote.shared = local.shared.clone();
        self.prefs.workspace.shared = local.shared.clone();
        self.pair.hardware = local.hardware.clone();
        self.prefs.discovery = !["offline", "settings", "light"].contains(&case);
        if ["linked", "away", "syncing", "detected"].contains(&case) || case.starts_with("shared-")
        {
            let plan = Plan::build(&local, &remote);
            let digest = plan.digest(&local, &remote);
            local.ack = digest.clone();
            remote.ack = digest;
            self.peer_key = remote.key.clone();
            self.pair.remote = Some(remote);
            self.pair.plan = Some(plan);
            self.pair.synced = case != "syncing";
        } else if case == "request" {
            self.pair.request = Some(Request {
                key: remote.key.clone(),
                name: remote.name.clone(),
                code: "123 456".into(),
            });
        } else if case == "available" {
            for (name, version, protocol, key) in [
                (
                    "Laptop",
                    crate::compatibility::VERSION,
                    crate::compatibility::PROTOCOL,
                    "22",
                ),
                ("Office PC", "0.2.1", 2, "33"),
            ] {
                let peer = crate::discovery::Peer {
                    id: name.into(),
                    name: name.into(),
                    version: version.into(),
                    protocol,
                    addresses: vec!["192.0.2.5:45876".parse().unwrap()],
                    public_key: Some(key.repeat(32)),
                    control_port: Some(crate::workspace::PORT),
                };
                self.discovered.insert(peer.id.clone(), peer);
            }
        }
        if ["settings", "light", "shared-devices"].contains(&case) {
            self.page = Page::Application;
        }
        for (keyboard, devices) in [
            (true, &local.devices.keyboards),
            (false, &local.devices.mice),
        ] {
            for device in devices {
                self.pair
                    .inputs
                    .push(crate::workspace::hardware::InputEndpoint {
                        info: device.clone(),
                        path: std::path::PathBuf::from(if keyboard {
                            "/dev/input/fixture-keyboard"
                        } else {
                            "/dev/input/fixture-mouse"
                        }),
                        #[cfg(target_os = "linux")]
                        generation: None,
                        keyboard,
                        mouse: !keyboard,
                    });
            }
        }
        self.preview = Some(local);
    }
    pub(super) fn preview_tick(&mut self, ctx: &egui::Context) {
        for _ in 0..32 {
            let Ok(action) = self.actions.try_recv() else {
                break;
            };
            self.action(action, ctx);
        }
        if let Some(local) = &mut self.preview {
            local.shared = self.prefs.workspace.shared.clone();
            if let Some(remote) = &mut self.pair.remote {
                remote.shared = local.shared.clone();
                self.pair.plan = Some(Plan::build(local, remote));
            }
        }
        let snapshot = self.tray_state();
        if self.tray_snapshot != snapshot {
            if let Some(tray) = &mut self.tray {
                tray.update(snapshot);
            }
            self.tray_snapshot = snapshot;
        }
        if ctx.input(|input| input.viewport().close_requested()) && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.action(Action::Quit, ctx);
        }
        if self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}
