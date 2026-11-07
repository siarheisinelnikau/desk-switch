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

struct PreviewApp {
    desktop: Desktop,
    frames: std::sync::Arc<std::sync::atomic::AtomicU64>,
    causes: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, u64>>>,
    capture: Option<Capture>,
}

impl eframe::App for PreviewApp {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        eframe::App::logic(&mut self.desktop, ctx, frame);
        if let Some(capture) = &mut self.capture {
            capture.tick(ctx, &mut self.desktop);
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        {
            let mut causes = self.causes.lock().unwrap();
            for cause in ui.ctx().repaint_causes() {
                *causes.entry(cause.to_string()).or_default() += 1;
            }
        }
        self.frames
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        eframe::App::ui(&mut self.desktop, ui, frame);
    }
}

fn render_probe(
    ctx: egui::Context,
    frames: std::sync::Arc<std::sync::atomic::AtomicU64>,
    backend: String,
    causes: std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, u64>>>,
    output: PathBuf,
    send: std::sync::mpsc::Sender<Action>,
) -> Result<()> {
    anyhow::ensure!(
        output.is_absolute(),
        "render probe needs an absolute output path"
    );
    std::thread::Builder::new()
        .name("Desk Switch bounded render probe".into())
        .spawn(move || {
            use std::{
                sync::atomic::Ordering,
                time::{Duration, Instant},
            };
            let mut samples = Vec::new();
            for state in ["visible-idle", "hidden", "restored-idle"] {
                if state != "visible-idle" {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(state != "hidden"));
                }
                // Each visibility transition may paint. Count the settled interval separately.
                std::thread::sleep(Duration::from_secs(2));
                let start = frames.load(Ordering::Relaxed);
                let popup_start = super::popup::FRAMES.load(Ordering::Relaxed);
                let since = Instant::now();
                std::thread::sleep(Duration::from_secs(6));
                samples.push(serde_json::json!({
                    "state": state, "seconds": since.elapsed().as_secs_f64(),
                    "ui_frames": frames.load(Ordering::Relaxed) - start,
                    "popup_ui_frames": super::popup::FRAMES.load(Ordering::Relaxed) - popup_start,
                }));
            }
            let result = serde_json::to_string_pretty(&serde_json::json!({
                "backend": backend, "fixture": true, "samples": samples,
                "total_ui_frames": frames.load(Ordering::Relaxed),
                "repaint_causes": *causes.lock().unwrap(),
            }))
            .map_err(anyhow::Error::from)
            .and_then(|data| crate::config::write_atomic(&output, &data));
            if let Err(error) = result {
                eprintln!("Render probe failed: {error:#}");
            }
            let _ = send.send(Action::Quit);
            ctx.request_repaint();
        })?;
    Ok(())
}

pub fn run(case: &str, dir: PathBuf) -> Result<()> {
    anyhow::ensure!(
        dir.is_absolute(),
        "preview needs an isolated absolute directory"
    );
    std::fs::create_dir_all(&dir)?;
    let _lock = crate::config::lock(&dir, "desktop.lock")?;
    let capture_path = std::env::var_os("DESK_SWITCH_CAPTURE_TO").map(PathBuf::from);
    if let Some(path) = &capture_path {
        anyhow::ensure!(path.is_absolute(), "capture output must be absolute");
    }
    let (capture_send, capture_result) = std::sync::mpsc::sync_channel(1);
    let capturing = capture_path.is_some();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Desk Switch")
            .with_inner_size(if case == "quick-controls" {
                [344.0, 442.0]
            } else if capturing || std::env::var_os("EFRAME_SCREENSHOT_TO").is_some() {
                [1080.0, 1000.0]
            } else {
                [1080.0, 760.0]
            })
            .with_min_inner_size(if case == "quick-controls" {
                [344.0, 442.0]
            } else {
                [880.0, 650.0]
            })
            .with_icon(egui::IconData {
                rgba: super::tray::icon_rgba(64),
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    let case = case.to_owned();
    eframe::run_native(
        "Desk Switch",
        options,
        Box::new(move |cc| {
            use eframe::glow::HasContext;
            let backend = cc
                .gl
                .as_ref()
                .map(|gl| unsafe {
                    format!(
                        "Glow / OpenGL: {} / {}",
                        gl.get_parameter_string(eframe::glow::RENDERER),
                        gl.get_parameter_string(eframe::glow::VERSION)
                    )
                })
                .unwrap_or_else(|| "Unknown renderer".into());
            eprintln!("Desk Switch preview renderer: {backend}");
            let mut app = Desktop::new(cc.egui_ctx.clone(), dir, false);
            app.fixture(&case);
            if let Ok(language) = std::env::var("DESK_SWITCH_PREVIEW_LANGUAGE") {
                if !crate::i18n::LANGUAGES
                    .iter()
                    .any(|(code, _)| *code == language)
                {
                    return Err("Unknown preview language".into());
                }
                app.prefs.language = language;
                crate::i18n::set_language(&app.prefs.language);
            }
            let frames = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
            let causes =
                std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
            if let Some(output) = std::env::var_os("DESK_SWITCH_RENDER_PROBE") {
                render_probe(
                    cc.egui_ctx.clone(),
                    frames.clone(),
                    backend,
                    causes.clone(),
                    output.into(),
                    app.quick.sender(),
                )?;
            }
            Ok(Box::new(PreviewApp {
                desktop: app,
                frames,
                causes,
                capture: capture_path.map(|output| Capture {
                    output,
                    since: std::time::Instant::now(),
                    frames: 0,
                    requested: false,
                    result: Some(capture_send),
                }),
            }))
        }),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if capturing {
        capture_result
            .recv_timeout(std::time::Duration::from_secs(1))?
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

impl Desktop {
    fn fixture(&mut self, case: &str) {
        #[cfg(windows)]
        if let Some(tray) = &self.tray {
            tray.preview_label();
        }
        self.preview_popup = case == "quick-controls";
        let case = if self.preview_popup {
            "shared-audio"
        } else {
            case
        };
        self.prefs.auto_update = false;
        self.prefs.autostart = false;
        self.startup_pending = false;
        let own_green = case.starts_with("shared-") || case == "detected";
        self.public_key = if own_green { "22" } else { "11" }.repeat(32);
        let mut local = Node {
            key: self.public_key.clone(),
            name: "Studio PC".into(),
            version: Some(crate::compatibility::Version::current()),
            management: Some(self.local_management()),
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
            self.preview_scroll = 550.0;
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
        if case == "shared-audio" {
            self.preview_scroll = 550.0;
            local
                .shared
                .audio_levels
                .get_mut(&local.key)
                .unwrap()
                .volume = 90;
        }
        if case == "shared-identities-differ" {
            remote.monitor.fingerprint = "preview-hdmi-edid".into();
            local.monitor.fingerprint = "preview-displayport-edid".into();
        }
        if [
            "shared-managed-settings",
            "shared-managed-updates",
            "shared-cable",
        ]
        .contains(&case)
        {
            self.page = Page::Application;
            local.diagnostics.platform = if cfg!(windows) { "windows" } else { "linux" }.into();
            remote.diagnostics.platform = "windows".into();
            let mut managed = self.local_management();
            managed.application.autostart = true;
            managed.update.version = Some("0.5.0".into());
            managed.update.channel_version = Some("0.5.0".into());
            managed.update.ready = true;
            managed.update.status = "Version 0.5.0 is verified and ready to install.".into();
            remote.management = Some(managed);
            self.update_status = "Last checked: no newer release was available.".into();
            self.settings_remote = case == "shared-managed-settings";
            if case == "shared-cable" {
                self.cable_open = true;
            }
        }
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
                    control_protocol: Some(crate::workspace::management::PROTOCOL),
                };
                self.discovered.insert(peer.id.clone(), peer);
            }
        }
        if case == "shared-devices" {
            self.preview_scroll = 990.0;
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
            self.close_window(ctx);
        }
        if self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Let fonts/layout settle for bounded capture, then sleep until input.
        if ctx.cumulative_pass_nr() < 2 {
            ctx.request_repaint();
        }
    }
}

struct Capture {
    output: PathBuf,
    since: std::time::Instant,
    frames: u32,
    requested: bool,
    result: Option<std::sync::mpsc::SyncSender<Result<(), String>>>,
}
impl Capture {
    fn tick(&mut self, ctx: &egui::Context, desktop: &mut Desktop) {
        if self.result.is_none() {
            return;
        }
        self.frames += 1;
        let screenshot = ctx.input(|input| {
            input.raw.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let outcome = if let Some(image) = screenshot {
            let temporary = self.output.with_extension("capture.png");
            Some(
                image::save_buffer(
                    &temporary,
                    image.as_raw(),
                    image.width() as u32,
                    image.height() as u32,
                    image::ColorType::Rgba8,
                )
                .map_err(anyhow::Error::from)
                .and_then(|()| std::fs::rename(temporary, &self.output).map_err(Into::into))
                .map_err(|error| error.to_string()),
            )
        } else if self.since.elapsed() > std::time::Duration::from_secs(10) || self.frames > 120 {
            Some(Err(
                "Native screenshot did not finish within its deadline".into()
            ))
        } else {
            None
        };
        if let Some(outcome) = outcome {
            let _ = self.result.take().unwrap().send(outcome);
            desktop.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.frames >= 4
            && self.since.elapsed() >= std::time::Duration::from_millis(250)
            && !self.requested
        {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }
}
