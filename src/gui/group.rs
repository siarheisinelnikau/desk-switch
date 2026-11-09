//! Consumer workspace for any number of linked computers.
#[cfg(target_os = "linux")]
use super::QueryKind;
use super::{Desktop, Page, group_data::Data};
use crate::{
    config,
    group::{
        model::{self, Member, Workspace},
        net::{Action, Event, Service},
    },
    workspace::{self, Node, Selection},
};
use anyhow::{Context, Result};
use eframe::egui::{self, Color32, RichText};
use std::{collections::BTreeMap, net::SocketAddr};

fn tr(text: &str) -> String {
    crate::i18n::tr(text)
}

fn clear_recovered_input_notice(
    notice: &mut Option<(String, bool)>,
    input_notice: &mut Option<String>,
) {
    if let Some(previous) = input_notice.take()
        && notice
            .as_ref()
            .is_some_and(|(message, error)| *error && *message == previous)
    {
        *notice = None;
    }
}

#[derive(Default)]
pub(super) struct Group {
    pub compat: super::group_compat::Compat,
    pub service: Option<Service>,
    pub workspace: Option<Workspace>,
    pub selected: String,
    remove_confirmation: Option<String>,
    reset_confirmation: bool,
    pub(super) removed: bool,
    requests: std::collections::VecDeque<(std::time::Instant, crate::group::net::Request)>,
    pub data: Data,
    pub(super) coordination: Option<Coordination>,
    pub(super) file_transfer: Option<crate::group::file_transfer::Service>,
    pub layout_editor: super::group_layout::Editor,
    network_log: std::collections::VecDeque<String>,
    network_reconnecting: std::collections::BTreeSet<String>,
    pub return_pending: bool,
    return_request: Option<u64>,
    return_retry_at: Option<std::time::Instant>,
    input_notice: Option<String>,
    pub(super) probe: Option<workspace::hardware::Probe>,
    last_node: Option<Node>,
    last_volume: Option<workspace::AudioLevel>,
    last_ready: u64,
    pub(super) input_ready: u64,
    pub(super) monitor_wait: Option<(u64, std::time::Instant)>,
    pub(super) monitor_error: Option<String>,
    pub(super) monitor_observed: Option<String>,
    pub(super) monitor_notice_id: Option<(String, u64)>,
    pub(super) monitor_history: std::collections::VecDeque<String>,
    pub(super) monitor_loaded: Option<(String, u64)>,
    pub(super) monitor_attempt: Option<super::group_monitor::Attempt>,
    pub(super) monitor_result: Option<(super::group_monitor::Attempt, Option<String>)>,
    pub(super) monitor_reply_at: Option<std::time::Instant>,
    pub reply_target: Option<String>,
    pub cable_target: Option<String>,
    pub update_target: Option<String>,
    pub monitor_read_reply: Option<(String, u64)>,
    pub(super) monitor_job: Option<super::worker::Worker>,
    pub preview: bool,
    pub add_open: bool,
    add_mode: AddMode,
    add_pending: Option<AddPending>,
    add_result: Option<(String, bool)>,
    #[cfg(target_os = "linux")]
    input_permission_retries: std::collections::BTreeSet<String>,
    manual: String,
    pub clipboard: Option<crate::group::clipboard::Clipboard>,
}

#[derive(Clone, Copy)]
pub(super) struct Coordination {
    pub writable: bool,
    pub online: usize,
    pub quorum: usize,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum AddMode {
    #[default]
    Nearby,
    Manual,
}
struct AddPending {
    address: SocketAddr,
    key: String,
    until: std::time::Instant,
}

impl Group {
    pub(super) fn writable(&self) -> bool {
        self.coordination.is_none_or(|status| status.writable)
    }

    fn connection_status(&self, ui: &mut egui::Ui) {
        if let Some(status) = self.coordination.filter(|status| !status.writable) {
            let enough = status.online >= status.quorum;
            ui.strong(tr(if enough {
                "Reconnecting…"
            } else {
                "Waiting for another computer"
            }));
            ui.weak(format!(
                "{}: {} / {}",
                tr("Connected / needed"),
                status.online,
                status.quorum,
            ));
            ui.weak(tr(if enough {
                "Shared controls will be available after reconnecting."
            } else {
                "Connect another saved computer to change shared settings."
            }));
        }
    }

    fn log_network(&mut self, message: String) {
        let message = workspace::management::status_text(message, 288);
        if self.network_log.back() == Some(&message) {
            return;
        }
        if self.network_log.len() == 12 {
            self.network_log.pop_front();
        }
        self.network_log.push_back(message);
    }

    fn report_network(&self, node: &mut Node) {
        for message in &self.network_log {
            node.diagnostics.line("Workspace", message);
        }
    }

    pub(super) fn monitor_busy(&self) -> bool {
        self.monitor_job.is_some()
            || self.monitor_wait.is_some()
            || self
                .workspace
                .as_ref()
                .is_some_and(Workspace::switch_pending)
    }
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
                    Action::Activate { key } => {
                        if state.activate(&self.public_key, &key, None).is_ok()
                            && let Some(command) = state.monitor_command.clone()
                        {
                            let _ = state.monitor_result(&command.writer, command.id, None);
                        }
                    }
                    Action::ActivateEdge { key, position } => {
                        if state
                            .activate(&self.public_key, &key, Some(position))
                            .is_ok()
                            && let Some(command) = state.monitor_command.clone()
                        {
                            let _ = state.monitor_result(&command.writer, command.id, None);
                        }
                    }
                    Action::SetLayout { expected, layout } => {
                        let _ = state.set_layout(expected, layout);
                    }
                    Action::ActivateDisplay { target, entry } => {
                        if state
                            .activate_display(&self.public_key, target, entry)
                            .is_ok()
                            && let Some(command) = state.monitor_command.clone()
                            && command.result.is_none()
                        {
                            let _ = state.monitor_result(&command.writer, command.id, None);
                        }
                    }
                    Action::CrossEdge { crossing } => {
                        if let Ok(Some(activation)) = crate::group::layout::resolve_crossing(
                            state,
                            &self.public_key,
                            crossing,
                        ) {
                            let _ = state.activate_display(
                                &self.public_key,
                                activation.target,
                                Some(activation.entry),
                            );
                        }
                    }
                    Action::ReturnInput => {
                        let _ = state.return_input();
                    }
                    Action::EdgePause(paused) => state.edge_paused = paused,
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
        if self.group.preview || self.group.removed {
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
                self.start_file_transfers(ctx);
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
                Event::Coordination {
                    leader,
                    writable,
                    online,
                    quorum,
                } => {
                    self.group.coordination = Some(Coordination {
                        writable: writable && leader.is_some(),
                        online,
                        quorum,
                    });
                }
                Event::Removed => {
                    self.group.removed = true;
                    self.group.service.take();
                    self.group.file_transfer.take();
                    self.group.clipboard.take();
                    self.group.data.stop();
                    self.stop_pair();
                    self.page = Page::Application;
                    self.action(super::Action::Show, ctx);
                    return;
                }
                Event::Request(request) => {
                    self.page = Page::Overview;
                    self.group.add_open = false;
                    if self.group.requests.len() < 16 {
                        let timeout = crate::group::net::CONSENT_TIMEOUT;
                        self.group
                            .requests
                            .push_back((std::time::Instant::now() + timeout, request));
                        ctx.request_repaint_after_for(timeout, egui::ViewportId::ROOT);
                        self.action(super::Action::Show, ctx);
                    }
                }
                Event::RequestExpired(id) => {
                    self.group.requests.retain(|(_, request)| request.id != id)
                }
                Event::Error(error) => {
                    self.group.log_network(error.clone());
                    if self.group.add_pending.take().is_some() {
                        self.group.add_result = Some((error.clone(), true));
                    }
                    self.notice = Some((error, true));
                }
                Event::Reconnecting { key, reason } => {
                    self.group.network_reconnecting.insert(key.clone());
                    let name = self
                        .group
                        .workspace
                        .as_ref()
                        .and_then(|state| state.members.get(&key))
                        .map(|member| member.node.name.as_str())
                        .unwrap_or(&key);
                    self.group.log_network(format!(
                        "Connection to {name} interrupted; reconnecting: {reason}"
                    ));
                }
                Event::State(state) => {
                    let restored =
                        self.group
                            .network_reconnecting
                            .iter()
                            .filter_map(|key| {
                                state.members.get(key).filter(|member| member.online).map(
                                    |member| {
                                        (
                                            key.clone(),
                                            format!("Connection to {} restored", member.node.name),
                                        )
                                    },
                                )
                            })
                            .collect::<Vec<_>>();
                    for (key, message) in restored {
                        self.group.network_reconnecting.remove(&key);
                        self.group.log_network(message);
                    }
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
                    if let Some(pending) = &self.group.add_pending
                        && let Some(member) = state.members.values().find(|member| {
                            member.online
                                && member.node.key != self.public_key
                                && (if pending.key.is_empty() {
                                    member.address.ip().to_canonical()
                                        == pending.address.ip().to_canonical()
                                } else {
                                    member.node.key == pending.key
                                })
                        })
                    {
                        self.group.add_result =
                            Some((format!("{}: {}", tr("Connected"), member.node.name), false));
                        self.group.add_pending = None;
                    }
                    self.group.workspace = Some(*state);
                }
                Event::Management { source, message } => {
                    if let workspace::management::Message::Reply {
                        error: Some(error), ..
                    } = &message
                    {
                        self.notice = Some((error.clone(), true));
                    }
                    self.group.reply_target = Some(source);
                    self.receive_management(message, ctx);
                    self.group.reply_target = None;
                }
                Event::Update { source, message } => {
                    let current = self.paired_update.is_some()
                        && self.group.update_target.as_ref() == Some(&source);
                    if current {
                        self.receive_update(message, ctx);
                    } else if let workspace::update::Message::Prepare { id, .. } = &message {
                        if self.update_action_busy() || self.download_action.is_some() {
                            let _ = self.update_send_to(
                                &source,
                                workspace::update::Message::Failed {
                                    id: *id,
                                    message: "Another update operation is already in progress."
                                        .into(),
                                },
                            );
                        } else {
                            self.group.update_target = Some(source);
                            self.receive_update(message, ctx);
                        }
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
                            && !self.prefs.pause_local_sharing
                            && state.members.get(&source).is_some_and(Member::compatible)
                    }) && let Some(clipboard) = &self.group.clipboard
                    {
                        clipboard.receive(mime, bytes, digest);
                    }
                }
            }
        }
        if let Some(pending) = &self.group.add_pending {
            let remaining = pending
                .until
                .saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                self.group.add_pending = None;
                self.group.add_result = Some((
                    "The connection was not approved. Check the address and try again.".into(),
                    true,
                ));
            } else {
                ctx.request_repaint_after_for(remaining, egui::ViewportId::ROOT);
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
                let enabled = state.settings.clipboard_enabled && !self.prefs.pause_local_sharing;
                clipboard.enabled(enabled);
                for (mime, bytes, digest) in clipboard.poll().into_iter().filter(|_| enabled) {
                    self.group_action(Action::Clipboard {
                        mime,
                        bytes,
                        digest,
                    });
                }
            }
            self.update_file_transfers(&state);
            let mut specs = if self.quitting || self.install_pending {
                BTreeMap::new()
            } else {
                match self.group.data.desired(
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
            if self.prefs.pause_local_sharing {
                specs.retain(|_, spec| spec.kind != super::worker::Kind::MicrophoneSend);
            }
            self.group.data.tick(specs, &self.dir, ctx);
            #[cfg(target_os = "linux")]
            self.request_group_input_access_if_needed(ctx);
            if self.group.data.cycle {
                self.group.data.cycle = false;
                self.group_cycle();
            }
            if let Some(crossing) = self.group.data.crossing.take() {
                self.group_action(Action::CrossEdge { crossing });
            }
            if self.group.data.returned {
                self.group.data.returned = false;
                if !self.group.return_pending {
                    self.group.return_request = Some(state.input_request);
                    self.group.return_retry_at = None;
                }
                self.group.return_pending = true;
            }
            if self.group.return_pending {
                let source = state.plan().input;
                let acknowledged = state.active_input == source && state.input_intent == source;
                let superseded = self
                    .group
                    .return_request
                    .is_some_and(|request| state.input_request > request);
                if acknowledged || superseded {
                    self.group.return_pending = false;
                    self.group.return_request = None;
                    self.group.return_retry_at = None;
                } else if !state.switch_pending()
                    && state
                        .members
                        .get(&state.hub)
                        .is_some_and(|member| member.online)
                {
                    let now = std::time::Instant::now();
                    if self.group.return_retry_at.is_none_or(|until| now >= until) {
                        self.group.return_retry_at = Some(now + std::time::Duration::from_secs(1));
                        self.group_action(Action::ReturnInput);
                    }
                    if let Some(until) = self.group.return_retry_at {
                        ctx.request_repaint_after_for(
                            until.saturating_duration_since(now),
                            egui::ViewportId::ROOT,
                        );
                    }
                }
            }
            if std::mem::take(&mut self.group.data.input_recovered) {
                clear_recovered_input_notice(&mut self.notice, &mut self.group.input_notice);
            }
            if let Some(error) = self.group.data.error.take() {
                if self.group.data.input_notice.take().as_deref() == Some(error.as_str()) {
                    self.group.input_notice = Some(error.clone());
                }
                self.notice = Some((error, true));
            }
            self.tick_group_monitor(&state, ctx);
        }
        let mut node = self.local_node();
        self.group.data.report(&mut node);
        self.group.report_network(&mut node);
        self.report_group_monitor_history(&mut node);
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

    #[cfg(target_os = "linux")]
    fn request_group_input_access_if_needed(&mut self, ctx: &egui::Context) {
        if self.quitting
            || self.install_pending
            || self.query.is_some()
            || self
                .group
                .workspace
                .as_ref()
                .and_then(|state| state.plan().input)
                .as_deref()
                != Some(self.public_key.as_str())
        {
            return;
        }
        if let Some(delay) = self.group.data.permission_retry_after() {
            ctx.request_repaint_after(delay);
            return;
        }
        if !self.group.data.permission_denied() {
            return;
        }
        let paths = self.input_permission_paths();
        if paths.is_empty() {
            return;
        }
        let selection = self.input_access_selection();
        // udev can restore a saved grant just after the first device open. Give
        // the selected native profile one retry before requesting new access.
        if paths.iter().all(|path| std::fs::File::open(path).is_ok())
            && self
                .group
                .input_permission_retries
                .insert(selection.clone())
        {
            self.group.data.retry();
            return;
        }
        if !self.pair.input_access_attempts.contains(&selection) {
            // scan records this selection before launching the helper. A denied
            // or cancelled request therefore cannot reopen itself on the next tick.
            self.scan(QueryKind::InputAccess);
        }
    }

    pub(super) fn group_overview(&mut self, ui: &mut egui::Ui) {
        ui.label(tr(
            "Share your devices across all connected computers. Change settings from any computer.",
        ));
        ui.add_space(12.0);
        self.group.connection_status(ui);
        let now = std::time::Instant::now();
        self.group.requests.retain(|(deadline, _)| *deadline > now);
        if let Some((_, request)) = self.group.requests.front().cloned() {
            egui::Frame::group(ui.style())
                .inner_margin(16)
                .show(ui, |ui| {
                    ui.strong(format!("{} · {}", tr("Connection request"), request.name));
                    ui.label(format!("{}: {}", tr("Security code"), request.code));
                    ui.horizontal(|ui| {
                        for (text, accept) in [("Codes match", true), ("Decline", false)] {
                            if ui.button(tr(text)).clicked()
                                && let Some(service) = &self.group.service
                            {
                                match service.approve(request.id, accept) {
                                    Ok(()) => {
                                        self.group.requests.pop_front();
                                    }
                                    Err(error) => self.notice = Some((format!("{error:#}"), true)),
                                }
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
        let visual = super::view::WorkspaceVisual::from_workspace(&state);
        if let Some(monitor) = &visual.monitor {
            egui::Frame::group(ui.style())
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    super::view::shared_monitor(ui, monitor, false);
                });
            ui.add_space(8.0);
        }
        ui.weak(tr("Device colour shows which PC it comes from"));
        let columns = if ui.available_width() >= 600.0 { 2 } else { 1 };
        let width = (ui.available_width() - 12.0 * (columns - 1) as f32) / columns as f32;
        for (row_index, row) in members.chunks(columns).enumerate() {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for (column, member) in row.iter().enumerate() {
                    let index = row_index * columns + column;
                    let key = &member.node.key;
                    let accent: Color32 = super::view::peer_color(index);
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::Frame::group(ui.style())
                                .fill(ui.visuals().window_fill())
                                .corner_radius(12)
                                .inner_margin(16)
                                .show(ui, |ui| {
                                    ui.set_width(width - 34.0);
                                    ui.set_min_height(106.0);
                                    ui.spacing_mut().item_spacing.y = 6.0;
                                    ui.horizontal_wrapped(|ui| {
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
                                    let devices = visual.member_devices(&state, key);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        for icon in super::view::DeviceIcon::ALL {
                                            super::view::device_icon(
                                                ui,
                                                icon,
                                                devices[icon.index()].as_ref(),
                                                0.8,
                                            );
                                        }
                                    });
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
                                    if member.online && !member.compatible() {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.weak(tr("Update required to share devices."));
                                            if ui.link(tr("Updates")).clicked() {
                                                self.group.selected = key.clone();
                                                self.page = Page::Application;
                                            }
                                        });
                                    } else if !member.online {
                                        ui.weak(tr(
                                            "Open Desk Switch on this computer to reconnect.",
                                        ));
                                    } else if member.node.input_error.is_some() {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.weak(tr("Keyboard and mouse need attention."));
                                            if ui.link(tr("Diagnostics")).clicked() {
                                                self.page = Page::Diagnostics;
                                            }
                                        });
                                    }
                                    ui.horizontal_wrapped(|ui| {
                                        let active = state.active_input.as_ref() == Some(key);
                                        let failed = active && member.node.monitor_error.is_some();
                                        let video_needed = state.settings.monitor_enabled
                                            && member.node.monitor.enabled
                                            && state.active_monitor.as_ref() != Some(key);
                                        if ui
                                            .add_enabled(
                                                member.compatible()
                                                    && self.group.writable()
                                                    && (!active || failed || video_needed)
                                                    && !state.switch_pending(),
                                                egui::Button::new(tr(if failed {
                                                    "Retry switch"
                                                } else if active && state.switch_pending() {
                                                    "Switching…"
                                                } else if active && !video_needed {
                                                    "Input target"
                                                } else {
                                                    "Switch here"
                                                })),
                                            )
                                            .clicked()
                                        {
                                            self.group_action(Action::Activate {
                                                key: key.clone(),
                                            });
                                        }
                                        if ui.button(tr("Settings")).clicked() {
                                            self.transfers.close();
                                            self.group.add_open = false;
                                            self.group.selected = key.clone();
                                            self.page = Page::Application;
                                        }
                                        self.transfer_card_action(ui, key, member.compatible());
                                    });
                                    if let Some(error) = &member.node.monitor_error {
                                        ui.colored_label(
                                            ui.visuals().error_fg_color,
                                            tr("Monitor switch failed"),
                                        )
                                        .on_hover_text(error);
                                    }
                                })
                        },
                    );
                }
            });
        }
        ui.add_space(4.0);
        egui::Frame::group(ui.style())
            .inner_margin(16)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.strong(tr("Shared devices"));
                let mut settings = state.settings.clone();
                let previous = settings.microphone.clone();
                let status = self
                    .group
                    .clipboard
                    .as_ref()
                    .and_then(|clipboard| clipboard.status());
                let mut arrange = false;
                let layout_supported = self.group.writable();
                if !self.group.writable() {
                    ui.disable();
                }
                if ui.available_width() >= 700.0 {
                    ui.columns(2, |columns| {
                        arrange = sharing_toggles(
                            &mut columns[0],
                            &mut settings,
                            status.as_deref(),
                            &visual,
                            layout_supported,
                        );
                        sharing_sources(&mut columns[1], &mut settings, &state);
                    });
                } else {
                    arrange = sharing_toggles(
                        ui,
                        &mut settings,
                        status.as_deref(),
                        &visual,
                        layout_supported,
                    );
                    sharing_sources(ui, &mut settings, &state);
                }
                if arrange {
                    self.group.add_open = false;
                    self.page = Page::Application;
                    self.group.layout_editor.open(&state);
                }
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
                for (index, member) in members.iter().enumerate() {
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
                        ui.add_sized(
                            [150.0, 20.0],
                            egui::Label::new(
                                RichText::new(&member.node.name)
                                    .color(super::view::peer_color(index)),
                            ),
                        );
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
    }

    fn begin_group_invite(&mut self, address: SocketAddr, key: String, ctx: &egui::Context) {
        if self.group.add_pending.is_some() {
            return;
        }
        let result = self
            .group
            .service
            .as_ref()
            .context("The workspace is disconnected")
            .and_then(|service| {
                service.action(Action::Invite {
                    address,
                    key: key.clone(),
                })
            });
        match result {
            Ok(()) => {
                self.group.add_result = Some((
                    "Connection requested. Compare the security codes on both computers.".into(),
                    false,
                ));
                self.group.add_pending = Some(AddPending {
                    address,
                    key,
                    until: std::time::Instant::now() + std::time::Duration::from_secs(100),
                });
                ctx.request_repaint_after_for(
                    std::time::Duration::from_secs(100),
                    egui::ViewportId::ROOT,
                );
            }
            Err(error) => self.group.add_result = Some((error.to_string(), true)),
        }
    }
    pub(super) fn group_add_dialog(&mut self, ctx: &egui::Context) {
        if !self.group.add_open {
            return;
        }
        let Some(state) = self.group.workspace.clone() else {
            return;
        };
        let mut open = self.group.add_open;
        egui::Window::new(tr("Add a computer"))
            .id(egui::Id::new("workspace-add-computer"))
            .open(&mut open).collapsible(false).resizable(false).default_width(520.0)
            .default_pos(ctx.content_rect().center() - egui::vec2(260.0, 180.0))
            .show(ctx, |ui| {
                ui.label(tr("Connect over your local network or VPN. Approve the request on the other computer."));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.group.add_mode, AddMode::Nearby, tr("Nearby computers"));
                    ui.selectable_value(&mut self.group.add_mode, AddMode::Manual, tr("Manual IP"));
                });
                ui.separator();
                let waiting = self.group.add_pending.is_some();
                if self.group.add_mode == AddMode::Manual {
                    ui.label(tr("IP address"));
                    let edit = ui.add(egui::TextEdit::singleline(&mut self.group.manual)
                        .desired_width(f32::INFINITY).margin(egui::Margin::symmetric(10, 9)).hint_text("192.168.1.20"));
                    if edit.changed() && !waiting { self.group.add_result = None; }
                    ui.weak(tr("Use IP, IP:port, or [IPv6]:port. Default port: 45880."));
                    let enter = edit.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    let connect = ui.add_enabled(!waiting && !self.group.manual.trim().is_empty(), egui::Button::new(tr("Connect"))).clicked();
                    if (connect || enter) && !waiting {
                        match manual_address(&self.group.manual) {
                            Ok(address) => self.begin_group_invite(address, String::new(), ctx),
                            Err(error) => self.group.add_result = Some((error.to_string(), true)),
                        }
                    }
                } else {
                    let available = self.discovered.values().filter(|peer| peer.public_key.as_ref().is_some_and(|key| state.members.get(key).is_none_or(|member| !member.online))).cloned().collect::<Vec<_>>();
                    if available.is_empty() {
                        ui.weak(tr("Nearby computers will appear here. You can also connect by IP address."));
                        if ui.button(tr("Use an IP address")).clicked() { self.group.add_mode = AddMode::Manual; }
                    }
                    egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                        for peer in available {
                            ui.horizontal_wrapped(|ui| {
                                ui.add(egui::Label::new(egui::RichText::new(&peer.name).strong()).truncate());
                                ui.weak(&peer.version);
                                let address = peer.addresses.iter().find(|address| address.is_ipv4()).or_else(|| peer.addresses.first()).copied();
                                if ui.add_enabled(!waiting && address.is_some(), egui::Button::new(tr("Connect"))).clicked()
                                    && let (Some(key), Some(address)) = (peer.public_key, address)
                                {
                                    self.begin_group_invite(SocketAddr::new(address.ip(), model::PORT), key, ctx);
                                }
                            });
                        }
                    });
                }
                if let Some((message, failed)) = &self.group.add_result {
                    ui.add_space(8.0);
                    ui.colored_label(if *failed { ui.visuals().error_fg_color } else { ui.visuals().text_color() }, tr(message));
                }
            });
        self.group.add_open = open;
    }
    #[cfg(debug_assertions)]
    pub(super) fn preview_manual_connection(&mut self) {
        self.group.add_open = true;
        self.group.add_mode = AddMode::Manual;
        self.group.manual = "192.0.2.42".into();
    }

    pub(super) fn group_settings(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.group.removed {
            ui.label(tr("This computer was removed from the workspace."));
            self.workspace_recovery(ui, &ctx);
            return;
        }
        let Some(state) = self.group.workspace.clone() else {
            return;
        };
        self.group.connection_status(ui);
        if state.members.len() > 1 && !self.group.writable() {
            self.workspace_recovery(ui, &ctx);
        }
        ui.horizontal_wrapped(|ui| {
            ui.strong(tr("Workspace displays"));
            if ui
                .add_enabled(
                    self.group.writable(),
                    egui::Button::new(tr("Arrange displays…")),
                )
                .on_disabled_hover_text(tr("Reconnect the workspace to arrange displays."))
                .clicked()
            {
                self.group.layout_editor.open(&state);
            }
        });
        ui.weak(tr(
            "Choose where the pointer goes when it reaches a screen edge.",
        ));
        let mut settings = state.settings.clone();
        ui.add_enabled_ui(self.group.writable(), |ui| {
            ui.checkbox(
                &mut settings.edge,
                tr("Move between computers at the screen edge"),
            );
            ui.add_enabled_ui(settings.edge, |ui| {
                edge_settings(ui, &mut settings);
                if state.edge_paused {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr("Edge switching is paused"));
                        if ui.small_button(tr("Resume edge switching")).clicked() {
                            self.group_action(Action::EdgePause(false));
                        }
                    });
                }
            });
        });
        if settings != state.settings {
            self.group_action(Action::Settings {
                expected: state.revision,
                value: settings,
            });
        }
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(12.0);
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
        if !local && member.node.key != state.hub {
            if ui
                .add_enabled(
                    self.group.writable() && !self.group.monitor_busy(),
                    egui::Button::new(tr("Remove computer…")),
                )
                .clicked()
            {
                self.group.remove_confirmation = Some(member.node.key.clone());
            }
            if self.group.remove_confirmation.as_ref() == Some(&member.node.key) {
                ui.label(tr("This computer will need approval to join again."));
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.group.writable(),
                            egui::Button::new(tr("Remove computer")),
                        )
                        .clicked()
                    {
                        self.group.remove_confirmation = None;
                        self.group_action(Action::RemoveMember {
                            key: member.node.key.clone(),
                        });
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        self.group.remove_confirmation = None;
                    }
                });
            }
        }
        ui.add_space(12.0);
        if ui.available_width() >= 740.0 {
            ui.columns(2, |columns| {
                self.group_application_settings(&mut columns[0], &member, local, &ctx);
                columns[0].add_space(12.0);
                self.group_device_settings(&mut columns[0], &member);
                self.group_update_settings(&mut columns[1], &member, local, &ctx);
                columns[1].add_space(12.0);
                self.group_monitor_settings(&mut columns[1], &member, local, &ctx);
            });
        } else {
            self.group_application_settings(ui, &member, local, &ctx);
            ui.add_space(12.0);
            self.group_update_settings(ui, &member, local, &ctx);
            ui.add_space(12.0);
            self.group_device_settings(ui, &member);
            ui.add_space(12.0);
            self.group_monitor_settings(ui, &member, local, &ctx);
        }
    }
    fn workspace_recovery(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if ui.button(tr("Start a new workspace…")).clicked() {
            self.group.reset_confirmation = true;
        }
        if self.group.reset_confirmation {
            ui.label(tr("Desk Switch will quit. Open it again to start a new workspace with your local preferences. You will need to approve each computer again."));
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.update_action_busy() && !self.group.monitor_busy(),
                        egui::Button::new(tr("Create workspace and quit")),
                    )
                    .clicked()
                {
                    let result = crate::config::request_workspace_reset(&self.dir);
                    self.group.reset_confirmation = false;
                    if let Err(error) = result {
                        self.notice = Some((format!("{error:#}"), true));
                    } else {
                        self.action(super::Action::Quit, ctx);
                    }
                }
                if ui.button(tr("Cancel")).clicked() {
                    self.group.reset_confirmation = false;
                }
            });
        }
    }

    pub(super) fn group_layout_dialog(&mut self, ctx: &egui::Context) {
        let Some(state) = self.group.workspace.as_ref() else {
            self.group.layout_editor.close();
            return;
        };
        let apply = self.group.layout_editor.show(ctx, state);
        if self.group.layout_editor.take_workspace_request() {
            self.page = Page::Overview;
        }
        if let Some(apply) = apply {
            self.group_action(Action::SetLayout {
                expected: apply.expected,
                layout: apply.layout,
            });
        }
    }
    fn group_application_settings(
        &mut self,
        ui: &mut egui::Ui,
        member: &Member,
        local: bool,
        ctx: &egui::Context,
    ) {
        let Some(management) = &member.node.management else {
            return;
        };
        let mut application = if local {
            self.local_application()
        } else {
            management.application.clone()
        };
        let before = application.clone();
        egui::Frame::group(ui.style())
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.strong(tr("Application"));
                egui::ComboBox::from_id_salt("group-theme")
                    .selected_text(tr(&application.theme))
                    .show_ui(ui, |ui| {
                        for theme in ["system", "dark", "light"] {
                            ui.selectable_value(&mut application.theme, theme.into(), tr(theme));
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
                if let Err(error) = self.apply_application(&application, ctx) {
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
    }
    fn group_update_settings(
        &mut self,
        ui: &mut egui::Ui,
        member: &Member,
        local: bool,
        ctx: &egui::Context,
    ) {
        let Some(management) = &member.node.management else {
            return;
        };
        egui::Frame::group(ui.style())
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
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
                if management.update.failed {
                    ui.colored_label(ui.visuals().error_fg_color, tr(&management.update.status));
                } else {
                    ui.label(tr(&management.update.status));
                }
                if let Some(progress) = &management.update.progress {
                    ui.add(
                        egui::ProgressBar::new(
                            progress.downloaded as f32 / progress.total.max(1) as f32,
                        )
                        .show_percentage(),
                    );
                }
                if self.group.update_target.as_ref() == Some(&member.node.key)
                    && let Some((status, failed)) = &self.peer_update_status
                {
                    ui.colored_label(
                        if *failed {
                            ui.visuals().error_fg_color
                        } else {
                            ui.visuals().text_color()
                        },
                        tr(status),
                    );
                }
                if !member.online {
                    ui.weak(tr("Offline"));
                }
                if management.update.prepared {
                    ui.label(tr("Update verified and ready to install."));
                }
                ui.weak(tr("When updating several computers, prepare them first. Install on the other computers before restarting this one."));
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            member.online
                                && !self.update_action_busy()
                                && !management.update.checking
                                && !management.update.installing,
                            egui::Button::new(tr("Check for updates")),
                        )
                        .clicked()
                    {
                        if local {
                            self.check_updates(ctx);
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
                            member.compatible()
                                && management.update.can_install()
                                && !management.update.prepared
                                && !self.update_action_busy(),
                            egui::Button::new(tr("Prepare update")),
                        )
                        .clicked()
                        && let Some(version) = management.update.version.clone()
                    {
                        if local {
                            self.prepare_update(version, ctx);
                        } else {
                            self.group_action(Action::Manage {
                                target: member.node.key.clone(),
                                message: workspace::management::Message::PrepareUpdate {
                                    id: super::management::request_id(),
                                    version,
                                },
                            });
                        }
                    }
                    if ui
                        .add_enabled(
                            member.online
                                && management.update.can_install()
                                && !self.update_action_busy(),
                            egui::Button::new(tr(if management.update.prepared {
                                "Install and restart"
                            } else {
                                "Download and install"
                            })),
                        )
                        .clicked()
                    {
                        self.install_selected_update(&member.node.key, ctx);
                    }
                });
            });
    }
    fn group_device_settings(&mut self, ui: &mut egui::Ui, member: &Member) {
        let mut preferences = member.preferences.clone();
        egui::Frame::group(ui.style())
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.strong(tr("Devices on this computer"));
                if !self.group.writable() {
                    ui.disable();
                }
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
                        .width(ui.available_width())
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
    }
    fn group_monitor_settings(
        &mut self,
        ui: &mut egui::Ui,
        member: &Member,
        local: bool,
        ctx: &egui::Context,
    ) {
        let mut monitor = member.node.monitor.clone();
        let mut read_monitor = false;
        egui::Frame::group(ui.style())
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.strong(tr("Shared monitor"));
                ui.checkbox(
                    &mut monitor.enabled,
                    tr("This computer uses the shared monitor"),
                );
                egui::ComboBox::from_id_salt("member-monitor")
                    .width(ui.available_width().min(480.0))
                    .selected_text(if monitor.name.is_empty() {
                        tr("Select a monitor")
                    } else {
                        monitor.name.clone()
                    })
                    .show_ui(ui, |ui| {
                        for display in &member.node.devices.displays {
                            if ui
                                .selectable_value(
                                    &mut monitor.id,
                                    display.id.clone(),
                                    &display.name,
                                )
                                .changed()
                            {
                                monitor.name = display.name.clone();
                                monitor.fingerprint = display.fingerprint.clone();
                                monitor.input = None;
                            }
                        }
                    });
                egui::ComboBox::from_id_salt("member-monitor-input")
                    .selected_text(
                        monitor
                            .input
                            .map(|input| match input {
                                15 => "DisplayPort".into(),
                                16 => "DisplayPort 2".into(),
                                17 => "HDMI 1".into(),
                                18 => "HDMI 2".into(),
                                3 => "DVI 1".into(),
                                other => format!("0x{other:02x}"),
                            })
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
                if ui
                    .add_enabled(
                        member.online && !monitor.id.is_empty(),
                        egui::Button::new(tr("Read the current input once")),
                    )
                    .clicked()
                {
                    read_monitor = true;
                }
            });
        let monitor_id = monitor.id.clone();
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
        if read_monitor {
            if local {
                if let Err(error) = self.start_monitor_read(&monitor_id, ctx) {
                    self.notice = Some((error.to_string(), true));
                }
            } else {
                self.group_action(Action::Manage {
                    target: member.node.key.clone(),
                    message: workspace::management::Message::ReadMonitor {
                        id: super::management::request_id(),
                        monitor: monitor_id,
                    },
                });
            }
        }
    }
}

fn sharing_toggles(
    ui: &mut egui::Ui,
    settings: &mut model::Settings,
    status: Option<&str>,
    visual: &super::view::WorkspaceVisual,
    layout_supported: bool,
) -> bool {
    use super::view::{DeviceIcon, device_checkbox};
    ui.horizontal_wrapped(|ui| {
        for (icon, value, label) in [
            (
                DeviceIcon::Keyboard,
                &mut settings.keyboard_enabled,
                "Keyboard",
            ),
            (DeviceIcon::Mouse, &mut settings.mouse_enabled, "Mouse"),
            (DeviceIcon::Headphones, &mut settings.audio_enabled, "Sound"),
            (
                DeviceIcon::Microphone,
                &mut settings.microphone_enabled,
                "Microphone",
            ),
        ] {
            device_checkbox(
                ui,
                icon,
                value,
                label,
                visual.devices[icon.index()].as_ref(),
                true,
            );
        }
    });
    ui.checkbox(&mut settings.clipboard_enabled, tr("Shared clipboard"));
    ui.weak(tr("Text, images, files and folders."));
    if settings.clipboard_enabled {
        ui.weak(tr("Copy or cut on one computer, paste on another."));
        #[cfg(target_os = "macos")]
        ui.weak(tr("Use Send files to receive files on this Mac."));
    }
    if let Some(status) = status {
        ui.weak(tr(status));
    }
    let mut arrange = false;
    ui.horizontal_wrapped(|ui| {
        let edge = ui.checkbox(
            &mut settings.edge,
            tr("Move between computers at the screen edge"),
        );
        let button = ui
            .add_enabled(layout_supported, egui::Button::new(tr("Arrange displays…")))
            .on_disabled_hover_text(tr("Reconnect the workspace to arrange displays."));
        arrange = (edge.changed() && settings.edge && layout_supported) || button.clicked();
    });
    ui.checkbox(
        &mut settings.monitor_enabled,
        tr("Switch the shared monitor"),
    );
    if settings.microphone_enabled {
        egui::CollapsingHeader::new(tr("Microphone level")).show(ui, |ui| {
            ui.add(egui::Slider::new(&mut settings.microphone_level.volume, 0..=100).suffix(" %"));
            ui.checkbox(&mut settings.microphone_level.muted, tr("Mute microphone"));
        });
    }
    arrange
}

fn edge_settings(ui: &mut egui::Ui, settings: &mut model::Settings) {
    ui.horizontal_wrapped(|ui| {
        ui.label(tr("Edge delay"));
        egui::ComboBox::from_id_salt("workspace-edge-delay")
            .width(140.0)
            .selected_text(if settings.edge_delay_ms == 0 {
                tr("No delay")
            } else {
                format!("{} ms", settings.edge_delay_ms)
            })
            .show_ui(ui, |ui| {
                for delay in [0, 150, 300, 500] {
                    ui.selectable_value(
                        &mut settings.edge_delay_ms,
                        delay,
                        if delay == 0 {
                            tr("No delay")
                        } else {
                            format!("{delay} ms")
                        },
                    );
                }
            })
            .response
            .on_hover_text(tr(
                "Keep pushing against the screen edge for the selected delay. Moving away cancels the switch.",
            ));
        ui.checkbox(
            &mut settings.edge_protect_corners,
            tr("Protect screen corners"),
        )
        .on_hover_text(tr("Avoid switching near the corners of a screen."));
    });
}

fn sharing_sources(ui: &mut egui::Ui, settings: &mut model::Settings, state: &Workspace) {
    selection(
        ui,
        "group-input-source",
        "Keyboard & mouse from",
        &mut settings.input,
        state,
    );
    selection(
        ui,
        "group-audio-output",
        "Play all sound on",
        &mut settings.audio,
        state,
    );
    selection(
        ui,
        "group-microphone-source",
        "Microphone from",
        &mut settings.microphone,
        state,
    );
}

pub(super) fn selection(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    value: &mut Selection,
    state: &Workspace,
) {
    ui.vertical(|ui| {
        ui.label(tr(label));
        let caption = match value {
            Selection::Auto => tr("Automatic"),
            Selection::Computer(key) => state
                .members
                .get(key)
                .map(|member| member.node.name.clone())
                .unwrap_or_else(|| tr("Unavailable")),
        };
        egui::ComboBox::from_id_salt(id)
            .width(ui.available_width().min(320.0))
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

fn shared_diagnostics(state: &Workspace, own: &str, local: &Node) -> String {
    use std::fmt::Write;
    let mut report = format!(
        "Desk Switch {} / protocol {}\nWorkspace protocol: {}\n",
        crate::compatibility::VERSION,
        crate::compatibility::PROTOCOL,
        model::PROTOCOL,
    );
    for (index, (key, member)) in state.members.iter().enumerate() {
        let node = if key == own { local } else { &member.node };
        let platform = match node.diagnostics.platform.as_str() {
            "windows" => "Windows",
            "linux" => "Ubuntu",
            "macos" => "macOS",
            _ => "Unknown",
        };
        let version = node
            .version
            .as_ref()
            .and_then(|version| semver::Version::parse(&version.app).ok())
            .map(|version| format!("{}.{}.{}", version.major, version.minor, version.patch))
            .unwrap_or_else(|| "unknown".into());
        let _ = writeln!(
            report,
            "Computer {} ({platform}, {version}): online={}, input error={}, audio error={}, microphone error={}, monitor error={}",
            index + 1,
            member.online,
            node.input_error.is_some(),
            node.audio_error.is_some(),
            node.microphone_error.is_some(),
            node.monitor_error.is_some(),
        );
    }
    report
}

impl Desktop {
    pub(super) fn group_diagnostics(&self, ui: &mut egui::Ui) {
        let Some(state) = &self.group.workspace else {
            return;
        };
        let mut local = self.local_node();
        self.group.data.report(&mut local);
        self.group.report_network(&mut local);
        self.report_group_monitor_history(&mut local);
        local.monitor_error = self.group.monitor_error.clone();
        if let Some(error) = &self.group.data.error {
            local.diagnostics.line("Workspace", error);
        }
        if let Some(error) = self
            .group
            .clipboard
            .as_ref()
            .and_then(|value| value.status())
        {
            local.diagnostics.line("Clipboard", &error);
        }

        ui.weak(tr(
            "Recent service messages from every computer in this workspace.",
        ));
        ui.add_space(12.0);
        if ui.button(tr("Copy diagnostics")).clicked() {
            ui.ctx()
                .copy_text(shared_diagnostics(state, &self.public_key, &local));
        }
        if ui.button(tr("Copy full diagnostics")).clicked() {
            // Workspace and Node contain public connection/settings state, never
            // the private identity. Refresh local worker output at copy time.
            let mut snapshot = state.clone();
            if let Some(member) = snapshot.members.get_mut(&self.public_key) {
                member.node = local.clone();
            }
            let report = format!(
                "Desk Switch {} / protocol {}\nWorkspace protocol: {}\nLocal computer: {}\nCurrent message: {}\n{}\n",
                crate::compatibility::VERSION,
                crate::compatibility::PROTOCOL,
                model::PROTOCOL,
                self.public_key,
                self.notice
                    .as_ref()
                    .map(|(message, _)| message.as_str())
                    .unwrap_or(""),
                serde_json::to_string_pretty(&snapshot).unwrap_or_default()
            );
            ui.ctx().copy_text(report);
        }
        ui.weak(tr(
            "Full diagnostics include computer names, network addresses, settings and recent logs.",
        ));
        ui.add_space(12.0);

        for (key, member) in &state.members {
            let is_local = key == &self.public_key;
            let node = if is_local { &local } else { &member.node };
            let errors = [
                ("Keyboard & mouse", &node.input_error),
                ("Shared monitor", &node.monitor_error),
                ("Sound", &node.audio_error),
                ("Microphone", &node.microphone_error),
            ];
            let has_errors = errors.iter().any(|(_, error)| error.is_some());
            let platform = match node.diagnostics.platform.as_str() {
                "windows" => "Windows",
                "linux" => "Ubuntu",
                "macos" => "macOS",
                _ => "—",
            };
            let version = node
                .version
                .as_ref()
                .map(|value| value.app.as_str())
                .unwrap_or("—");
            let header = format!(
                "{} · {platform} · {version} · {}",
                node.name,
                tr(if member.online {
                    "Connected"
                } else {
                    "Offline"
                })
            );
            egui::Frame::group(ui.style())
                .fill(ui.visuals().window_fill())
                .corner_radius(12)
                .inner_margin(16)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    egui::CollapsingHeader::new(header)
                        .id_salt(("group-computer-diagnostics", key))
                        .default_open(is_local || has_errors)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                if is_local {
                                    ui.weak(tr("This PC"));
                                }
                                if let Some(version) = &node.version {
                                    ui.weak(format!("{} {}", tr("Protocol:"), version.protocol));
                                }
                            });
                            if !member.online {
                                ui.weak(tr("Offline: showing the last received diagnostics."));
                            }
                            for (name, error) in errors {
                                if let Some(error) = error {
                                    ui.colored_label(
                                        ui.visuals().error_fg_color,
                                        format!("{}: {}", tr(name), tr(error)),
                                    );
                                }
                            }
                            if node.diagnostics.logs.is_empty() {
                                ui.weak(tr("No recent service messages."));
                            } else {
                                egui::ScrollArea::vertical()
                                    .id_salt(("group-computer-service-logs", key))
                                    .max_height(230.0)
                                    .stick_to_bottom(true)
                                    .show(ui, |ui| {
                                        for line in &node.diagnostics.logs {
                                            ui.label(
                                                RichText::new(tr(line)).monospace().size(12.0),
                                            );
                                        }
                                    });
                            }
                        });
                });
            ui.add_space(8.0);
        }
    }
}

fn manual_address(value: &str) -> Result<SocketAddr> {
    use std::net::{IpAddr, Ipv4Addr};
    let value = value.trim();
    anyhow::ensure!(value.len() <= 128, "IP address is too long");
    let address = value
        .parse::<IpAddr>()
        .map(|ip| SocketAddr::new(ip, model::PORT))
        .or_else(|_| value.parse::<SocketAddr>())
        .context("Enter an IP address, optionally followed by :port")?;
    let address = match address {
        SocketAddr::V6(value) => value
            .ip()
            .to_ipv4_mapped()
            .map(|ip| SocketAddr::new(ip.into(), value.port()))
            .unwrap_or(address),
        _ => address,
    };
    anyhow::ensure!(
        address.port() != 0
            && !address.ip().is_unspecified()
            && !address.ip().is_multicast()
            && address.ip() != IpAddr::V4(Ipv4Addr::BROADCAST),
        "Enter the other computer's unicast IP address"
    );
    Ok(address)
}

#[cfg(test)]
mod manual_tests {
    use super::*;
    #[test]
    fn input_recovery_clears_only_its_owned_notice_and_never_an_update_or_monitor_error() {
        let old = "peer heartbeat timed out (last reply 1761 ms ago)".to_owned();
        let mut notice = Some((old.clone(), true));
        clear_recovered_input_notice(&mut notice, &mut Some(old.clone()));
        assert!(notice.is_none());
        for message in ["Monitor switch failed", "Could not download the update"] {
            let mut notice = Some((message.to_owned(), true));
            clear_recovered_input_notice(&mut notice, &mut Some(old.clone()));
            assert_eq!(notice, Some((message.to_owned(), true)));
        }
    }
    #[test]
    fn manual_connection_addresses_keep_ports_and_normalize_mapped_ipv4() {
        assert_eq!(
            manual_address(" 192.0.2.42 ").unwrap().to_string(),
            "192.0.2.42:45880"
        );
        assert_eq!(manual_address("192.0.2.42:1234").unwrap().port(), 1234);
        assert_eq!(manual_address("fd00::20").unwrap().port(), model::PORT);
        assert_eq!(manual_address("[fd00::20]:1234").unwrap().port(), 1234);
        assert_eq!(
            manual_address("::ffff:192.0.2.42").unwrap().to_string(),
            "192.0.2.42:45880"
        );
        for value in [
            "",
            "host.example",
            "https://192.0.2.42",
            "0.0.0.0",
            "::",
            "224.0.0.1",
            "255.255.255.255",
            "192.0.2.42:0",
        ] {
            assert!(manual_address(value).is_err(), "{value}");
        }
    }
}
