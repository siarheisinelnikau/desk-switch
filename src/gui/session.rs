use super::{
    Desktop, QueryKind,
    worker::{Kind, Worker},
};
use crate::{
    config, discovery,
    workspace::{
        self, Hardware, Node, Plan,
        hardware::Probe,
        net::{Event, Request, Service},
    },
};
use anyhow::{Context, Result, ensure};
use eframe::egui;
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Pair {
    pub(super) service: Option<Service>,
    probe: Option<Probe>,
    pub remote: Option<Node>,
    pub last_remote: Option<Node>,
    pub hardware: Hardware,
    pub(super) inputs: Vec<workspace::hardware::InputEndpoint>,
    pub devices: workspace::devices::Inventory,
    pub microphone_failed: bool,
    microphone_profile: String,
    microphone_capture: Option<String>,
    microphone_choice: Option<String>,
    pub plan: Option<Plan>,
    active_plan: Option<Plan>,
    pub synced: bool,
    pub request: Option<Request>,
    pub outgoing: Option<Request>,
    pub busy: bool,
    auto_suspended: bool,
    pub waiting_since: Option<Instant>,
    pub status: String,
    remote_address: Option<SocketAddr>,
    last_dial: Option<Instant>,
    reconnect: Reconnect,
    input_profile: String,
    monitor_profile: String,
    monitor_setup_pending: bool,
    audio_profile: String,
    pub input_failed: bool,
    pub monitor_failed: bool,
    pub audio_failed: bool,
    #[cfg(target_os = "linux")]
    pub input_access_attempts: std::collections::BTreeSet<String>,
    scanned: bool,
    pub monitor_input: Option<u16>,
}

// The data services have their own authenticated reconnects and a short input
// lease. Losing the settings channel must not destroy their OS permission.
const RECONNECT_GRACE: Duration = Duration::from_secs(20);

#[derive(Default)]
struct Reconnect(Option<Instant>);

impl Reconnect {
    fn lost(&mut self, now: Instant) {
        self.0.get_or_insert(now);
    }
    fn restored(&mut self) {
        self.0 = None;
    }
    fn expired(&self, now: Instant) -> bool {
        self.0
            .is_some_and(|since| now.duration_since(since) >= RECONNECT_GRACE)
    }
}

impl Pair {
    fn connection_restored(&mut self, address: SocketAddr) {
        self.remote_address = Some(address);
        self.request = None;
        self.outgoing = None;
        self.busy = false;
        self.waiting_since = None;
        self.status.clear();
        // Device failures, refused consent and service profiles survive a
        // settings reconnect. Only Retry or a changed device plan resets them.
    }

    fn connection_lost(&mut self, error: String, now: Instant) {
        if let Some(remote) = self.remote.take() {
            self.last_remote = Some(remote);
        }
        self.plan = None;
        self.reconnect.lost(now);
        self.synced = false;
        self.monitor_input = None;
        self.request = None;
        self.outgoing = None;
        self.busy = false;
        self.waiting_since = None;
        self.status = error;
        self.last_dial = Some(now);
    }
}

impl Desktop {
    #[cfg(target_os = "linux")]
    pub(super) fn input_permission_paths(&self) -> Vec<String> {
        let choices = self.prefs.workspace.shared.devices_for(&self.public_key);
        self.pair
            .inputs
            .iter()
            .filter(|d| {
                d.keyboard && choices.keyboard.as_ref().is_none_or(|id| id == &d.info.id)
                    || d.mouse && choices.mouse.as_ref().is_none_or(|id| id == &d.info.id)
            })
            .take(16)
            .map(|d| d.path.to_string_lossy().into_owned())
            .collect()
    }

    pub(super) fn local_node(&self) -> Node {
        #[cfg(debug_assertions)]
        if let Some(node) = &self.preview {
            return node.clone();
        }
        let mut node = Node {
            key: self.public_key.clone(),
            name: discovery::computer_name(),
            version: Some(crate::compatibility::Version::current()),
            management: Some(self.local_management()),
            hardware: {
                let mut choices = self.prefs.workspace.shared.devices_for(&self.public_key);
                if let Some(binding) = &self.prefs.workspace.shared.microphone_binding
                    && binding.computer == self.public_key
                {
                    choices.microphone = Some(binding.device.clone());
                }
                workspace::hardware::hardware(&self.pair.devices, &choices, self.pair.scanned)
            },
            devices: workspace::devices::Inventory {
                individual_input: cfg!(target_os = "linux"),
                ..self.pair.devices.clone()
            },
            monitor: self.prefs.workspace.monitor.clone(),
            shared: self.prefs.workspace.shared.clone(),
            input: self
                .input
                .as_ref()
                .filter(|w| w.connected())
                .and_then(|w| w.input_status),
            monitor_input: match self
                .input
                .as_ref()
                .filter(|w| w.running() && !w.stopping && w.monitoring)
            {
                Some(worker) => worker.monitor_input,
                None => match self.monitor.as_ref().filter(|w| w.running() && !w.stopping) {
                    Some(worker) => worker.monitor_input,
                    None => self.pair.monitor_input,
                },
            },
            audio_playing: self
                .audio
                .as_ref()
                .is_some_and(|w| w.running() && w.audio_playing),
            monitor_error: self.monitor.as_ref().and_then(|w| {
                w.switch_error.clone().or_else(|| {
                    (w.exit_ok == Some(false))
                        .then(|| w.logs.back().cloned())
                        .flatten()
                })
            }),
            input_error: self.input.as_ref().and_then(|w| {
                w.switch_error.clone().or_else(|| {
                    (w.exit_ok == Some(false))
                        .then(|| w.logs.back().cloned())
                        .flatten()
                })
            }),
            audio_error: self
                .audio
                .as_ref()
                .filter(|w| w.exit_ok == Some(false))
                .and_then(|w| w.logs.back().cloned()),
            microphone_active: self
                .microphone
                .as_ref()
                .is_some_and(|w| w.running() && !w.stopping && w.audio_playing),
            microphone_error: self
                .microphone
                .as_ref()
                .filter(|w| w.exit_ok == Some(false))
                .and_then(|w| w.logs.back().cloned()),
            ack: String::new(),
            diagnostics: self.diagnostics_snapshot(),
        };
        for error in [
            &mut node.input_error,
            &mut node.audio_error,
            &mut node.microphone_error,
            &mut node.monitor_error,
        ]
        .into_iter()
        .flatten()
        {
            error.truncate(error.floor_char_boundary(2048));
        }
        if let Some(remote) = &self.pair.remote {
            node.ack = Plan::build(&node, remote).digest(&node, remote);
        }
        node
    }

    fn diagnostics_snapshot(&self) -> workspace::diagnostics::Diagnostics {
        let mut report = workspace::diagnostics::Diagnostics {
            platform: std::env::consts::OS.into(),
            ..Default::default()
        };
        report.line("Update", &self.update_status);
        report.line("Pair", &self.pair.status);
        for (name, worker) in [
            ("Input", &self.input),
            ("Monitor", &self.monitor),
            ("Audio", &self.audio),
            ("Microphone", &self.microphone),
        ] {
            if let Some(worker) = worker {
                for line in worker.logs.iter().rev().take(9).rev() {
                    report.line(name, line);
                }
            }
        }
        report
    }

    pub(super) fn availability_changed(&mut self, ctx: &egui::Context) {
        self.pair.auto_suspended = false;
        if let Err(error) = self.prefs.save(&self.dir) {
            self.notice = Some((error.to_string(), true));
        }
        if !self.prefs.discovery {
            self.stop_pair();
        }
        self.sync_pair(ctx);
    }

    pub(super) fn stop_pair(&mut self) {
        if let Some(service) = &self.pair.service {
            service.disconnect();
        }
        self.pair.service.take();
        self.pair.probe.take();
        self.discovery.take();
        self.discovered.clear();
        self.pair.remote = None;
        self.pair.plan = None;
        self.pair.active_plan = None;
        self.pair.reconnect.restored();
        self.pair.synced = false;
        self.pair.monitor_input = None;
        self.pair.request = None;
        self.pair.outgoing = None;
        self.pair.busy = false;
        self.pair.waiting_since = None;
        self.pair.scanned = false;
        self.pair.input_profile.clear();
        self.pair.monitor_profile.clear();
        self.pair.monitor_setup_pending = false;
        self.pair.audio_profile.clear();
        for worker in [
            &mut self.input,
            &mut self.monitor,
            &mut self.audio,
            &mut self.microphone,
        ]
        .into_iter()
        .flatten()
        {
            worker.stop();
        }
    }

    fn sync_pair(&mut self, ctx: &egui::Context) {
        #[cfg(debug_assertions)]
        if self.preview.is_some() {
            return;
        }
        if !self.prefs.discovery
            || self.quitting
            || self.install_pending
            || self.install_job.is_some()
            || self.paired_update.as_ref().is_some_and(|u| u.handed_off)
        {
            return;
        }
        if self.pair.service.is_none() && self.discovery_error.is_none() {
            let result: Result<()> = (|| {
                let identity = if self.dir.join("identity").exists() {
                    config::load_identity(&self.dir)?
                } else {
                    config::create_identity(&self.dir)?
                };
                self.public_key = config::hex(&identity.public);
                self.pair.service = Some(Service::start(
                    self.dir.clone(),
                    self.local_node(),
                    workspace::PORT,
                    ctx.clone(),
                )?);
                self.pair.probe = Some(Probe::start(ctx.clone())?);
                Ok(())
            })();
            if let Err(error) = result {
                self.stop_pair();
                self.discovery_error = Some(format!("{error:#}"));
            }
        }
        if let Some(service) = &self.pair.service
            && self.discovery.is_none()
            && self.discovery_error.is_none()
        {
            match discovery::Discovery::start(
                &discovery::computer_name(),
                &self.public_key,
                service.port,
                ctx.clone(),
            ) {
                Ok(discovery) => self.discovery = Some(discovery),
                Err(error) => self.discovery_error = Some(format!("{error:#}")),
            }
        }
    }

    pub(super) fn tick_pair(&mut self, ctx: &egui::Context) {
        self.sync_pair(ctx);
        if let Some(snapshot) = self.pair.probe.as_ref().and_then(Probe::poll) {
            if let Some(error) = snapshot.error {
                self.pair.status = error;
            }
            self.pair.scanned = snapshot.hardware.known;
            self.pair.hardware = snapshot.hardware;
            self.pair.inputs = snapshot.inputs;
            self.pair.devices = snapshot.devices;
            self.monitors = snapshot.displays;
            let monitor = &mut self.prefs.workspace.monitor;
            if monitor.id.is_empty() && self.monitors.len() == 1 {
                let found = &self.monitors[0];
                monitor.id = found.id.clone();
                monitor.name = found.name.clone();
                monitor.fingerprint = found.fingerprint.clone();
                if monitor.enabled {
                    self.remember_monitor();
                }
            }
        }
        if self.pair.monitor_setup_pending {
            for worker in [&mut self.input, &mut self.monitor].into_iter().flatten() {
                worker.stop();
            }
            if self.query.is_none() && !self.input_running() && !self.monitor_running() {
                self.pair.monitor_setup_pending = false;
                self.scan(QueryKind::Monitors);
            }
        }
        let discovery_events = self
            .discovery
            .as_ref()
            .map(|d| d.poll().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in discovery_events {
            match event {
                discovery::Event::Found(peer)
                    if self.discovered.len() < 32 || self.discovered.contains_key(&peer.id) =>
                {
                    self.discovered.insert(peer.id.clone(), peer);
                }
                discovery::Event::Removed(id) => {
                    self.discovered.remove(&id);
                }
                discovery::Event::Error(error) => self.discovery_error = Some(error),
                _ => {}
            }
        }
        let events = self
            .pair
            .service
            .as_ref()
            .map(|s| s.poll().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Request(request) => {
                    self.page = super::Page::Overview;
                    self.pair.busy = true;
                    self.pair.request = Some(request);
                    self.pair.waiting_since = Some(Instant::now());
                    self.pair.status = String::new();
                    self.action(super::Action::Show, ctx);
                }
                Event::Pairing(request) => {
                    self.pair.busy = true;
                    self.pair.outgoing = Some(request);
                    self.pair.waiting_since.get_or_insert_with(Instant::now);
                }
                Event::Connected { key, name, address } => {
                    self.peer_key = key;
                    self.prefs.workspace.peer_name = name;
                    self.prefs.workspace.last_address = address.to_string();
                    self.pair.connection_restored(address);
                    if let Some(message) = self
                        .paired_update
                        .as_ref()
                        .and_then(workspace::update::Coordinator::retry)
                    {
                        self.apply_update_effects(
                            vec![workspace::update::Effect::Send(message)],
                            ctx,
                        );
                    }
                    let _ = self.prefs.save(&self.dir);
                }
                Event::State(remote) => {
                    self.pair.status.clear();
                    if self.prefs.workspace.shared.merge(&remote.shared) {
                        let _ = self.prefs.save(&self.dir);
                    }
                    self.pair.remote = Some(*remote);
                    self.apply_input_monitor_requirement();
                }
                Event::Update(message) => self.receive_update(message, ctx),
                Event::UpdateSent(message) => self.sent_update(&message, ctx),
                Event::Management(message) => self.receive_management(message, ctx),
                Event::Forgotten => self.apply_forget(),
                Event::Closed(error) => {
                    self.pair.connection_lost(error, Instant::now());
                }
            }
        }
        let node = self.local_node();
        if let Some(remote) = &self.pair.remote {
            let keyboard = Plan::can_disable(&node, remote, true);
            let mouse = Plan::can_disable(&node, remote, false);
            let shared = &mut self.prefs.workspace.shared;
            if (!keyboard && !shared.keyboard_enabled) || (!mouse && !shared.mouse_enabled) {
                shared.keyboard_enabled |= !keyboard;
                shared.mouse_enabled |= !mouse;
                self.shared_changed();
            }
        }
        let node = self.local_node();
        let shared = &self.prefs.workspace.shared;
        if shared.microphone_enabled
            && !shared.microphone_level.muted
            && shared.microphone_binding.is_none()
            && let Some(remote) = &self.pair.remote
            && Plan::build(&node, remote).microphone_source.as_ref() == Some(&self.public_key)
        {
            let choices = shared.devices_for(&self.public_key);
            if let Some(device) = workspace::devices::selected(
                &self.pair.devices.microphones,
                choices.microphone.as_deref(),
            ) {
                self.prefs.workspace.shared.microphone_binding =
                    Some(workspace::devices::Binding {
                        computer: self.public_key.clone(),
                        device: device.id.clone(),
                    });
                self.shared_changed();
            }
        }
        let node = self.local_node();
        self.pair.plan = self.pair.remote.as_ref().map(|r| Plan::build(&node, r));
        if let Some(remote) = &self.pair.remote {
            let output = self
                .pair
                .plan
                .as_ref()
                .and_then(|p| p.audio_output.as_deref());
            if output.is_some()
                && self.prefs.workspace.shared.prepare_audio_levels(
                    &self.public_key,
                    &remote.key,
                    output,
                )
            {
                self.shared_changed();
            }
        }
        self.pair.synced = self.pair.scanned
            && self.pair.remote.as_ref().is_some_and(|r| {
                r.hardware.known
                    && !node.ack.is_empty()
                    && node.ack == r.ack
                    && crate::compatibility::Version::current()
                        .require(r.version.as_ref())
                        .is_ok()
            });
        if self
            .prefs
            .workspace
            .apply_pending_monitor(self.monitor_setup_is_local())
        {
            let _ = self.prefs.save(&self.dir);
            self.pair.synced = false;
        }
        if self.pair.synced {
            self.pair.reconnect.restored();
        } else if self.pair.reconnect.expired(Instant::now()) {
            for worker in [
                &mut self.input,
                &mut self.monitor,
                &mut self.audio,
                &mut self.microphone,
            ]
            .into_iter()
            .flatten()
            {
                worker.stop();
            }
            self.pair.active_plan = None;
        }
        if let Some(service) = &self.pair.service {
            service.publish(node);
        }
        if self.prefs.auto_connect
            && !self.pair.auto_suspended
            && self.pair.remote.is_none()
            && !self.pair.busy
            && !self.peer_key.is_empty()
            && self
                .pair
                .last_dial
                .is_none_or(|time| time.elapsed() > Duration::from_secs(4))
        {
            let found = self
                .discovered
                .values()
                .find(|p| p.public_key.as_ref() == Some(&self.peer_key))
                .cloned();
            if let Some(peer) = found.filter(discovery::Peer::manageable) {
                self.link_peer(&peer);
            } else if let Ok(address) = self.prefs.workspace.last_address.parse::<SocketAddr>()
                && let Some(service) = &self.pair.service
            {
                self.pair.last_dial = Some(Instant::now());
                if service.dial(vec![address], self.peer_key.clone()).is_ok() {
                    self.pair.busy = true;
                    self.pair.waiting_since = Some(Instant::now());
                }
            }
        }
        if self.prefs.discovery
            && self.prefs.auto_connect
            && !self.pair.auto_suspended
            && self.pair.remote.is_none()
            && !self.pair.busy
            && !self.peer_key.is_empty()
            && self.pair.service.is_some()
            && let Some(since) = self.pair.last_dial
        {
            ctx.request_repaint_after(
                Duration::from_secs(4)
                    .saturating_sub(since.elapsed())
                    .max(Duration::from_millis(100)),
            );
        }
        if let Some(since) = self.pair.reconnect.0 {
            let remaining = RECONNECT_GRACE.saturating_sub(since.elapsed());
            if !remaining.is_zero() {
                ctx.request_repaint_after(remaining);
            }
        }
        self.reconcile_services();
    }

    pub(super) fn link_peer(&mut self, peer: &discovery::Peer) {
        self.pair.auto_suspended = false;
        let result: Result<()> = (|| {
            ensure!(
                peer.manageable(),
                "Install a version with remote management on both PCs once to manage future updates from either computer."
            );
            let service = self
                .pair
                .service
                .as_ref()
                .context("Включите доступность в сети")?;
            let addresses = peer
                .addresses
                .iter()
                .map(|address| {
                    let mut address = *address;
                    address.set_port(peer.control_port.unwrap());
                    address
                })
                .collect();
            service.dial(addresses, peer.public_key.clone().unwrap())?;
            Ok(())
        })();
        self.pair.last_dial = Some(Instant::now());
        match result {
            Ok(()) => {
                self.pair.busy = true;
                self.pair.waiting_since = Some(Instant::now());
                self.pair.status = "Запрос отправлен. Подтвердите связь на другом ПК.".into();
            }
            Err(e) => self.pair.status = format!("{e:#}"),
        }
    }

    pub(super) fn link_address(&mut self) {
        self.pair.auto_suspended = false;
        let result: Result<()> = (|| {
            let address = workspace::devices::peer_address(&self.prefs.workspace.manual_address)?;
            self.pair
                .service
                .as_ref()
                .context("Turn on network availability on both PCs")?
                .dial_address(address)?;
            self.prefs.save(&self.dir)?;
            Ok(())
        })();
        self.pair.last_dial = Some(Instant::now());
        match result {
            Ok(()) => {
                self.pair.busy = true;
                self.pair.waiting_since = Some(Instant::now());
                self.pair.status = "Запрос отправлен. Подтвердите связь на другом ПК.".into();
            }
            Err(error) => self.pair.status = format!("{error:#}"),
        }
    }

    pub(super) fn cancel_pairing(&mut self, ctx: &egui::Context) {
        self.stop_pair();
        self.pair.last_dial = Some(Instant::now());
        self.pair.auto_suspended = true;
        self.pair.status = "Connection cancelled".into();
        self.sync_pair(ctx);
    }

    pub(super) fn approve_pair(&mut self, accept: bool) {
        if let Some(request) = self.pair.request.take()
            && let Some(service) = &self.pair.service
        {
            service.approve(&request.key, accept);
        }
    }

    pub(super) fn shared_changed(&mut self) {
        self.ctx.request_repaint();
        self.prefs.workspace.shared.changed(&self.public_key);
        self.apply_input_monitor_requirement();
        if let Err(error) = self.prefs.save(&self.dir) {
            self.notice = Some((error.to_string(), true));
        }
    }

    pub(super) fn forget_pair(&mut self) {
        if let Some(service) = &self.pair.service
            && self.pair.remote.is_some()
        {
            match service.forget() {
                Ok(()) => self.pair.status = "Разрываем пару…".into(),
                Err(error) => self.notice = Some((error.to_string(), true)),
            }
            return;
        }
        self.apply_forget();
    }

    fn apply_forget(&mut self) {
        self.stop_pair();
        self.pair.last_remote = None;
        self.paired_update = None;
        let result: Result<()> = (|| {
            if self.dir.join("peer").exists() {
                std::fs::remove_file(self.dir.join("peer"))?;
            }
            self.peer_key.clear();
            self.prefs.workspace.peer_name.clear();
            self.prefs.workspace.last_address.clear();
            self.prefs.workspace.shared = workspace::Shared::default();
            self.prefs.save(&self.dir)
        })();
        self.message(
            result,
            "Пара разорвана. Для новой связи потребуется подтверждение.",
        );
    }

    pub(super) fn remember_monitor(&mut self) {
        if !self.prefs.workspace.shared.monitor_enabled {
            return;
        }
        if !self.prefs.workspace.monitor.enabled {
            self.prefs.workspace.pending_monitor = None;
        }
        if self.prefs.workspace.monitor.enabled && !self.prefs.workspace.monitor.id.is_empty() {
            if !self.monitor_setup_is_local() && self.can_remember_monitor() {
                self.scan(QueryKind::Monitors);
                return;
            }
            self.prefs.workspace.monitor.input = None;
            self.pair.monitor_input = None;
            self.pair.monitor_setup_pending = true;
        }
        if let Err(e) = self.prefs.save(&self.dir) {
            self.notice = Some((e.to_string(), true));
        }
    }

    pub(super) fn monitor_setup_is_local(&self) -> bool {
        let local = self.local_node();
        self.pair.remote.as_ref().is_none_or(|remote| {
            !Plan::input_in_use(&local, remote)
                && Plan::can_disable(&local, remote, true)
                && Plan::can_disable(&local, remote, false)
        })
    }

    pub(super) fn can_remember_monitor(&self) -> bool {
        self.monitor_setup_is_local()
            || self.prefs.workspace.monitor.input.is_none()
                && !self.monitor_running()
                && self.input.as_ref().is_none_or(|w| !w.monitoring)
    }

    pub(super) fn retry_devices(&mut self) {
        if let Some((_, error)) = self.device_notice.take()
            && self
                .notice
                .as_ref()
                .is_some_and(|(message, failed)| *failed && message == &error)
        {
            self.notice = None;
        }
        self.pair.input_failed = false;
        self.pair.monitor_failed = false;
        self.pair.audio_failed = false;
        self.pair.microphone_failed = false;
        self.pair.microphone_capture = None;
    }

    #[cfg(target_os = "linux")]
    pub(super) fn request_input_access_if_needed(&mut self) {
        if self.quitting
            || self.install_pending
            || !self.pair.synced
            || !self.pair.input_failed
            || self.prefs.role != "host"
            || self.query.is_some()
            || self.input_permission_paths().is_empty()
        {
            return;
        }
        let permission_denied = self.input.as_ref().is_some_and(|worker| {
            worker.age() >= Duration::from_secs(2)
                && worker
                    .logs
                    .iter()
                    .any(|line| super::input_permission_error(line))
        });
        if permission_denied {
            // udev may finish restoring an existing grant just after the first
            // open failed. Retry that open before considering a new auth dialog.
            if self
                .input_permission_paths()
                .iter()
                .all(|path| std::fs::File::open(path).is_ok())
            {
                self.pair.input_failed = false;
            } else if !self
                .pair
                .input_access_attempts
                .contains(&self.input_access_selection())
            {
                self.scan(QueryKind::InputAccess);
            }
        }
    }

    fn reconcile_monitor(&mut self, plan: &Plan, remote: &Node) {
        let args = if plan.input_source.is_none()
            && Plan::monitor_only_available(&self.local_node(), remote)
        {
            Plan::monitor_inputs(&self.local_node(), remote).map(|inputs| {
                let host = self.public_key < remote.key;
                let mut address = self.pair.remote_address.expect("connected pair address");
                address.set_port(workspace::INPUT_PORT);
                if !host {
                    address.set_ip(if address.is_ipv4() {
                        std::net::Ipv4Addr::UNSPECIFIED.into()
                    } else {
                        std::net::Ipv6Addr::UNSPECIFIED.into()
                    });
                }
                vec![
                    "monitor-service".to_owned(),
                    self.prefs.workspace.monitor.id.clone(),
                    format!("{:X}", inputs.host),
                    format!("{:X}", inputs.peer),
                    if host { "host" } else { "receiver" }.into(),
                    address.to_string(),
                ]
            })
        } else {
            None
        };
        let profile = args
            .as_ref()
            .map(|args| format!("{args:?}"))
            .unwrap_or_default();
        if profile != self.pair.monitor_profile {
            if let Some(worker) = &mut self.monitor {
                worker.stop();
            }
            self.pair.monitor_profile = profile;
            self.pair.monitor_failed = false;
        }
        // Input shutdown includes serialized video recovery. Never open a
        // second DDC writer while it is still running or releasing devices.
        if !self.input_running()
            && !self.monitor_running()
            && !self.pair.monitor_failed
            && self.query.is_none()
            && let Some(args) = args
        {
            match Worker::start(Kind::Monitor, &args, &self.dir, &self.ctx) {
                Ok(mut worker) => {
                    worker.monitoring = true;
                    self.pair.monitor_input = None;
                    self.monitor = Some(worker);
                }
                Err(error) => {
                    self.pair.monitor_failed = true;
                    self.notice = Some((format!("{error:#}"), true));
                }
            }
        }
    }

    fn reconcile_services(&mut self) {
        if self.quitting
            || self.install_pending
            || self.install_job.is_some()
            || self.paired_update.as_ref().is_some_and(|u| u.handed_off)
            || !self.prefs.discovery
            || self.pair.monitor_setup_pending
            || self
                .query
                .as_ref()
                .is_some_and(|(kind, _)| matches!(kind, QueryKind::Monitors))
        {
            return;
        }
        let Some(plan) = self.pair.plan.clone() else {
            return;
        };
        let Some(remote) = self.pair.remote.clone() else {
            return;
        };
        if crate::compatibility::Version::current()
            .require(remote.version.as_ref())
            .is_err()
        {
            for worker in [
                &mut self.input,
                &mut self.monitor,
                &mut self.audio,
                &mut self.microphone,
            ]
            .into_iter()
            .flatten()
            {
                worker.stop();
            }
            self.pair.active_plan = None;
            return;
        }
        let Some(address) = self.pair.remote_address else {
            return;
        };
        // A capability/role change first releases the previous input owner.
        // Nothing new starts until both machines acknowledge the same plan.
        if !self.pair.synced {
            if let Some(worker) = &mut self.monitor {
                worker.stop();
            }
            if let Some(active) = &self.pair.active_plan {
                if (active.input_source != plan.input_source
                    || active.keyboard_enabled != plan.keyboard_enabled
                    || active.mouse_enabled != plan.mouse_enabled
                    || active.monitor != plan.monitor
                    || active.edge != plan.edge
                    || active.purple_on_left != plan.purple_on_left)
                    && let Some(worker) = &mut self.input
                {
                    worker.stop();
                }
                if active.microphone_source != plan.microphone_source
                    && let Some(worker) = &mut self.microphone
                {
                    worker.stop();
                }
                if active.audio_output != plan.audio_output
                    && let Some(worker) = &mut self.audio
                {
                    worker.stop();
                }
            }
            return;
        }
        self.pair.active_plan = Some(plan.clone());
        self.reconcile_microphone(&plan, address);
        let mut input_profile = String::new();
        if let Some(source) = plan.input_source.as_ref() {
            let host = source == &self.public_key;
            self.prefs.role = if host { "host" } else { "receiver" }.into();
            let mut endpoint = address;
            endpoint.set_port(workspace::INPUT_PORT);
            if !host {
                endpoint.set_ip(if address.is_ipv4() {
                    std::net::Ipv4Addr::UNSPECIFIED.into()
                } else {
                    std::net::Ipv6Addr::UNSPECIFIED.into()
                });
            }
            self.prefs.address = endpoint.to_string();
            self.form.monitor_enabled = plan.monitor.is_some();
            self.form.sharing = config::InputSharing {
                keyboard: plan.keyboard_enabled,
                mouse: plan.mouse_enabled,
            };
            self.form.monitor_id = self.prefs.workspace.monitor.id.clone();
            if let Some(inputs) = plan.monitor {
                self.form.host_code = format!("{:02X}", inputs.host);
                self.form.peer_code = format!("{:02X}", inputs.peer);
            }
            self.form.edge_mode = if plan.edge {
                if plan.monitor.is_some() {
                    "video"
                } else {
                    "input"
                }
            } else {
                "off"
            }
            .into();
            self.form.edge_screen = "auto".into();
            self.form.edge_side = if (self.public_key == plan.purple) == plan.purple_on_left {
                "right"
            } else {
                "left"
            }
            .into();
            let choices = self.prefs.workspace.shared.devices_for(&self.public_key);
            self.form.capture_devices = self
                .pair
                .inputs
                .iter()
                .filter(|d| {
                    d.keyboard && choices.keyboard.as_ref().is_none_or(|id| id == &d.info.id)
                        || d.mouse && choices.mouse.as_ref().is_none_or(|id| id == &d.info.id)
                })
                .map(|d| d.path.to_string_lossy())
                .collect::<Vec<_>>()
                .join("\n");
            self.form.capture_keyboard = choices.keyboard.and_then(|id| {
                self.pair
                    .inputs
                    .iter()
                    .find(|d| d.info.id == id)
                    .map(|d| d.path.clone())
            });
            self.form.capture_mouse = choices.mouse.and_then(|id| {
                self.pair
                    .inputs
                    .iter()
                    .find(|d| d.info.id == id)
                    .map(|d| d.path.clone())
            });
            input_profile = format!(
                "{}:{}:{}",
                self.prefs.role,
                self.prefs.address,
                toml::to_string(&self.form.document().unwrap_or_default()).unwrap_or_default()
            );
            #[cfg(target_os = "linux")]
            if host {
                // Use the generation captured with the hardware inventory. A
                // reused event path must not bind a stale selected identity to
                // a different device before the next hardware scan.
                for device in self.pair.inputs.iter().filter(|device| {
                    self.form
                        .capture_devices
                        .lines()
                        .any(|path| std::path::Path::new(path) == device.path)
                }) {
                    input_profile
                        .push_str(&format!("\n{}:{:?}", device.info.id, device.generation));
                }
            }
        }
        if input_profile != self.pair.input_profile {
            if let Some(worker) = &mut self.input {
                worker.stop();
            }
            self.pair.input_profile = input_profile.clone();
            self.pair.input_failed = false;
        }
        if !input_profile.is_empty()
            && self.monitor_running()
            && let Some(worker) = &mut self.monitor
        {
            worker.stop();
        }
        if !input_profile.is_empty()
            && !self.input_running()
            && !self.monitor_running()
            && !self.pair.input_failed
        {
            let result = self.start_input();
            if let Err(error) = result {
                self.pair.input_failed = true;
                let error = format!("{error:#}");
                self.device_notice = Some((Kind::Input, error.clone()));
                self.notice = Some((error, true));
            }
        }
        self.reconcile_monitor(&plan, &remote);
        let mut audio_profile = String::new();
        if let Some(output) = &plan.audio_output {
            let receive = output == &self.public_key;
            self.prefs.audio_mode = if receive { "receive" } else { "send" }.into();
            let mut endpoint = address;
            endpoint.set_port(workspace::AUDIO_PORT);
            if !receive {
                endpoint.set_ip(if address.is_ipv4() {
                    std::net::Ipv4Addr::UNSPECIFIED.into()
                } else {
                    std::net::Ipv6Addr::UNSPECIFIED.into()
                });
            }
            self.prefs.audio_address = endpoint.to_string();
            let shared = &self.prefs.workspace.shared;
            let choices = shared.devices_for(&self.public_key);
            let speaker = workspace::devices::selected(
                &self.pair.devices.speakers,
                choices.speaker.as_deref(),
            )
            .map(|d| d.id.clone())
            .unwrap_or_default();
            self.prefs.audio_output = speaker.clone();
            self.prefs.audio_source = speaker;
            let incoming = shared.audio_level(&remote.key);
            let native = shared.audio_level(&self.public_key);
            self.prefs.volume = incoming.volume;
            self.prefs.muted = incoming.muted;
            self.prefs.native_volume = native.volume;
            self.prefs.native_muted = native.muted;
            let source_choice = self.prefs.workspace.shared.devices_for(&remote.key);
            let output_device = format!(
                "{:?}:{:?}",
                workspace::devices::selected(
                    &self.pair.devices.speakers,
                    choices.speaker.as_deref()
                )
                .map(|d| &d.id),
                workspace::devices::selected(
                    &remote.devices.speakers,
                    source_choice.speaker.as_deref()
                )
                .map(|d| &d.id)
            );
            audio_profile = format!(
                "{}:{}:{}:{}:{output_device}",
                self.prefs.audio_mode,
                self.prefs.audio_address,
                self.prefs.audio_output,
                self.prefs.audio_source
            );
        }
        if audio_profile != self.pair.audio_profile {
            if let Some(worker) = &mut self.audio {
                worker.stop();
            }
            self.pair.audio_profile = audio_profile.clone();
            self.pair.audio_failed = false;
        }
        if !audio_profile.is_empty() {
            if !self.audio_running() && !self.pair.audio_failed {
                let result: Result<()> = (|| {
                    let args = self.prefs.audio_args()?;
                    self.audio = Some(Worker::start(
                        if self.prefs.audio_mode == "send" {
                            Kind::AudioSend
                        } else {
                            Kind::AudioReceive
                        },
                        &args,
                        &self.dir,
                        &self.ctx,
                    )?);
                    self.applied_mix.clear();
                    Ok(())
                })();
                if let Err(error) = result {
                    self.pair.audio_failed = true;
                    let error = format!("{error:#}");
                    self.device_notice = Some((
                        if self.prefs.audio_mode == "send" {
                            Kind::AudioSend
                        } else {
                            Kind::AudioReceive
                        },
                        error.clone(),
                    ));
                    self.notice = Some((error, true));
                }
            } else if self.prefs.audio_mode == "receive" && self.audio_running() {
                let shared = &self.prefs.workspace.shared;
                if let Some(worker) = &mut self.audio {
                    // Commands are small and only emitted when the shared mix changes.
                    let incoming = shared.audio_level(&remote.key);
                    let native = shared.audio_level(&self.public_key);
                    let mix = format!("{}:{}", incoming.volume, incoming.muted);
                    if self.applied_mix != mix {
                        let _ = worker.command(&format!("volume {}", incoming.volume));
                        let _ = worker.command(if incoming.muted { "mute" } else { "unmute" });
                        self.applied_mix = mix;
                    }
                    if let Err(error) = worker.set_output_level(native) {
                        self.notice = Some((format!("Changing system volume: {error:#}"), true));
                    }
                }
            }
        }
    }
}

impl Desktop {
    fn reconcile_microphone(&mut self, plan: &Plan, mut address: SocketAddr) {
        let choices = self.prefs.workspace.shared.devices_for(&self.public_key);
        if choices.microphone != self.pair.microphone_choice {
            self.pair.microphone_capture = None;
            self.pair.microphone_choice = choices.microphone.clone();
        }
        let mut args = Vec::new();
        if let Some(source) = &plan.microphone_source {
            let sending = source == &self.public_key;
            address.set_port(workspace::MICROPHONE_PORT);
            if sending {
                address.set_ip(if address.is_ipv4() {
                    std::net::Ipv4Addr::UNSPECIFIED.into()
                } else {
                    std::net::Ipv6Addr::UNSPECIFIED.into()
                });
            }
            args = vec![
                "microphone".into(),
                if sending { "serve" } else { "play" }.into(),
                address.to_string(),
            ];
            if sending {
                self.pair.microphone_capture = self
                    .prefs
                    .workspace
                    .shared
                    .microphone_binding
                    .as_ref()
                    .filter(|binding| binding.computer == self.public_key)
                    .map(|binding| binding.device.clone());
                if let Some(id) = &self.pair.microphone_capture {
                    args.extend(["--source".into(), id.clone()]);
                } else {
                    args.clear();
                }
            } else {
                if let Some(bridge) = workspace::devices::selected(
                    &self.pair.devices.microphone_bridges,
                    choices.microphone_bridge.as_deref(),
                ) {
                    args.extend(["--output".into(), bridge.id.clone()]);
                }
                args.extend([
                    "--volume".into(),
                    self.prefs
                        .workspace
                        .shared
                        .microphone_level
                        .volume
                        .to_string(),
                ]);
            }
        }
        let profile = serde_json::to_string(if args.get(1).is_some_and(|v| v == "play") {
            &args[..args.len() - 2]
        } else {
            &args
        })
        .unwrap();
        if profile != self.pair.microphone_profile {
            if let Some(worker) = &mut self.microphone {
                worker.stop();
            }
            self.pair.microphone_profile = profile;
            self.pair.microphone_failed = false;
        }
        if !args.is_empty() && !self.microphone_running() && !self.pair.microphone_failed {
            let sending = plan.microphone_source.as_ref() == Some(&self.public_key);
            match Worker::start(
                if sending {
                    Kind::MicrophoneSend
                } else {
                    Kind::MicrophoneReceive
                },
                &args,
                &self.dir,
                &self.ctx,
            ) {
                Ok(worker) => {
                    self.microphone = Some(worker);
                    self.applied_microphone_volume = None;
                }
                Err(error) => {
                    self.pair.microphone_failed = true;
                    let error = format!("{error:#}");
                    self.device_notice = Some((
                        if sending {
                            Kind::MicrophoneSend
                        } else {
                            Kind::MicrophoneReceive
                        },
                        error.clone(),
                    ));
                    self.notice = Some((error, true));
                }
            }
        }
        if args.get(1).is_some_and(|v| v == "play") && self.microphone_running() {
            let volume = self.prefs.workspace.shared.microphone_level.volume;
            if self.applied_microphone_volume != Some(volume)
                && let Some(worker) = &mut self.microphone
                && worker.command(&format!("volume {volume}")).is_ok()
            {
                self.applied_microphone_volume = Some(volume);
            }
        }
    }
}

#[cfg(test)]
mod reconnect_tests {
    use super::*;

    #[test]
    fn settings_reconnect_does_not_retry_denied_devices_or_invalidate_running_profiles() {
        let mut pair = Pair {
            input_failed: true,
            audio_failed: true,
            microphone_failed: true,
            input_profile: "approved-input-session".into(),
            audio_profile: "running-audio".into(),
            microphone_profile: "denied-microphone".into(),
            ..Default::default()
        };
        #[cfg(target_os = "linux")]
        pair.input_access_attempts
            .insert("already-denied-selection".into());
        pair.connection_lost("temporary disconnect".into(), Instant::now());
        pair.connection_restored("127.0.0.1:45878".parse().unwrap());
        assert!(pair.input_failed && pair.audio_failed && pair.microphone_failed);
        assert_eq!(pair.input_profile, "approved-input-session");
        assert_eq!(pair.audio_profile, "running-audio");
        assert_eq!(pair.microphone_profile, "denied-microphone");
        #[cfg(target_os = "linux")]
        assert!(
            pair.input_access_attempts
                .contains("already-denied-selection")
        );
    }

    #[test]
    fn reconnect_retains_permission_until_grace_expires_without_extending_on_dial_failures() {
        let now = Instant::now();
        let mut reconnect = Reconnect::default();
        assert!(!reconnect.expired(now));
        reconnect.lost(now);
        assert!(!reconnect.expired(now + Duration::from_secs(6)));
        reconnect.lost(now + Duration::from_secs(16));
        assert!(reconnect.expired(now + RECONNECT_GRACE));
        reconnect.restored();
        assert!(!reconnect.expired(now + Duration::from_secs(60)));
    }
}
