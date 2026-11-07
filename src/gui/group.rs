//! Consumer workspace for any number of linked computers.
use super::{Desktop, Page, QueryKind, group_data::Data};
use crate::{
    config,
    group::{
        model::{self, Member, Workspace},
        net::{Action, Event, Service},
    },
    workspace::{self, Node, Selection, net::Request},
};
use anyhow::{Context, Result};
use eframe::egui::{self, Color32, RichText};
use std::{collections::BTreeMap, net::SocketAddr};

fn tr(text: &str) -> String {
    crate::i18n::tr(text)
}

#[derive(Default)]
pub(super) struct Group {
    pub compat: super::group_compat::Compat,
    pub service: Option<Service>,
    pub workspace: Option<Workspace>,
    pub selected: String,
    pub request: Option<Request>,
    pub data: Data,
    probe: Option<workspace::hardware::Probe>,
    last_node: Option<Node>,
    last_volume: Option<workspace::AudioLevel>,
    last_ready: u64,
    input_ready: u64,
    monitor_wait: Option<(u64, std::time::Instant)>,
    monitor_activation: u64,
    monitor_error: Option<String>,
    monitor_failed: Option<u64>,
    pub reply_target: Option<String>,
    pub cable_target: Option<String>,
    pub update_target: Option<String>,
    monitor_seen: Option<Option<String>>,
    monitor_job: Option<super::worker::Worker>,
    pub preview: bool,
    manual: String,
    pub clipboard: Option<crate::group::clipboard::Clipboard>,
}

impl Desktop {
    pub(super) fn management_send_to(
        &self,
        target: &str,
        message: workspace::management::Message,
    ) -> Result<()> {
        if self.legacy_management_target(target) {
            return self.send_legacy_management(target, message);
        }
        if let Some(service) = &self.group.service {
            service.action(Action::Manage {
                target: target.into(),
                message,
            })
        } else {
            self.pair
                .service
                .as_ref()
                .context("The computer is disconnected")?
                .manage(message)
        }
    }
    pub(super) fn management_reply(&self, message: workspace::management::Message) {
        if let Some(target) = self
            .group
            .reply_target
            .as_ref()
            .or(self.group.cable_target.as_ref())
        {
            let _ = self.management_send_to(target, message);
        } else if let Some(service) = &self.pair.service {
            let _ = service.manage(message);
        }
    }
    pub(super) fn group_action(&mut self, action: Action) {
        if self.group.preview {
            if let Some(state) = &mut self.group.workspace {
                match action {
                    Action::Settings { expected, value } => {
                        let _ = state.set_settings(expected, value);
                        state.bind_microphone();
                    }
                    Action::Member {
                        key,
                        expected,
                        value,
                    } => {
                        let _ = state.set_member(&key, &expected, value);
                    }
                    Action::Activate { key } => state.active_input = Some(key),
                    _ => {}
                }
            }
            self.ctx.request_repaint_of(egui::ViewportId::ROOT);
            return;
        }
        if let Action::Manage { target, message } = &action {
            if let Err(error) = self.management_send_to(target, message.clone()) {
                self.notice = Some((error.to_string(), true));
            }
            return;
        }
        if let Some(service) = &self.group.service
            && let Err(error) = service.action(action)
        {
            self.notice = Some((error.to_string(), true));
        }
    }
    pub(super) fn group_cycle(&mut self) {
        let Some(state) = &self.group.workspace else {
            return;
        };
        let keys = state
            .members
            .values()
            .filter(|member| member.compatible())
            .map(|member| member.node.key.clone())
            .collect::<Vec<_>>();
        if keys.len() < 2 {
            return;
        }
        let current = state
            .active_input
            .as_ref()
            .and_then(|active| keys.iter().position(|key| key == active))
            .unwrap_or(0);
        let key = keys[(current + 1) % keys.len()].clone();
        let position = self.group.data.cycle_position.take();
        self.group_action(if let Some(position) = position {
            Action::ActivateEdge { key, position }
        } else {
            Action::Activate { key }
        });
    }
    pub(super) fn tick_group(&mut self, ctx: &egui::Context) {
        if self.group.preview {
            return;
        }
        if self.group.service.is_none() && !self.quitting && self.discovery_error.is_none() {
            let result = (|| {
                if !self.dir.join("identity").exists() {
                    config::create_identity(&self.dir)?;
                }
                self.public_key = config::hex(&config::load_identity(&self.dir)?.public);
                let local = Member {
                    node: self.local_node(),
                    address: SocketAddr::from(([127, 0, 0, 1], model::PORT)),
                    online: true,
                    native_output: None,
                    preferences: Default::default(),
                    input_ready: 0,
                };
                self.group.service = Some(Service::start(
                    self.dir.clone(),
                    local,
                    model::PORT,
                    ctx.clone(),
                )?);
                self.group.probe = Some(workspace::hardware::Probe::start(ctx.clone())?);
                self.group.clipboard =
                    Some(crate::group::clipboard::Clipboard::start(ctx.clone())?);
                self.prefs.discovery = true;
                Ok::<_, anyhow::Error>(())
            })();
            if let Err(error) = result {
                self.discovery_error = Some(format!("{error:#}"));
            }
        }
        if let Some(snapshot) = self
            .group
            .probe
            .as_ref()
            .and_then(workspace::hardware::Probe::poll)
        {
            self.pair.scanned = snapshot.hardware.known;
            self.pair.hardware = snapshot.hardware;
            self.pair.inputs = snapshot.inputs;
            self.pair.devices = snapshot.devices;
            self.monitors = snapshot.displays;
            if let Some(error) = snapshot.error {
                self.notice = Some((error, true));
            }
            if self.prefs.workspace.monitor.id.is_empty() && self.monitors.len() == 1 {
                let monitor = &self.monitors[0];
                self.prefs.workspace.monitor.id = monitor.id.clone();
                self.prefs.workspace.monitor.name = monitor.name.clone();
                self.prefs.workspace.monitor.fingerprint = monitor.fingerprint.clone();
            }
        }
        if self.discovery.is_none() && self.group.service.is_some() && !self.quitting {
            match crate::discovery::Discovery::start(
                &crate::discovery::computer_name(),
                &self.public_key,
                workspace::PORT,
                ctx.clone(),
            ) {
                Ok(discovery) => self.discovery = Some(discovery),
                Err(error) => self.discovery_error = Some(error.to_string()),
            }
        }
        if let Some(discovery) = &self.discovery {
            for event in discovery.poll() {
                match event {
                    crate::discovery::Event::Found(peer) => {
                        self.discovered.insert(peer.id.clone(), peer);
                    }
                    crate::discovery::Event::Removed(id) => {
                        self.discovered.remove(&id);
                    }
                    crate::discovery::Event::Error(error) => self.discovery_error = Some(error),
                }
            }
        }
        let events = self
            .group
            .service
            .as_ref()
            .map(Service::poll)
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Request(request) => {
                    self.group.request = Some(request);
                    self.action(super::Action::Show, ctx);
                }
                Event::Error(error) => self.notice = Some((error, true)),
                Event::State(state) => {
                    self.prefs.workspace.shared.input = state.settings.input.clone();
                    self.prefs.workspace.shared.audio = state.settings.audio.clone();
                    self.prefs.workspace.shared.audio_enabled = state.settings.audio_enabled;
                    self.prefs.workspace.shared.microphone_enabled =
                        state.settings.microphone_enabled;
                    self.prefs.workspace.shared.microphone = state.settings.microphone.clone();
                    self.prefs.workspace.shared.microphone_binding =
                        state.settings.microphone_binding.clone();
                    self.prefs.workspace.shared.keyboard_enabled = state.settings.keyboard_enabled;
                    self.prefs.workspace.shared.mouse_enabled = state.settings.mouse_enabled;
                    self.prefs.workspace.shared.monitor_enabled = state.settings.monitor_enabled;
                    self.prefs.workspace.shared.devices = state
                        .members
                        .iter()
                        .map(|(key, member)| (key.clone(), member.preferences.devices.clone()))
                        .collect();
                    self.prefs.workspace.shared.audio_levels = state
                        .members
                        .iter()
                        .map(|(key, member)| {
                            (
                                key.clone(),
                                if state.plan().audio.as_ref() == Some(key) {
                                    member
                                        .native_output
                                        .or(member.preferences.output)
                                        .unwrap_or_default()
                                } else {
                                    member.preferences.audio
                                },
                            )
                        })
                        .collect();
                    self.group.workspace = Some(*state);
                }
                Event::Management { source, message } => {
                    self.group.reply_target = Some(source);
                    self.receive_management(message, ctx);
                    self.group.reply_target = None;
                }
                Event::Update { source, message } => {
                    if self
                        .group
                        .update_target
                        .as_ref()
                        .is_none_or(|target| target == &source)
                    {
                        self.group.update_target = Some(source);
                        self.receive_update(message, ctx);
                    }
                }
                Event::UpdateSent(message) => self.sent_update(&message, ctx),
                Event::Clipboard {
                    source,
                    mime,
                    bytes,
                    digest,
                } => {
                    if self.group.workspace.as_ref().is_some_and(|state| {
                        state.settings.clipboard_enabled
                            && state.members.get(&source).is_some_and(Member::compatible)
                    }) && let Some(clipboard) = &self.group.clipboard
                    {
                        clipboard.receive(mime, bytes, digest);
                    }
                }
            }
        }
        self.tick_group_compat(ctx);
        if let Some(state) = self.group.workspace.clone() {
            if self.group.selected.is_empty() {
                self.group.selected = self.public_key.clone();
            }
            let focused = self
                .group
                .update_target
                .as_ref()
                .unwrap_or(&self.group.selected);
            self.pair.remote = state
                .members
                .get(focused)
                .filter(|member| member.node.key != self.public_key)
                .map(|member| member.node.clone());
            self.pair.synced = state
                .members
                .values()
                .filter(|member| member.online)
                .count()
                > 1;
            self.peer_key = focused.clone();
            self.settings_remote = focused != &self.public_key;
            self.prefs.workspace.peer_name = self
                .pair
                .remote
                .as_ref()
                .map(|node| node.name.clone())
                .unwrap_or_default();
            if let Some(clipboard) = &self.group.clipboard {
                clipboard.enabled(state.settings.clipboard_enabled);
                for (mime, bytes, digest) in clipboard.poll() {
                    self.group_action(Action::Clipboard {
                        mime,
                        bytes,
                        digest,
                    });
                }
            }
            let specs = if self.quitting || self.install_pending {
                BTreeMap::new()
            } else {
                match super::group_data::desired(
                    &state,
                    &self.public_key,
                    &self.pair.inputs,
                    self.prefs.show_remote_cursor,
                ) {
                    Ok(specs) => specs,
                    Err(error) => {
                        self.notice = Some((error.to_string(), true));
                        BTreeMap::new()
                    }
                }
            };
            self.group.data.tick(specs, &self.dir, ctx);
            if self.group.data.cycle {
                self.group.data.cycle = false;
                self.group_cycle();
            }
            if self.group.data.returned {
                self.group.data.returned = false;
                if let Some(source) = state.plan().input {
                    self.group_action(Action::Activate { key: source });
                }
            }
            if let Some(error) = self.group.data.error.take() {
                self.notice = Some((error, true));
            }
            let monitor = state.active_monitor.clone();
            let here = state.active_input.as_deref() == Some(&self.public_key);
            let video = here
                && state.settings.monitor_enabled
                && state.members[&self.public_key].node.monitor.enabled
                && monitor.as_deref() == Some(&self.public_key);
            let changed = self
                .group
                .monitor_seen
                .as_ref()
                .is_some_and(|previous| previous != &monitor);
            let retry = self
                .group
                .monitor_failed
                .is_some_and(|failed| failed != state.activation);
            if video && (changed || retry) && self.group.monitor_job.is_none() {
                self.group.input_ready = 0;
                self.group.monitor_wait = None;
                self.group.monitor_activation = state.activation;
                self.group.monitor_error = None;
                self.group.monitor_failed = None;
                match state.monitor_input(&self.public_key) {
                    Ok(input) => {
                        let args = vec![
                            "monitor-select".into(),
                            self.prefs.workspace.monitor.id.clone(),
                            input.to_string(),
                        ];
                        match super::worker::Worker::start(
                            super::worker::Kind::Query,
                            &args,
                            &self.dir,
                            ctx,
                        ) {
                            Ok(worker) => self.group.monitor_job = Some(worker),
                            Err(error) => {
                                self.group.monitor_error = Some(error.to_string());
                                self.group.monitor_failed = Some(state.activation);
                            }
                        }
                    }
                    Err(error) => {
                        self.group.monitor_error = Some(error.to_string());
                        self.group.monitor_failed = Some(state.activation);
                    }
                }
            }
            self.group.monitor_seen = Some(monitor);
            if let Some(worker) = &mut self.group.monitor_job {
                worker.poll();
                if worker.settled() {
                    if worker.exit_ok == Some(true) {
                        self.group.monitor_wait = Some((
                            self.group.monitor_activation,
                            std::time::Instant::now() + crate::video::SETTLE,
                        ));
                    } else {
                        self.group.monitor_failed = Some(self.group.monitor_activation);
                        self.group.monitor_error =
                            Some(worker.logs.back().cloned().unwrap_or_else(|| {
                                "Monitor selection failed. Select the port manually or retry."
                                    .into()
                            }));
                    }
                    self.group.monitor_job = None;
                } else {
                    ctx.request_repaint_after(worker.next_poll());
                }
            }
            if let Some((activation, until)) = self.group.monitor_wait {
                if until <= std::time::Instant::now() {
                    self.group.input_ready = activation;
                    self.group.monitor_wait = None;
                } else {
                    ctx.request_repaint_after(
                        until.saturating_duration_since(std::time::Instant::now()),
                    );
                }
            }
            if here
                && (!video
                    || (self.group.monitor_job.is_none()
                        && self.group.monitor_wait.is_none()
                        && self.group.monitor_failed.is_none()))
            {
                self.group.input_ready = state.activation;
            }
        }
        let mut node = self.local_node();
        self.group.data.report(&mut node);
        node.monitor_error = self.group.monitor_error.clone();
        if self.group.last_node.as_ref() != Some(&node)
            || self.group.last_volume != self.group.data.volume
            || self.group.last_ready != self.group.input_ready
        {
            if let Some(service) = &self.group.service {
                service.publish(node.clone(), self.group.data.volume, self.group.input_ready);
            }
            self.group.last_node = Some(node);
            self.group.last_volume = self.group.data.volume;
            self.group.last_ready = self.group.input_ready;
        }
        if self.quitting {
            self.group.data.stop();
        }
    }

    pub(super) fn group_overview(&mut self, ui: &mut egui::Ui) {
        ui.heading(tr("Your workspace"));
        ui.label(tr(
            "Share your devices across all connected computers. Change settings from any computer.",
        ));
        ui.add_space(12.0);
        if let Some(request) = self.group.request.clone() {
            egui::Frame::group(ui.style())
                .inner_margin(16)
                .show(ui, |ui| {
                    ui.strong(format!("{} · {}", tr("Connection request"), request.name));
                    ui.label(format!("{}: {}", tr("Security code"), request.code));
                    ui.horizontal(|ui| {
                        for (text, accept) in [("Allow", true), ("Decline", false)] {
                            if ui.button(tr(text)).clicked() {
                                if let Some(service) = &self.group.service {
                                    service.approve(request.key.clone(), accept);
                                }
                                self.group.request = None;
                            }
                        }
                    });
                });
        }
        let Some(state) = self.group.workspace.clone() else {
            ui.label(tr("Starting workspace…"));
            return;
        };
        let plan = state.plan();
        let members = state.members.values().collect::<Vec<_>>();
        let columns = if ui.available_width() >= 600.0 { 2 } else { 1 };
        egui::Grid::new("workspace-computers")
            .num_columns(columns)
            .spacing([12.0, 12.0])
            .show(ui, |ui| {
                let width = (ui.available_width() - 12.0 * (columns - 1) as f32) / columns as f32;
                for (index, member) in members.iter().enumerate() {
                    let key = &member.node.key;
                    let accent = [
                        Color32::from_rgb(76, 211, 163),
                        Color32::from_rgb(175, 142, 255),
                        Color32::from_rgb(92, 185, 239),
                        Color32::from_rgb(244, 176, 103),
                    ][index % 4];
                    egui::Frame::group(ui.style())
                        .fill(ui.visuals().window_fill())
                        .corner_radius(12)
                        .inner_margin(16)
                        .show(ui, |ui| {
                            ui.set_width((width - 34.0).max(220.0));
                            ui.horizontal(|ui| {
                                ui.colored_label(
                                    accent,
                                    RichText::new(&member.node.name).size(18.0).strong(),
                                );
                                if key == &self.public_key {
                                    ui.weak(tr("This PC"));
                                }
                            });
                            let platform = match member.node.diagnostics.platform.as_str() {
                                "windows" => "Windows",
                                "linux" => "Ubuntu",
                                "macos" => "macOS",
                                _ => "Computer",
                            };
                            ui.weak(format!(
                                "{platform} · {} · {}",
                                member
                                    .node
                                    .version
                                    .as_ref()
                                    .map(|version| version.app.as_str())
                                    .unwrap_or("—"),
                                tr(if member.online {
                                    "Connected"
                                } else {
                                    "Offline"
                                })
                            ));
                            let mut roles = Vec::new();
                            if plan.input.as_ref() == Some(key) {
                                roles.push(tr("Keyboard & mouse"));
                            }
                            if plan.audio.as_ref() == Some(key) {
                                roles.push(tr("Shared output"));
                            }
                            if plan
                                .microphone
                                .as_ref()
                                .is_some_and(|binding| &binding.computer == key)
                            {
                                roles.push(tr("Microphone"));
                            }
                            ui.label(if roles.is_empty() {
                                tr("Available for sharing")
                            } else {
                                roles.join(" · ")
                            });
                            ui.horizontal(|ui| {
                                let active = state.active_input.as_ref() == Some(key);
                                if ui
                                    .add_enabled(
                                        member.compatible() && !active,
                                        egui::Button::new(tr(if active {
                                            "Input target"
                                        } else {
                                            "Switch here"
                                        })),
                                    )
                                    .clicked()
                                {
                                    self.group_action(Action::Activate { key: key.clone() });
                                }
                                if ui.button(tr("Settings")).clicked() {
                                    self.group.selected = key.clone();
                                    self.page = Page::Application;
                                }
                            });
                        });
                    if (index + 1) % columns == 0 {
                        ui.end_row();
                    }
                }
            });
        ui.add_space(16.0);
        egui::Frame::group(ui.style())
            .inner_margin(16)
            .show(ui, |ui| {
                ui.strong(tr("Shared devices"));
                let mut settings = state.settings.clone();
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut settings.keyboard_enabled, tr("Keyboard"));
                    ui.checkbox(&mut settings.mouse_enabled, tr("Mouse"));
                    ui.checkbox(&mut settings.audio_enabled, tr("Sound"));
                    ui.checkbox(&mut settings.microphone_enabled, tr("Microphone"));
                });
                ui.checkbox(&mut settings.clipboard_enabled, tr("Shared clipboard"));
                if let Some(status) = self
                    .group
                    .clipboard
                    .as_ref()
                    .and_then(|clipboard| clipboard.status())
                {
                    ui.weak(tr(&status));
                }
                ui.checkbox(
                    &mut settings.edge,
                    tr("Move between computers at the screen edge"),
                );
                ui.weak(tr(
                    "Text and images. Files are planned for a later release.",
                ));
                ui.checkbox(
                    &mut settings.monitor_enabled,
                    tr("Switch the shared monitor"),
                );
                selection(
                    ui,
                    "group-input-source",
                    "Keyboard & mouse from",
                    &mut settings.input,
                    &state,
                );
                selection(
                    ui,
                    "group-audio-output",
                    "Play all sound on",
                    &mut settings.audio,
                    &state,
                );
                let previous = settings.microphone.clone();
                selection(
                    ui,
                    "group-microphone-source",
                    "Microphone from",
                    &mut settings.microphone,
                    &state,
                );
                if settings.microphone != previous {
                    settings.microphone_binding = None;
                }
                if settings != state.settings {
                    self.group_action(Action::Settings {
                        expected: state.revision,
                        value: settings,
                    });
                }
                ui.separator();
                for member in &members {
                    let system = plan.audio.as_ref() == Some(&member.node.key);
                    let mut preferences = member.preferences.clone();
                    let mut level = if system {
                        member
                            .native_output
                            .or(preferences.output)
                            .unwrap_or_default()
                    } else {
                        preferences.audio
                    };
                    let before = level;
                    ui.horizontal(|ui| {
                        ui.add_sized([150.0, 20.0], egui::Label::new(&member.node.name));
                        ui.add(egui::Slider::new(&mut level.volume, 0..=100).suffix(" %"));
                        ui.checkbox(&mut level.muted, tr("Mute"));
                        ui.weak(tr(if system {
                            "System volume"
                        } else {
                            "Relative volume"
                        }));
                    });
                    if level != before {
                        if system {
                            preferences.output = Some(level);
                        } else {
                            preferences.audio = level;
                        }
                        self.group_action(Action::Member {
                            key: member.node.key.clone(),
                            expected: member.preferences.clone(),
                            value: preferences,
                        });
                    }
                }
            });
        ui.add_space(12.0);
        egui::CollapsingHeader::new(tr("Add a computer"))
            .default_open(state.members.len() < 2)
            .show(ui, |ui| {
                let available = self
                    .discovered
                    .values()
                    .filter(|peer| {
                        peer.public_key
                            .as_ref()
                            .is_some_and(|key| !state.members.contains_key(key))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                for peer in available {
                    ui.horizontal(|ui| {
                        ui.label(&peer.name);
                        ui.weak(&peer.version);
                        if ui.button(tr("Connect")).clicked() {
                            if let (Some(key), Some(address)) =
                                (peer.public_key, peer.addresses.first())
                            {
                                self.group_action(Action::Invite {
                                    key,
                                    address: SocketAddr::new(address.ip(), model::PORT),
                                });
                            }
                        }
                    });
                }
                ui.horizontal(|ui| {
                    ui.label(tr("IP address"));
                    ui.text_edit_singleline(&mut self.group.manual);
                    if ui.button(tr("Connect")).clicked() {
                        let value = self.group.manual.trim();
                        let address = value.parse::<SocketAddr>().or_else(|_| {
                            value
                                .parse::<std::net::IpAddr>()
                                .map(|ip| SocketAddr::new(ip, model::PORT))
                        });
                        match address {
                            Ok(address) => self.group_action(Action::Invite {
                                address,
                                key: String::new(),
                            }),
                            Err(error) => self.notice = Some((error.to_string(), true)),
                        }
                    }
                });
            });
        if ui.button(tr("Retry devices")).clicked() {
            self.group.data.retry();
        }
    }

    pub(super) fn group_settings(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.heading(tr("Settings"));
        let Some(state) = self.group.workspace.clone() else {
            return;
        };
        egui::ComboBox::from_id_salt("settings-computer")
            .width(320.0)
            .selected_text(
                state
                    .members
                    .get(&self.group.selected)
                    .map(|member| member.node.name.as_str())
                    .unwrap_or("This PC"),
            )
            .show_ui(ui, |ui| {
                for member in state.members.values() {
                    ui.selectable_value(
                        &mut self.group.selected,
                        member.node.key.clone(),
                        &member.node.name,
                    );
                }
            });
        let Some(member) = state.members.get(&self.group.selected).cloned() else {
            return;
        };
        let local = member.node.key == self.public_key;
        self.settings_remote = !local;
        if !local {
            self.pair.remote = Some(member.node.clone());
            self.peer_key = member.node.key.clone();
        }
        ui.label(tr("Changes apply to the selected computer."));
        ui.add_space(12.0);
        if let Some(management) = &member.node.management {
            let mut application = if local {
                self.local_application()
            } else {
                management.application.clone()
            };
            let before = application.clone();
            egui::Frame::group(ui.style())
                .inner_margin(16)
                .show(ui, |ui| {
                    ui.strong(tr("Application"));
                    egui::ComboBox::from_id_salt("group-theme")
                        .selected_text(tr(&application.theme))
                        .show_ui(ui, |ui| {
                            for theme in ["system", "dark", "light"] {
                                ui.selectable_value(
                                    &mut application.theme,
                                    theme.into(),
                                    tr(theme),
                                );
                            }
                        });
                    egui::ComboBox::from_id_salt("group-language")
                        .selected_text(
                            crate::i18n::LANGUAGES
                                .iter()
                                .find(|(code, _)| *code == application.language)
                                .map(|(_, name)| *name)
                                .unwrap_or("English"),
                        )
                        .show_ui(ui, |ui| {
                            for (code, name) in crate::i18n::LANGUAGES {
                                ui.selectable_value(&mut application.language, code.into(), name);
                            }
                        });
                    ui.checkbox(&mut application.autostart, tr("Start at login"));
                    ui.checkbox(
                        &mut application.auto_connect,
                        tr("Connect the workspace at application startup"),
                    );
                    ui.checkbox(
                        &mut application.auto_update,
                        tr("Check for updates at startup"),
                    );
                    if member.node.diagnostics.platform == "windows" {
                        ui.checkbox(
                            &mut application.show_remote_cursor,
                            tr("Show the remote cursor on Windows"),
                        );
                    }
                    if ui.button(tr("VB-CABLE installers…")).clicked() {
                        self.cable_open = true;
                        self.cable_remote = !local;
                    }
                });
            if application != before {
                if local {
                    if let Err(error) = self.apply_application(&application, &ctx) {
                        self.notice = Some((error.to_string(), true));
                    }
                } else {
                    self.group_action(Action::Manage {
                        target: member.node.key.clone(),
                        message: workspace::management::Message::SetApplication {
                            id: super::management::request_id(),
                            expected: before,
                            application,
                        },
                    });
                }
            }
            ui.add_space(12.0);
            egui::Frame::group(ui.style())
                .inner_margin(16)
                .show(ui, |ui| {
                    ui.strong(tr("Updates"));
                    ui.label(format!(
                        "{} {}",
                        tr("Installed version:"),
                        member
                            .node
                            .version
                            .as_ref()
                            .map(|version| version.app.as_str())
                            .unwrap_or("—")
                    ));
                    ui.label(tr(&management.update.status));
                    ui.horizontal(|ui| {
                        if ui.button(tr("Check for updates")).clicked() {
                            if local {
                                self.check_updates(&ctx);
                            } else {
                                self.group_action(Action::Manage {
                                    target: member.node.key.clone(),
                                    message: workspace::management::Message::CheckUpdates {
                                        id: super::management::request_id(),
                                    },
                                });
                            }
                        }
                        if ui
                            .add_enabled(
                                management.update.can_install(),
                                egui::Button::new(tr("Download and install")),
                            )
                            .clicked()
                        {
                            if local {
                                self.install_update(&ctx);
                            } else {
                                self.group.update_target = Some(member.node.key.clone());
                                self.start_remote_update(&ctx);
                            }
                        }
                    });
                });
        }
        ui.add_space(12.0);
        let mut preferences = member.preferences.clone();
        egui::Frame::group(ui.style())
            .inner_margin(16)
            .show(ui, |ui| {
                ui.strong(tr("Devices on this computer"));
                for (id, label, value, devices) in [
                    (
                        "member-keyboard",
                        "Keyboard",
                        &mut preferences.devices.keyboard,
                        &member.node.devices.keyboards,
                    ),
                    (
                        "member-mouse",
                        "Mouse",
                        &mut preferences.devices.mouse,
                        &member.node.devices.mice,
                    ),
                    (
                        "member-speaker",
                        "Headphones / speakers",
                        &mut preferences.devices.speaker,
                        &member.node.devices.speakers,
                    ),
                    (
                        "member-microphone",
                        "Microphone",
                        &mut preferences.devices.microphone,
                        &member.node.devices.microphones,
                    ),
                    (
                        "member-bridge",
                        "Virtual microphone",
                        &mut preferences.devices.microphone_bridge,
                        &member.node.devices.microphone_bridges,
                    ),
                ] {
                    ui.label(tr(label));
                    egui::ComboBox::from_id_salt(id)
                        .width(420.0)
                        .selected_text(
                            value
                                .as_ref()
                                .and_then(|id| devices.iter().find(|device| &device.id == id))
                                .map(|device| device.name.clone())
                                .unwrap_or_else(|| tr("Automatic")),
                        )
                        .show_ui(ui, |ui| {
                            ui.selectable_value(value, None, tr("Automatic"));
                            for device in devices {
                                ui.selectable_value(value, Some(device.id.clone()), &device.name);
                            }
                        });
                }
            });
        if preferences != member.preferences {
            self.group_action(Action::Member {
                key: member.node.key.clone(),
                expected: member.preferences.clone(),
                value: preferences,
            });
        }
        ui.add_space(12.0);
        let mut monitor = member.node.monitor.clone();
        egui::Frame::group(ui.style())
            .inner_margin(16)
            .show(ui, |ui| {
                ui.strong(tr("Shared monitor"));
                ui.checkbox(
                    &mut monitor.enabled,
                    tr("This computer uses the shared monitor"),
                );
                ui.label(&monitor.name);
                egui::ComboBox::from_id_salt("member-monitor-input")
                    .selected_text(
                        monitor
                            .input
                            .map(|input| format!("0x{input:02x}"))
                            .unwrap_or_else(|| tr("Choose this computer's port")),
                    )
                    .show_ui(ui, |ui| {
                        for (input, label) in [
                            (15, "DisplayPort"),
                            (16, "DisplayPort 2"),
                            (17, "HDMI 1"),
                            (18, "HDMI 2"),
                            (3, "DVI 1"),
                        ] {
                            ui.selectable_value(&mut monitor.input, Some(input), label);
                        }
                    });
                if local && ui.button(tr("Read the current input once")).clicked() {
                    self.scan(QueryKind::Monitors);
                }
            });
        if monitor != member.node.monitor {
            if local {
                self.prefs.workspace.monitor = monitor;
                let _ = self.prefs.save(&self.dir);
            } else {
                self.group_action(Action::Manage {
                    target: member.node.key.clone(),
                    message: workspace::management::Message::SetMonitor {
                        id: super::management::request_id(),
                        expected: member.node.monitor.clone(),
                        monitor,
                    },
                });
            }
        }
    }
}

pub(super) fn selection(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    value: &mut Selection,
    state: &Workspace,
) {
    ui.horizontal(|ui| {
        ui.add_sized([180.0, 20.0], egui::Label::new(tr(label)));
        let caption = match value {
            Selection::Auto => tr("Automatic"),
            Selection::Computer(key) => state
                .members
                .get(key)
                .map(|member| member.node.name.clone())
                .unwrap_or_else(|| tr("Unavailable")),
        };
        egui::ComboBox::from_id_salt(id)
            .width(260.0)
            .selected_text(caption)
            .show_ui(ui, |ui| {
                ui.selectable_value(value, Selection::Auto, tr("Automatic"));
                for member in state.members.values() {
                    ui.add_enabled_ui(member.compatible(), |ui| {
                        ui.selectable_value(
                            value,
                            Selection::Computer(member.node.key.clone()),
                            &member.node.name,
                        );
                    });
                }
            });
    });
}
