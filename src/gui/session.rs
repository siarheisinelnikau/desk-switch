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
    service: Option<Service>,
    probe: Option<Probe>,
    pub remote: Option<Node>,
    pub hardware: Hardware,
    paths: Vec<std::path::PathBuf>,
    pub plan: Option<Plan>,
    active_plan: Option<Plan>,
    pub synced: bool,
    pub request: Option<Request>,
    pub outgoing: Option<Request>,
    pub busy: bool,
    pub status: String,
    remote_address: Option<SocketAddr>,
    last_dial: Option<Instant>,
    input_profile: String,
    audio_profile: String,
    pub input_failed: bool,
    pub audio_failed: bool,
    scanned: bool,
}

impl Desktop {
    pub(super) fn local_node(&self) -> Node {
        #[cfg(debug_assertions)]
        if let Some(node) = &self.preview {
            return node.clone();
        }
        let mut node = Node {
            key: self.public_key.clone(),
            name: discovery::computer_name(),
            hardware: self.pair.hardware.clone(),
            monitor: self.prefs.workspace.monitor.clone(),
            shared: self.prefs.workspace.shared.clone(),
            input: self
                .input
                .as_ref()
                .filter(|w| w.connected())
                .and_then(|w| w.input_status),
            audio_playing: self
                .audio
                .as_ref()
                .is_some_and(|w| w.running() && w.audio_playing),
            input_error: self
                .input
                .as_ref()
                .filter(|w| w.exit_ok == Some(false))
                .and_then(|w| w.logs.back().cloned()),
            audio_error: self
                .audio
                .as_ref()
                .filter(|w| w.exit_ok == Some(false))
                .and_then(|w| w.logs.back().cloned()),
            ack: String::new(),
        };
        for error in [&mut node.input_error, &mut node.audio_error]
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

    pub(super) fn availability_changed(&mut self, ctx: &egui::Context) {
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
        self.pair.synced = false;
        self.pair.request = None;
        self.pair.outgoing = None;
        self.pair.busy = false;
        self.pair.scanned = false;
        self.pair.input_profile.clear();
        self.pair.audio_profile.clear();
        for worker in [&mut self.input, &mut self.audio].into_iter().flatten() {
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
                self.discovery = Some(discovery::Discovery::start(
                    &discovery::computer_name(),
                    &self.public_key,
                    self.pair.service.as_ref().unwrap().port,
                    ctx.clone(),
                )?);
                Ok(())
            })();
            if let Err(error) = result {
                self.stop_pair();
                self.discovery_error = Some(format!("{error:#}"));
            }
        }
    }

    pub(super) fn tick_pair(&mut self, ctx: &egui::Context) {
        self.sync_pair(ctx);
        if let Some(snapshot) = self.pair.probe.as_ref().and_then(Probe::poll) {
            if let Some(error) = snapshot.error {
                self.pair.status = error;
            } else {
                self.pair.hardware = snapshot.hardware;
                self.pair.paths = snapshot.paths;
                self.pair.scanned = true;
            }
            self.monitors = snapshot.displays;
            let monitor = &mut self.prefs.workspace.monitor;
            if monitor.id.is_empty() && self.monitors.len() == 1 {
                let found = &self.monitors[0];
                monitor.id = found.id.clone();
                monitor.name = found.name.clone();
                monitor.fingerprint = found.fingerprint.clone();
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
                    self.pair.status = String::new();
                    self.action(super::Action::Show, ctx);
                }
                Event::Pairing(request) => {
                    self.pair.busy = true;
                    self.pair.outgoing = Some(request);
                }
                Event::Connected { key, name, address } => {
                    self.peer_key = key;
                    self.prefs.workspace.peer_name = name;
                    self.prefs.workspace.last_address = address.to_string();
                    self.pair.remote_address = Some(address);
                    self.pair.request = None;
                    self.pair.outgoing = None;
                    self.pair.busy = false;
                    self.pair.status = String::new();
                    self.pair.input_failed = false;
                    self.pair.audio_failed = false;
                    let _ = self.prefs.save(&self.dir);
                }
                Event::State(remote) => {
                    if self.prefs.workspace.shared.merge(&remote.shared) {
                        let _ = self.prefs.save(&self.dir);
                    }
                    self.pair.remote = Some(*remote);
                }
                Event::Forgotten => self.apply_forget(),
                Event::Closed(error) => {
                    self.pair.remote = None;
                    self.pair.plan = None;
                    self.pair.active_plan = None;
                    self.pair.synced = false;
                    self.pair.request = None;
                    self.pair.outgoing = None;
                    self.pair.busy = false;
                    self.pair.status = error;
                    self.pair.last_dial = Some(Instant::now());
                    for worker in [&mut self.input, &mut self.audio].into_iter().flatten() {
                        worker.stop();
                    }
                    self.pair.input_profile.clear();
                    self.pair.audio_profile.clear();
                }
            }
        }
        let node = self.local_node();
        self.pair.plan = self.pair.remote.as_ref().map(|r| Plan::build(&node, r));
        self.pair.synced = self.pair.scanned
            && self
                .pair
                .remote
                .as_ref()
                .is_some_and(|r| r.hardware.known && !node.ack.is_empty() && node.ack == r.ack);
        if let Some(service) = &self.pair.service {
            service.publish(node);
        }
        if self.prefs.auto_connect
            && self.pair.remote.is_none()
            && !self.pair.busy
            && !self.peer_key.is_empty()
            && self.public_key < self.peer_key
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
            if let Some(peer) = found.filter(discovery::Peer::compatible) {
                self.link_peer(&peer);
            } else if let Ok(address) = self.prefs.workspace.last_address.parse::<SocketAddr>()
                && let Some(service) = &self.pair.service
            {
                self.pair.last_dial = Some(Instant::now());
                let _ = service.dial(vec![address], self.peer_key.clone());
            }
        }
        self.reconcile_services();
    }

    pub(super) fn link_peer(&mut self, peer: &discovery::Peer) {
        let result: Result<()> = (|| {
            ensure!(
                peer.compatible(),
                "Версии не совпадают. Обновите оба компьютера."
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
                self.pair.status = "Запрос отправлен. Подтвердите связь на другом ПК.".into();
            }
            Err(e) => self.pair.status = format!("{e:#}"),
        }
    }

    pub(super) fn approve_pair(&mut self, accept: bool) {
        if let Some(request) = self.pair.request.take()
            && let Some(service) = &self.pair.service
        {
            service.approve(&request.key, accept);
        }
    }

    pub(super) fn shared_changed(&mut self) {
        self.prefs.workspace.shared.changed(&self.public_key);
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
        if self.prefs.workspace.monitor.enabled && !self.prefs.workspace.monitor.id.is_empty() {
            self.scan(QueryKind::Monitors);
        }
        if let Err(e) = self.prefs.save(&self.dir) {
            self.notice = Some((e.to_string(), true));
        }
    }

    pub(super) fn retry_devices(&mut self) {
        self.pair.input_failed = false;
        self.pair.audio_failed = false;
    }

    fn reconcile_services(&mut self) {
        if self.quitting
            || self.install_pending
            || self.install_job.is_some()
            || !self.prefs.discovery
        {
            return;
        }
        let Some(plan) = self.pair.plan.clone() else {
            return;
        };
        let Some(remote) = self.pair.remote.clone() else {
            return;
        };
        let Some(address) = self.pair.remote_address else {
            return;
        };
        // A capability/role change first releases the previous input owner.
        // Nothing new starts until both machines acknowledge the same plan.
        if !self.pair.synced {
            if let Some(active) = &self.pair.active_plan {
                if (active.input_source != plan.input_source
                    || active.monitor != plan.monitor
                    || active.edge != plan.edge
                    || active.purple_on_left != plan.purple_on_left)
                    && let Some(worker) = &mut self.input
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
            self.form.capture_devices = self
                .pair
                .paths
                .iter()
                .map(|p| p.to_string_lossy())
                .collect::<Vec<_>>()
                .join("\n");
            input_profile = format!(
                "{}:{}:{}",
                self.prefs.role,
                self.prefs.address,
                toml::to_string(&self.form.document().unwrap_or_default()).unwrap_or_default()
            );
        }
        if input_profile != self.pair.input_profile {
            if let Some(worker) = &mut self.input {
                worker.stop();
            }
            self.pair.input_profile = input_profile.clone();
            self.pair.input_failed = false;
        }
        if !input_profile.is_empty() {
            if self
                .input
                .as_ref()
                .is_some_and(|w| !w.running() && !w.stopping && w.exit_ok == Some(false))
            {
                self.pair.input_failed = true;
            }
            if !self.input_running() && !self.pair.input_failed {
                let result = self.start_input();
                if let Err(error) = result {
                    self.pair.input_failed = true;
                    self.notice = Some((format!("{error:#}"), true));
                }
            }
        }
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
            self.prefs.volume = self.prefs.workspace.shared.volume;
            self.prefs.muted = self.prefs.workspace.shared.muted;
            let output_device = format!(
                "{}:{}",
                self.pair.hardware.audio_name, remote.hardware.audio_name
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
            if self
                .audio
                .as_ref()
                .is_some_and(|w| !w.running() && !w.stopping && w.exit_ok == Some(false))
            {
                self.pair.audio_failed = true;
            }
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
                    )?);
                    Ok(())
                })();
                if let Err(error) = result {
                    self.pair.audio_failed = true;
                    self.notice = Some((format!("{error:#}"), true));
                }
            } else if self.prefs.audio_mode == "receive" && self.audio_running() {
                let shared = &self.prefs.workspace.shared;
                if let Some(worker) = &mut self.audio {
                    // Commands are small and only emitted when the shared mix changes.
                    let mix = format!("{}:{}", shared.volume, shared.muted);
                    if self.applied_mix != mix {
                        let _ = worker.command(&format!("volume {}", shared.volume));
                        let _ = worker.command(if shared.muted { "mute" } else { "unmute" });
                        self.applied_mix = mix;
                    }
                }
            }
        }
    }
}
