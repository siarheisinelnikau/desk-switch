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
            monitor: Default::default(),
            shared: Shared::default(),
            input: Some(InputStatus::Here),
            monitor_input: None,
            audio_playing: true,
            audio_error: None,
            input_error: None,
            ack: String::new(),
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
                node.monitor.enabled = true;
                node.monitor.fingerprint = "preview-shared-monitor".into();
                node.monitor.input = Some(input);
                node.monitor.name = "Shared display".into();
            }
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
        if ["settings", "light"].contains(&case) {
            self.page = Page::Application;
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
        let snapshot = (self.pair.synced, false, false);
        if self.tray_snapshot != snapshot {
            if let Some(tray) = &mut self.tray {
                tray.update(snapshot.0, false, false);
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
