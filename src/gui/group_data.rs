//! The group planner feeds the existing native input/audio workers. Each link
//! keeps its pinned peer identity and its own profile, lock and recovery state.
use super::{
    model::Preferences,
    worker::{Kind, Worker},
};
use crate::{
    config,
    group::model::{Member, Workspace},
    workspace::{self, AudioLevel, hardware::InputEndpoint},
};
use anyhow::{Context, Result, ensure};
use eframe::egui;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, PartialEq)]
pub(super) struct Spec {
    name: String,
    peer: String,
    kind: Kind,
    args: Vec<String>,
    capture: Vec<PathBuf>,
    keyboard: Option<PathBuf>,
    mouse: Option<PathBuf>,
    sharing: config::InputSharing,
    cursor: bool,
    edge: bool,
    routes: crate::edge::Routes,
    entry: Option<u16>,
    display_entry: Option<crate::edge::Entry>,
    generation: u64,
    desired_remote: bool,
    input_returned: bool,
    input_request: u64,
    activate: bool,
    native: Option<AudioLevel>,
}
impl Spec {
    fn input_host(&self) -> bool {
        self.kind == Kind::Input && self.args.get(2).is_some_and(|role| role == "host")
    }

    fn switched_after_generation(
        &self,
        status: Option<crate::lifecycle::InputStatus>,
        switched: bool,
    ) -> bool {
        // Peer means forwarding only on the host. A receiver reports Peer as
        // soon as it authenticates, before it receives an input transfer.
        self.input_host()
            && (status == Some(crate::lifecycle::InputStatus::Peer)
                || (!self.desired_remote && switched))
    }

    fn return_host_input(&self, switched: bool) -> bool {
        self.input_host() && !self.desired_remote && switched
    }
}
struct Task {
    spec: Spec,
    worker: Worker,
    switched: bool,
    was_remote: bool,
    cycling: bool,
    cycle_since: Option<std::time::Instant>,
    coordinated_return: Option<u64>,
    switch_failed: bool,
    restart_requested: bool,
    local_entry: Option<u64>,
    manual_return: bool,
    manual_request: u64,
    retry_after_auth: bool,
    retry_after: Option<std::time::Instant>,
    restart_count: u8,
}

#[derive(Debug, Eq, PartialEq)]
enum SessionEffect {
    Recovering,
    Recovered,
    ManualReturn,
    None,
}
impl Task {
    fn reconcile_intent(&mut self, wanted: &Spec) {
        if self.manual_return
            && (wanted.input_returned || wanted.input_request > self.manual_request)
        {
            self.manual_return = false;
            if !self.retry_after_auth {
                self.switch_failed = false;
            }
        }
    }

    fn input_session(&mut self, session: crate::lifecycle::InputSession) -> SessionEffect {
        use crate::lifecycle::{InputSession, RecoveryReason};
        match session {
            InputSession::Recovery(reason) => {
                self.switched = false;
                self.was_remote = false;
                self.local_entry = None;
                self.cycling = false;
                self.cycle_since = None;
                self.switch_failed = true;
                if reason == RecoveryReason::EmergencyKey {
                    self.manual_return = true;
                    self.manual_request = self.spec.input_request;
                    self.retry_after_auth = false;
                    self.retry_after = None;
                    SessionEffect::ManualReturn
                } else {
                    self.retry_after_auth = true;
                    SessionEffect::Recovering
                }
            }
            InputSession::Authenticated => {
                if !self.worker.input_authenticated || !self.worker.running() {
                    return SessionEffect::None;
                }
                self.restart_count = 0;
                self.retry_after = None;
                if self.manual_return {
                    return SessionEffect::None;
                }
                if self.retry_after_auth {
                    self.switch_failed = false;
                    self.retry_after_auth = false;
                }
                if self.worker.switch_error.is_none()
                    && !self.switch_failed
                    && self.worker.input_authenticated
                {
                    SessionEffect::Recovered
                } else {
                    SessionEffect::None
                }
            }
        }
    }
}

fn equivalent(a: &Spec, b: &Spec) -> bool {
    let normalize = |spec: &Spec| {
        let mut spec = spec.clone();
        spec.activate = false;
        spec.native = None;
        spec.cursor = false;
        spec.edge = false;
        spec.routes = crate::edge::Routes::default();
        spec.entry = None;
        spec.display_entry = None;
        spec.generation = 0;
        spec.desired_remote = false;
        spec.input_returned = false;
        spec.input_request = 0;
        if matches!(spec.kind, Kind::AudioReceive | Kind::MicrophoneReceive) {
            let mut args = Vec::new();
            let mut input = spec.args.into_iter();
            while let Some(arg) = input.next() {
                if arg == "--volume" {
                    input.next();
                } else if arg != "--mute" {
                    args.push(arg);
                }
            }
            spec.args = args;
        }
        spec
    };
    normalize(a) == normalize(b)
}
fn mix_volume(spec: &Spec) -> Option<&str> {
    spec.args
        .windows(2)
        .find(|args| args[0] == "--volume")
        .map(|args| args[1].as_str())
}
#[derive(Default)]
pub(super) struct Data {
    tasks: BTreeMap<String, Task>,
    routing_cache: crate::group::layout::RoutingCache,
    pub error: Option<String>,
    pub input_notice: Option<String>,
    pub input_recovered: bool,
    manual_request: Option<u64>,
    pub cycle: bool,
    pub cycle_position: Option<u16>,
    pub crossing: Option<crate::edge::Crossing>,
    pub returned: bool,
    pub volume: Option<AudioLevel>,
    pub input: Option<crate::lifecycle::InputStatus>,
    pub audio_playing: bool,
    pub microphone_active: bool,
}

fn endpoint(member: &Member, port: u16) -> String {
    let mut address = member.address;
    address.set_port(port);
    address.to_string()
}
fn microphone_port(state: &Workspace, destination: &str) -> Result<u16> {
    let offset = state
        .members
        .keys()
        .position(|key| key == destination)
        .context("Unknown microphone receiver")?;
    u16::try_from(49_000usize + offset).context("No available microphone TCP port")
}
fn device_arg(args: &mut Vec<String>, flag: &str, device: &Option<String>) {
    if let Some(device) = device {
        args.extend([flag.into(), device.clone()]);
    }
}

#[cfg(test)]
fn desired(
    state: &Workspace,
    own: &str,
    inputs: &[InputEndpoint],
    cursor: bool,
) -> Result<BTreeMap<String, Spec>> {
    desired_with_routes(
        state,
        own,
        inputs,
        cursor,
        crate::group::layout::routes_for(state, own),
    )
}

fn desired_with_routes(
    state: &Workspace,
    own: &str,
    inputs: &[InputEndpoint],
    cursor: bool,
    edge_routes: crate::edge::Routes,
) -> Result<BTreeMap<String, Spec>> {
    let mut result = BTreeMap::new();
    let Some(local) = state.members.get(own).filter(|member| member.compatible()) else {
        return Ok(result);
    };
    let plan = state.plan();
    let routes = state.routes(own);
    let choices = &local.preferences.devices;
    let blank = |name: String, peer: String, kind, args| Spec {
        name,
        peer,
        kind,
        args,
        capture: Vec::new(),
        keyboard: None,
        mouse: None,
        sharing: if kind == Kind::Input {
            config::InputSharing {
                keyboard: state.settings.keyboard_enabled,
                mouse: state.settings.mouse_enabled,
            }
        } else {
            config::InputSharing::default()
        },
        cursor: kind == Kind::Input && cursor,
        edge: kind == Kind::Input && state.settings.edge && state.settings.mouse_enabled,
        routes: if kind == Kind::Input {
            edge_routes.clone()
        } else {
            Default::default()
        },
        entry: None,
        display_entry: None,
        generation: state.activation,
        desired_remote: false,
        input_returned: state.input_intent == plan.input && state.active_input == plan.input,
        input_request: state.input_request,
        activate: false,
        native: None,
    };
    if let Some(source) = plan.input.as_ref() {
        let host = source == own;
        let peer = if host {
            state
                .active_input
                .as_ref()
                .filter(|key| key.as_str() != own)
                .and_then(|key| state.members.get(key))
                .filter(|member| member.compatible())
                .or_else(|| {
                    state
                        .members
                        .values()
                        .find(|member| member.node.key != own && member.compatible())
                })
        } else {
            state.members.get(source)
        };
        if let Some(peer) = peer {
            let address = if host {
                endpoint(peer, workspace::INPUT_PORT)
            } else {
                "[::]:45876".into()
            };
            let mut spec = blank(
                "input".into(),
                peer.node.key.clone(),
                Kind::Input,
                vec![
                    "run".into(),
                    "--role".into(),
                    if host { "host" } else { "receiver" }.into(),
                    address,
                ],
            );
            spec.desired_remote = host && state.active_input.as_ref() == Some(&peer.node.key);
            spec.activate = spec.desired_remote
                && peer.input_ready == state.activation
                && peer.node.monitor_error.is_none();
            spec.entry = host.then_some(state.active_input_entry).flatten();
            spec.display_entry = host.then_some(state.display_entry).flatten();
            for input in inputs {
                let keyboard = input.keyboard
                    && choices
                        .keyboard
                        .as_ref()
                        .is_none_or(|id| id == &input.info.id);
                let mouse =
                    input.mouse && choices.mouse.as_ref().is_none_or(|id| id == &input.info.id);
                if (keyboard || mouse) && !spec.capture.contains(&input.path) {
                    spec.capture.push(input.path.clone());
                }
                if keyboard && spec.keyboard.is_none() {
                    spec.keyboard = Some(input.path.clone());
                }
                if mouse && spec.mouse.is_none() {
                    spec.mouse = Some(input.path.clone());
                }
            }
            result.insert(spec.name.clone(), spec);
        }
    }
    if let Some(output) = plan.audio.as_ref() {
        if output == own {
            for member in routes
                .audio_sources
                .iter()
                .filter_map(|route| state.members.get(&route.key))
            {
                let level = member.preferences.audio;
                let mut args = vec![
                    "audio".into(),
                    "play".into(),
                    endpoint(member, workspace::AUDIO_PORT),
                    "--volume".into(),
                    level.volume.to_string(),
                ];
                if level.muted {
                    args.push("--mute".into());
                }
                device_arg(&mut args, "--output", &choices.speaker);
                let mut spec = blank(
                    format!("audio-receive-{}", member.node.key),
                    member.node.key.clone(),
                    Kind::AudioReceive,
                    args,
                );
                spec.native = local.preferences.output;
                result.insert(spec.name.clone(), spec);
            }
        } else {
            let mut args = vec!["audio".into(), "serve".into(), "[::]:45877".into()];
            device_arg(&mut args, "--source", &choices.speaker);
            let spec = blank("audio-send".into(), output.clone(), Kind::AudioSend, args);
            result.insert(spec.name.clone(), spec);
        }
    }
    if let Some(binding) = plan.microphone.as_ref() {
        if binding.computer == own {
            for member in routes
                .microphone_targets
                .iter()
                .filter_map(|route| state.members.get(&route.key))
            {
                let args = vec![
                    "microphone".into(),
                    "serve".into(),
                    format!("[::]:{}", microphone_port(state, &member.node.key)?),
                    "--source".into(),
                    binding.device.clone(),
                ];
                let spec = blank(
                    format!("microphone-send-{}", member.node.key),
                    member.node.key.clone(),
                    Kind::MicrophoneSend,
                    args,
                );
                result.insert(spec.name.clone(), spec);
            }
        } else if local.node.hardware.microphone_receive {
            let owner = &state.members[&binding.computer];
            let mut args = vec![
                "microphone".into(),
                "play".into(),
                endpoint(owner, microphone_port(state, own)?),
                "--volume".into(),
                if state.settings.microphone_level.muted {
                    0
                } else {
                    state.settings.microphone_level.volume
                }
                .to_string(),
            ];
            device_arg(&mut args, "--output", &choices.microphone_bridge);
            let spec = blank(
                "microphone-receive".into(),
                binding.computer.clone(),
                Kind::MicrophoneReceive,
                args,
            );
            result.insert(spec.name.clone(), spec);
        }
    }
    Ok(result)
}

fn configure(root: &Path, spec: &Spec) -> Result<PathBuf> {
    crate::group::model::valid_key(&spec.peer)?;
    ensure!(
        spec.name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "Invalid service profile"
    );
    let dir = root.join("group-services").join(&spec.name);
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.parent().unwrap(),
            std::fs::Permissions::from_mode(0o700),
        )?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    if !dir.join("identity").exists() {
        std::fs::copy(root.join("identity"), dir.join("identity"))?;
    }
    config::write_atomic(&dir.join("peer"), &spec.peer)?;
    config::save_settings(
        &dir,
        &config::FileSettings {
            monitor: None,
            edge: None,
            routes: (spec.kind == Kind::Input).then(|| spec.routes.clone()),
            capture: Some(config::CaptureSettings {
                devices: spec.capture.clone(),
                keyboard: spec.keyboard.clone(),
                mouse: spec.mouse.clone(),
                sharing: spec.sharing,
            }),
        },
    )?;
    let prefs = Preferences {
        show_remote_cursor: spec.cursor,
        autostart: false,
        auto_connect: false,
        auto_update: false,
        ..Default::default()
    };
    prefs.save(&dir)?;
    if spec.kind == Kind::Input {
        migrate_portal_token(root, &dir)?;
        config::write_atomic(&dir.join("group-input"), "1")?;
    }
    Ok(dir)
}

fn migrate_portal_token(root: &Path, dir: &Path) -> Result<()> {
    let source = root.join("wayland-restore-token");
    let destination = dir.join("wayland-restore-token");
    if source.exists() && !destination.exists() {
        // Portal restore tokens are single use. Move the old token while its
        // session lock is available, and preserve any newer group grant.
        let _lock = config::lock(root, "portal.lock")?;
        if source.exists() && !destination.exists() {
            std::fs::rename(source, destination)
                .context("Moving the existing Wayland permission into the group input profile")?;
        }
    }
    Ok(())
}
impl Data {
    pub fn desired(
        &mut self,
        state: &Workspace,
        own: &str,
        inputs: &[InputEndpoint],
        cursor: bool,
    ) -> Result<BTreeMap<String, Spec>> {
        let routes = self.routing_cache.routes_for(state, own);
        desired_with_routes(state, own, inputs, cursor, routes)
    }

    pub fn emergency(&mut self) -> Result<()> {
        let mut result = Ok(());
        for task in self.tasks.values_mut().filter(|task| {
            task.spec.kind == Kind::Input && task.worker.running() && !task.worker.stopping
        }) {
            if let Err(error) = task.worker.command("emergency") {
                result = Err(error);
            }
        }
        result
    }
    pub fn report(&self, node: &mut workspace::Node) {
        node.input = self.input;
        node.audio_playing = self.audio_playing;
        node.microphone_active = self.microphone_active;
        node.input_error = None;
        node.audio_error = None;
        node.microphone_error = None;
        for task in self.tasks.values().filter(|task| !task.worker.stopping) {
            let error = task.worker.switch_error.clone().or_else(|| {
                (task.worker.exit_ok == Some(false))
                    .then(|| task.worker.logs.back().cloned())
                    .flatten()
            });
            let field = match task.spec.kind {
                Kind::Input => &mut node.input_error,
                Kind::AudioSend | Kind::AudioReceive => &mut node.audio_error,
                Kind::MicrophoneSend | Kind::MicrophoneReceive => &mut node.microphone_error,
                _ => continue,
            };
            if let Some(mut error) = error {
                error.truncate(error.floor_char_boundary(2048));
                field.get_or_insert(error);
            }
            let service = match task.spec.kind {
                Kind::Input => "Input",
                Kind::AudioSend => "Audio send",
                Kind::AudioReceive => "Audio receive",
                Kind::MicrophoneSend => "Microphone send",
                Kind::MicrophoneReceive => "Microphone receive",
                _ => unreachable!(),
            };
            for line in task
                .worker
                .logs
                .iter()
                .skip(task.worker.logs.len().saturating_sub(4))
            {
                node.diagnostics.line(service, line);
            }
        }
    }
    pub fn stop(&mut self) {
        for task in self.tasks.values_mut() {
            task.worker.stop();
        }
    }
    pub fn quiet(&self) -> bool {
        self.tasks.values().all(|task| task.worker.settled())
    }
    pub fn force_stop(&mut self) {
        for task in self.tasks.values_mut() {
            task.worker.force_stop();
        }
    }
    pub fn retry(&mut self) {
        for task in self.tasks.values_mut() {
            if task.switch_failed || !task.worker.running() {
                task.restart_requested = true;
                task.restart_count = 0;
                task.retry_after = None;
                task.worker.stop();
            }
        }
        self.error = None;
    }
    #[cfg(target_os = "linux")]
    pub fn permission_denied(&self) -> bool {
        self.tasks.values().any(|task| {
            task.spec.kind == Kind::Input
                && task.spec.args.get(2).is_some_and(|role| role == "host")
                && task.worker.age() >= std::time::Duration::from_secs(2)
                && task
                    .worker
                    .logs
                    .iter()
                    .any(|line| super::input_permission_error(line))
        })
    }
    #[cfg(target_os = "linux")]
    pub fn permission_retry_after(&self) -> Option<std::time::Duration> {
        self.tasks.values().find_map(|task| {
            let delay = std::time::Duration::from_secs(2);
            (task.spec.kind == Kind::Input
                && task.spec.args.get(2).is_some_and(|role| role == "host")
                && task.worker.age() < delay
                && task
                    .worker
                    .logs
                    .iter()
                    .any(|line| super::input_permission_error(line)))
            .then(|| delay.saturating_sub(task.worker.age()))
        })
    }
    #[cfg(target_os = "linux")]
    pub fn input_permission_paths(&self) -> Vec<PathBuf> {
        self.tasks
            .values()
            .find(|task| {
                task.spec.kind == Kind::Input
                    && task.spec.args.get(2).is_some_and(|role| role == "host")
            })
            .map(|task| task.spec.capture.clone())
            .unwrap_or_default()
    }
    pub fn tick(&mut self, desired: BTreeMap<String, Spec>, root: &Path, ctx: &egui::Context) {
        self.input = None;
        self.input_notice = None;
        self.input_recovered = false;
        self.audio_playing = false;
        self.microphone_active = false;
        for (name, task) in &mut self.tasks {
            if !desired
                .get(name)
                .is_some_and(|wanted| equivalent(&task.spec, wanted))
            {
                // A hotplug can remove the source or reverse this link's role.
                // Release the old capture before consuming its queued events:
                // its final recovery must not return the new owner's input.
                task.worker.stop();
                task.worker.poll();
                if !task.worker.settled() {
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
                continue;
            }
            if task.worker.poll() {
                self.error = task.worker.logs.back().cloned();
                if task.spec.kind == Kind::Input {
                    self.input_notice = self.error.clone();
                    task.switch_failed = true;
                    task.switched = false;
                    task.was_remote = false;
                    if !task.manual_return && task.restart_count < 3 {
                        task.restart_count += 1;
                        task.retry_after = Some(
                            std::time::Instant::now()
                                + std::time::Duration::from_secs(
                                    3 * u64::from(task.restart_count).pow(2),
                                ),
                        );
                    }
                }
            }
            task.spec.input_request = desired[name].input_request;
            while let Some(session) = task.worker.input_sessions.pop_front() {
                match task.input_session(session) {
                    SessionEffect::ManualReturn => {
                        self.returned = true;
                        self.manual_request = Some(task.manual_request);
                        self.input_recovered = false;
                    }
                    SessionEffect::Recovering => self.input_recovered = false,
                    SessionEffect::Recovered => self.input_recovered = true,
                    SessionEffect::None => {}
                }
            }
            while let Some((activation, completed)) = task.worker.return_results.pop_front() {
                if task.coordinated_return == Some(activation) {
                    task.coordinated_return = None;
                    if completed {
                        task.was_remote = false;
                        task.switched = false;
                        task.local_entry = None;
                    }
                }
            }
            if desired
                .get(name)
                .is_some_and(|spec| spec.generation != task.spec.generation)
                && !task.worker.running()
            {
                task.restart_requested = true;
                task.restart_count = 0;
                task.retry_after = None;
            }
            if let Some(retry) = task.retry_after {
                let remaining = retry.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    task.restart_requested = true;
                    task.retry_after = None;
                } else {
                    ctx.request_repaint_after(remaining);
                }
            }
            if task.worker.cycle_input {
                task.worker.cycle_input = false;
                let position = task.worker.cycle_input_position.take();
                if task.spec.input_host() {
                    task.cycling = true;
                    task.cycle_since = Some(std::time::Instant::now());
                    self.cycle = true;
                    self.cycle_position = position;
                }
            }
            if let Some(crossing) = task.worker.crossing.take()
                && task.spec.input_host()
                && crossing.activation == task.spec.generation
            {
                task.cycling = true;
                task.cycle_since = Some(std::time::Instant::now());
                self.crossing = Some(crossing);
            }
            if let Some((activation, error)) = task.worker.display_error.take()
                && activation == task.spec.generation
            {
                // A failed optional placement leaves the current input session
                // usable. It must not become an emergency return or disconnect.
                self.error = Some(error);
                self.input_notice = self.error.clone();
            }
            if task.cycling
                && task
                    .cycle_since
                    .is_some_and(|started| started.elapsed() >= std::time::Duration::from_secs(2))
            {
                // An unavailable coordinator or rejected edge must not suppress
                // a real local recovery indefinitely.
                task.cycling = false;
                task.cycle_since = None;
            }
            if task.spec.kind == Kind::Input {
                self.input = task.worker.input_status;
                if task.spec.input_host() {
                    if task.worker.input_status == Some(crate::lifecycle::InputStatus::Peer) {
                        task.was_remote = true;
                    }
                    if task.was_remote
                        && task.worker.input_status == Some(crate::lifecycle::InputStatus::Here)
                        && task.coordinated_return.is_none()
                        && !task.cycling
                        && !task.worker.stopping
                    {
                        task.was_remote = false;
                        task.switched = false;
                    }
                }
                if let Some(error) = task.worker.new_switch_error.take() {
                    task.switch_failed = true;
                    self.error = Some(error);
                    self.input_notice = self.error.clone();
                }
            }
            if let Some(level) = task.worker.output_level.changed.take() {
                self.volume = Some(level);
            }
            if task.worker.running() && !task.worker.stopping {
                self.audio_playing |=
                    matches!(task.spec.kind, Kind::AudioSend | Kind::AudioReceive)
                        && task.worker.audio_playing;
                self.microphone_active |= matches!(
                    task.spec.kind,
                    Kind::MicrophoneSend | Kind::MicrophoneReceive
                ) && task.worker.audio_playing;
            }
        }
        let restart_counts = self
            .tasks
            .iter()
            .filter(|(_, task)| task.restart_requested)
            .map(|(name, task)| (name.clone(), task.restart_count))
            .collect::<BTreeMap<_, _>>();
        self.tasks.retain(|name, task| {
            !task.worker.settled()
                || (!task.restart_requested
                    && desired
                        .get(name)
                        .is_some_and(|spec| equivalent(&task.spec, spec)))
        });
        for (name, spec) in desired {
            if spec.kind == Kind::Input
                && self
                    .manual_request
                    .is_some_and(|request| spec.input_returned || spec.input_request > request)
            {
                self.manual_request = None;
            }
            if !self.tasks.contains_key(&name) {
                match configure(root, &spec)
                    .and_then(|dir| Worker::start(spec.kind, &spec.args, &dir, ctx))
                {
                    Ok(worker) => {
                        let manual = self.manual_request.filter(|_| spec.kind == Kind::Input);
                        self.tasks.insert(
                            name.clone(),
                            Task {
                                spec: spec.clone(),
                                worker,
                                switched: false,
                                was_remote: false,
                                cycling: false,
                                cycle_since: None,
                                coordinated_return: None,
                                switch_failed: manual.is_some(),
                                restart_requested: false,
                                local_entry: None,
                                manual_return: manual.is_some(),
                                manual_request: manual.unwrap_or(0),
                                retry_after_auth: false,
                                retry_after: None,
                                restart_count: restart_counts.get(&name).copied().unwrap_or(0),
                            },
                        );
                    }
                    Err(error) => {
                        self.error = Some(format!("{error:#}"));
                        continue;
                    }
                }
            }
            let task = self.tasks.get_mut(&name).unwrap();
            if task.worker.stopping {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                continue;
            }
            task.reconcile_intent(&spec);
            if task.spec.generation != spec.generation {
                task.spec.generation = spec.generation;
                task.cycling = false;
                task.cycle_since = None;
                if !task.manual_return
                    || spec.input_returned
                    || spec.input_request > task.manual_request
                {
                    task.switch_failed = false;
                    task.manual_return = false;
                    task.retry_after_auth = false;
                }
                // A new activation may retry a rejected switch, but must not
                // toggle an already active destination back to the source.
                task.switched =
                    spec.switched_after_generation(task.worker.input_status, task.switched);
            }
            if task.spec.kind == Kind::Input && task.spec.routes != spec.routes {
                match task.worker.set_routes(&spec.routes) {
                    Ok(()) => task.spec.routes = spec.routes.clone(),
                    Err(error) => self.error = Some(error.to_string()),
                }
            }
            task.spec.edge = spec.edge;
            if matches!(spec.kind, Kind::AudioReceive | Kind::MicrophoneReceive)
                && task.spec.args != spec.args
            {
                if let Some(volume) = mix_volume(&spec) {
                    let _ = task.worker.command(&format!("volume {volume}"));
                }
                if spec.kind == Kind::AudioReceive {
                    let _ = task
                        .worker
                        .command(if spec.args.iter().any(|arg| arg == "--mute") {
                            "mute"
                        } else {
                            "unmute"
                        });
                }
                task.spec.args = spec.args.clone();
            }
            if task.spec.cursor != spec.cursor {
                let _ = task.worker.command(if spec.cursor {
                    "remote-cursor on"
                } else {
                    "remote-cursor off"
                });
                task.spec.cursor = spec.cursor;
            }
            task.spec.activate = spec.activate;
            task.spec.desired_remote = spec.desired_remote;
            task.spec.input_returned = spec.input_returned;
            task.spec.input_request = spec.input_request;
            if task.local_entry != Some(spec.generation)
                && task.coordinated_return.is_none()
                && spec.input_host()
                && task.worker.input_authenticated
                && let Some(entry) = spec.display_entry
                && (!spec.desired_remote || task.switched)
                && task.worker.input_status
                    == Some(if spec.desired_remote {
                        crate::lifecycle::InputStatus::Peer
                    } else {
                        crate::lifecycle::InputStatus::Here
                    })
                && task.worker.enter_display(spec.generation, entry).is_ok()
            {
                task.local_entry = Some(spec.generation);
            }
            if !spec.desired_remote
                && spec.edge
                && spec.display_entry.is_none()
                && task.local_entry != Some(spec.generation)
                && task.worker.input_status == Some(crate::lifecycle::InputStatus::Here)
                && spec.input_host()
                && task.worker.input_authenticated
                && let Some(position) = spec.entry
                && task
                    .worker
                    .command(&format!("enter-edge {position}"))
                    .is_ok()
            {
                task.local_entry = Some(spec.generation);
            }
            task.spec.entry = spec.entry;
            task.spec.display_entry = spec.display_entry;
            if spec.input_host()
                && spec.activate
                && task.coordinated_return.is_none()
                && !task.switched
                && !task.switch_failed
                && task.worker.input_authenticated
                && task.worker.input_status == Some(crate::lifecycle::InputStatus::Here)
            {
                if let Err(error) = task
                    .worker
                    .switch_display(spec.generation, spec.display_entry)
                {
                    self.error = Some(error.to_string());
                    task.switch_failed = true;
                } else {
                    task.switched = true;
                    task.local_entry = Some(spec.generation);
                }
            }
            if spec.return_host_input(task.switched) && task.coordinated_return.is_none() {
                match task.worker.return_input(spec.generation) {
                    Ok(()) => task.coordinated_return = Some(spec.generation),
                    Err(error) => self.error = Some(error.to_string()),
                }
                task.switched = false;
                task.was_remote = false;
            }
            if let Some(level) = spec.native
                && task.worker.kind == Kind::AudioReceive
            {
                let _ = task.worker.set_output_level(level);
            }
            if task.worker.needs_poll() {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};

    fn input_session_fixture(spec: Spec) -> Task {
        let ctx = egui::Context::default();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/qa-input-session");
        // This child only prints the test harness's help and exits.
        let worker = Worker::start(Kind::Input, &["--help".into()], &root, &ctx).unwrap();
        Task {
            spec,
            worker,
            switched: true,
            was_remote: true,
            cycling: false,
            cycle_since: None,
            coordinated_return: None,
            switch_failed: false,
            restart_requested: false,
            local_entry: None,
            manual_return: false,
            manual_request: 0,
            retry_after_auth: false,
            retry_after: None,
            restart_count: 0,
        }
    }

    #[test]
    fn input_recovery_keeps_the_target_and_rearms_only_after_authentication() {
        use crate::lifecycle::{InputSession, RecoveryReason};
        let mut state = crate::group::model::tests::fixture(2);
        let source = state.plan().input.unwrap();
        let target = state
            .members
            .keys()
            .find(|key| *key != &source)
            .unwrap()
            .clone();
        state.activate(&source, &target, None).unwrap();
        let spec = desired(&state, &source, &[], true).unwrap()["input"].clone();
        let mut task = input_session_fixture(spec);
        for reason in [
            RecoveryReason::ConnectionLost,
            RecoveryReason::HeartbeatTimeout,
            RecoveryReason::LeaseExpired,
            RecoveryReason::InputOverflow,
            RecoveryReason::ControlOverflow,
            RecoveryReason::SwitchFailed,
        ] {
            task.worker.input_authenticated = false;
            assert_eq!(
                task.input_session(InputSession::Recovery(reason)),
                SessionEffect::Recovering
            );
            assert!(!task.switched && !task.was_remote && task.switch_failed);
            assert!(!task.manual_return && task.spec.desired_remote);
            assert_eq!(task.spec.peer, target);
            task.worker.input_authenticated = true;
            assert_eq!(
                task.input_session(InputSession::Authenticated),
                SessionEffect::Recovered
            );
            assert!(!task.switch_failed && !task.switched);
        }
        task.worker.force_stop();
    }

    #[test]
    fn emergency_return_survives_reauthentication_and_auto_restore_but_accepts_new_user_intent() {
        use crate::lifecycle::{InputSession, RecoveryReason};
        let mut state = crate::group::model::tests::fixture(2);
        let source = state.plan().input.unwrap();
        let target = state
            .members
            .keys()
            .find(|key| *key != &source)
            .unwrap()
            .clone();
        state.activate(&source, &target, None).unwrap();
        let spec = desired(&state, &source, &[], true).unwrap()["input"].clone();
        let mut task = input_session_fixture(spec);
        assert_eq!(
            task.input_session(InputSession::Recovery(RecoveryReason::EmergencyKey)),
            SessionEffect::ManualReturn
        );
        task.worker.input_authenticated = true;
        assert_eq!(
            task.input_session(InputSession::Authenticated),
            SessionEffect::None
        );
        assert!(task.manual_return && task.switch_failed);
        let request = state.input_request;
        state.members.get_mut(&target).unwrap().online = false;
        state.reconcile_input();
        state.members.get_mut(&target).unwrap().online = true;
        state.reconcile_input();
        assert_eq!(state.input_request, request);
        let automatic = desired(&state, &source, &[], true).unwrap()["input"].clone();
        assert!(automatic.generation > task.spec.generation);
        task.reconcile_intent(&automatic);
        assert!(task.manual_return && task.switch_failed);

        // A deliberate retry of the same server-side target is a new request,
        // even though no physical monitor command is needed.
        state.activate(&source, &target, None).unwrap();
        let explicit = desired(&state, &source, &[], true).unwrap()["input"].clone();
        assert!(explicit.input_request > request);
        task.reconcile_intent(&explicit);
        assert!(!task.manual_return && !task.switch_failed);
        assert!(state.monitor_command.is_none());
        task.worker.force_stop();
    }

    #[test]
    fn moving_physical_input_reverses_roles_and_discards_the_old_hosts_queued_recovery() {
        use crate::lifecycle::InputStatus;
        let mut state = crate::group::model::tests::fixture(2);
        let windows = state.plan().input.unwrap();
        let ubuntu = state
            .members
            .keys()
            .find(|key| *key != &windows)
            .unwrap()
            .clone();
        state.active_input = Some(ubuntu.clone());
        let old_host = desired(&state, &windows, &[], true).unwrap()["input"].clone();
        assert!(old_host.input_host() && old_host.desired_remote);

        let removed = state.members.get_mut(&windows).unwrap();
        let keyboards = std::mem::take(&mut removed.node.devices.keyboards);
        let mice = std::mem::take(&mut removed.node.devices.mice);
        removed.node.hardware.keyboard = false;
        removed.node.hardware.mouse = false;
        assert!(
            !desired(&state, &windows, &[], true)
                .unwrap()
                .contains_key("input")
        );
        let attached = state.members.get_mut(&ubuntu).unwrap();
        attached.node.devices.keyboards = keyboards;
        attached.node.devices.mice = mice;
        attached.node.hardware.keyboard = true;
        attached.node.hardware.mouse = true;
        let receiver = desired(&state, &windows, &[], true).unwrap()["input"].clone();
        let host = desired(&state, &ubuntu, &[], true).unwrap()["input"].clone();
        assert!(!receiver.input_host());
        assert_eq!(receiver.peer, ubuntu);
        assert!(host.input_host() && !host.desired_remote && !host.activate);
        assert_eq!(host.peer, windows);
        assert!(!equivalent(&old_host, &receiver));

        // The fixture child only prints the test harness's help. It never
        // enters the application's native input/audio/monitor code.
        let ctx = egui::Context::default();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/qa-retired-input");
        let mut worker = Worker::start(Kind::Input, &["--help".into()], &root, &ctx).unwrap();
        worker.input_status = Some(InputStatus::Here);
        worker.cycle_input = true;
        worker.cycle_input_position = Some(12_345);
        worker.new_switch_error = Some("The superseded host restored its local input".into());
        let mut data = Data::default();
        data.tasks.insert(
            "input".into(),
            Task {
                spec: old_host,
                worker,
                switched: true,
                was_remote: true,
                cycling: false,
                cycle_since: None,
                coordinated_return: None,
                switch_failed: false,
                restart_requested: false,
                local_entry: None,
                manual_return: false,
                manual_request: 0,
                retry_after_auth: false,
                retry_after: None,
                restart_count: 0,
            },
        );
        // During unplug there is no source. This must release the capture and
        // ignore its already queued recovery before the new host is started.
        data.tick(BTreeMap::new(), &root, &ctx);
        assert!(!data.returned && !data.cycle);
        assert!(data.input.is_none() && data.error.is_none());
        assert!(data.tasks.values().all(|task| task.worker.stopping));
        data.force_stop();
    }

    #[test]
    fn arranging_a_third_display_updates_routes_without_restarting_input_and_carries_exact_entry() {
        use crate::{
            edge::{Crossing, NativeScreen, ScreenRect, Side},
            group::layout::{self, Point},
        };
        let mut state = crate::group::model::tests::fixture(3);
        let source = state.plan().input.unwrap();
        let keys = state.members.keys().cloned().collect::<Vec<_>>();
        for (index, member) in state.members.values_mut().enumerate() {
            member.node.monitor.enabled = false;
            member.node.devices.displays = vec![workspace::devices::Display {
                id: format!("ordinary-{index}"),
                name: format!("Display {index}"),
                fingerprint: String::new(),
                native: Some(NativeScreen {
                    native_id: format!("native-{index}"),
                    rect: ScreenRect {
                        x: 0,
                        y: 0,
                        width: 1920,
                        height: 1080,
                    },
                    epoch: index as u64 + 1,
                }),
            }];
        }
        state.active_input = Some(source.clone());
        state.settings.edge = true;
        let mut arrangement = layout::draft(&state);
        for (index, key) in keys.iter().enumerate() {
            arrangement.positions.insert(
                key.clone(),
                Point {
                    x: index as i32 * 1920,
                    y: 0,
                },
            );
        }
        state.set_layout(0, arrangement).unwrap();
        let original = desired(&state, &source, &[], true).unwrap()["input"].clone();
        assert_eq!(original.routes.boundaries.len(), 1);
        assert_eq!(original.routes.boundaries[0].side, Side::Right);

        let mut arrangement = state.layout.clone();
        arrangement
            .positions
            .insert(keys[2].clone(), Point { x: 0, y: 1080 });
        state
            .set_layout(state.layout.revision, arrangement)
            .unwrap();
        let changed = desired(&state, &source, &[], true).unwrap()["input"].clone();
        assert!(
            equivalent(&original, &changed),
            "Applying positions must keep the authenticated native input worker"
        );
        assert_ne!(original.routes, changed.routes);
        let bottom = changed
            .routes
            .boundaries
            .iter()
            .find(|boundary| boundary.side == Side::Bottom)
            .unwrap();
        let activation = layout::resolve_crossing(
            &state,
            &source,
            Crossing {
                revision: changed.routes.revision,
                activation: state.activation,
                boundary: bottom.id,
                position: 20_000,
            },
        )
        .unwrap()
        .unwrap();
        let entry = activation.entry;
        assert_eq!(entry.side, Side::Top);
        assert_eq!(entry.geometry, 3);
        state
            .activate_display(&source, activation.target, Some(entry))
            .unwrap();
        let routed = desired(&state, &source, &[], true).unwrap()["input"].clone();
        assert_eq!(routed.peer, keys[2]);
        assert!(routed.activate);
        assert_eq!(routed.display_entry, Some(entry));
        assert!(routed.entry.is_none());
        assert!(
            state.monitor_command.is_none(),
            "An ordinary display must not select a monitor port"
        );
    }

    #[test]
    fn receiver_authentication_and_monitor_generations_never_request_emergency_return() {
        use crate::lifecycle::InputStatus;
        let mut state = crate::group::model::tests::fixture(2);
        let source = state.plan().input.unwrap();
        let receiver = state
            .members
            .keys()
            .find(|key| *key != &source)
            .unwrap()
            .clone();
        let original = desired(&state, &receiver, &[], true).unwrap()["input"].clone();
        let mut switched = true;
        for generation in 1..=7 {
            state.activation = generation;
            state.active_input = Some(receiver.clone());
            let member = state.members.get_mut(&receiver).unwrap();
            member.input_ready = generation;
            member.node.monitor_error = (generation == 7).then(|| "DDCRC_RETRIES(EIO)".into());
            let wanted = desired(&state, &receiver, &[], true).unwrap();
            let spec = &wanted["input"];
            assert!(!spec.input_host() && !spec.desired_remote && !spec.activate);
            assert!(
                equivalent(&original, spec),
                "monitor state must not restart the authenticated receiver"
            );
            // Receivers report Peer at authentication and Here when input is active.
            for status in [InputStatus::Peer, InputStatus::Here, InputStatus::Peer] {
                switched = spec.switched_after_generation(Some(status), switched);
                assert!(!switched);
                assert!(!spec.return_host_input(switched));
            }
            assert!(
                !spec.return_host_input(true),
                "even stale host bookkeeping must never send emergency to a receiver"
            );
        }
    }

    #[test]
    fn host_keeps_forwarding_while_monitor_state_changes_and_returns_only_to_its_owner() {
        use crate::lifecycle::InputStatus;
        let mut state = crate::group::model::tests::fixture(2);
        let source = state.plan().input.unwrap();
        let receiver = state
            .members
            .keys()
            .find(|key| *key != &source)
            .unwrap()
            .clone();
        state.active_input = Some(receiver.clone());
        state.activation = 5;
        state.members.get_mut(&receiver).unwrap().input_ready = 5;
        let active = desired(&state, &source, &[], true).unwrap()["input"].clone();
        assert!(active.input_host() && active.activate);
        let mut switched = active.switched_after_generation(Some(InputStatus::Peer), true);
        assert!(switched && !active.return_host_input(switched));

        state.activation = 6;
        let waiting = desired(&state, &source, &[], true).unwrap()["input"].clone();
        assert!(!waiting.activate && equivalent(&active, &waiting));
        switched = waiting.switched_after_generation(Some(InputStatus::Peer), switched);
        assert!(switched && !waiting.return_host_input(switched));

        let member = state.members.get_mut(&receiver).unwrap();
        member.input_ready = 6;
        member.node.monitor_error = Some("DDCRC_RETRIES(EIO)".into());
        let failed = desired(&state, &source, &[], true).unwrap()["input"].clone();
        assert!(!failed.activate && equivalent(&active, &failed));
        switched = failed.switched_after_generation(Some(InputStatus::Peer), switched);
        assert!(switched && !failed.return_host_input(switched));

        state.active_input = Some(source.clone());
        state.activation = 7;
        let returned = desired(&state, &source, &[], true).unwrap()["input"].clone();
        assert!(equivalent(&active, &returned));
        switched = returned.switched_after_generation(Some(InputStatus::Peer), switched);
        assert!(returned.return_host_input(switched));
        switched = returned.switched_after_generation(Some(InputStatus::Here), false);
        assert!(!returned.return_host_input(switched));
    }

    #[test]
    fn scoped_ipv6_peers_keep_their_interfaces_for_input_audio_and_microphone_links() {
        let mut state = crate::group::model::tests::fixture(4);
        state.settings.microphone_enabled = true;
        state.bind_microphone();
        for (index, member) in state.members.values_mut().enumerate() {
            member.address = SocketAddr::V6(SocketAddrV6::new(
                "fe80::1".parse::<Ipv6Addr>().unwrap(),
                46_000 + index as u16,
                0,
                11 + index as u32,
            ));
        }
        let source = state.plan().input.unwrap();
        let output = state.plan().audio.unwrap();
        state.active_input = Some(output.clone());
        let inputs = desired(&state, &source, &[], true).unwrap();
        let target = inputs["input"].args[3].parse::<SocketAddr>().unwrap();
        assert_eq!(target, {
            let mut address = state.members[&output].address;
            address.set_port(workspace::INPUT_PORT);
            address
        });
        let outputs = desired(&state, &output, &[], true).unwrap();
        for spec in outputs
            .values()
            .filter(|spec| spec.kind == Kind::AudioReceive)
        {
            let address = spec.args[2].parse::<SocketAddr>().unwrap();
            assert_eq!(address, {
                let mut address = state.members[&spec.peer].address;
                address.set_port(workspace::AUDIO_PORT);
                address
            });
        }
        let microphone = &inputs["microphone-receive"];
        assert_eq!(microphone.args[2].parse::<SocketAddr>().unwrap(), {
            let mut address = state.members[&microphone.peer].address;
            address.set_port(microphone_port(&state, &source).unwrap());
            address
        });
    }

    #[test]
    fn input_targets_wait_for_matching_monitor_generation_and_keep_one_native_link() {
        let mut state = crate::group::model::tests::fixture(4);
        let source = state.plan().input.unwrap();
        state.activation = 7;
        state.settings.edge = true;
        state.active_input_entry = Some(32_767);
        for target in state
            .members
            .keys()
            .filter(|key| *key != &source)
            .cloned()
            .collect::<Vec<_>>()
        {
            state.active_input = Some(target.clone());
            let waiting = desired(&state, &source, &[], true).unwrap();
            let spec = &waiting["input"];
            assert!(spec.desired_remote && !spec.activate);
            assert_eq!(spec.peer, target);
            assert_eq!(
                spec.args[3],
                endpoint(&state.members[&target], workspace::INPUT_PORT)
            );
            assert_eq!(spec.entry, Some(32_767));
            state.members.get_mut(&target).unwrap().input_ready = state.activation;
            let ready = desired(&state, &source, &[], true).unwrap();
            assert!(ready["input"].activate);
            assert!(equivalent(spec, &ready["input"]));
            state.members.get_mut(&target).unwrap().node.monitor_error =
                Some("DDC command failed".into());
            assert!(
                !desired(&state, &source, &[], true).unwrap()["input"].activate,
                "a terminal monitor failure acknowledges completion without granting remote input"
            );
            state.members.get_mut(&target).unwrap().node.monitor_error = None;
            for member in state
                .members
                .values()
                .filter(|member| member.node.key != source)
            {
                let receiver = desired(&state, &member.node.key, &[], true).unwrap();
                let spec = &receiver["input"];
                assert_eq!(spec.peer, source);
                assert_eq!(spec.args[2], "receiver");
                assert_eq!(spec.args[3], "[::]:45876");
                assert!(!spec.activate && !spec.desired_remote);
            }
            state.activation += 1;
            let next = desired(&state, &source, &[], true).unwrap();
            assert!(!next["input"].activate);
            assert!(equivalent(&ready["input"], &next["input"]));
        }
    }

    #[test]
    fn mixer_and_input_toggles_preserve_unrelated_streams_but_device_changes_restart_them() {
        let mut state = crate::group::model::tests::fixture(4);
        state.settings.microphone_enabled = true;
        state.bind_microphone();
        let output = state.plan().audio.unwrap();
        let before = desired(&state, &output, &[], true).unwrap();
        state.settings.keyboard_enabled = false;
        state.settings.mouse_enabled = false;
        state.settings.edge = true;
        for member in state.members.values_mut() {
            member.preferences.audio = AudioLevel {
                volume: 27,
                muted: true,
            };
        }
        state.settings.microphone_level.volume = 43;
        state.members.get_mut(&output).unwrap().preferences.output = Some(AudioLevel {
            volume: 81,
            muted: false,
        });
        let after = desired(&state, &output, &[], false).unwrap();
        for (name, spec) in &before {
            if spec.kind != Kind::Input {
                assert!(
                    equivalent(spec, &after[name]),
                    "input sharing restarted {name}"
                );
            }
        }
        state
            .members
            .get_mut(&output)
            .unwrap()
            .preferences
            .devices
            .speaker = Some("replacement-headphones".into());
        state
            .members
            .get_mut(&output)
            .unwrap()
            .node
            .devices
            .speakers
            .push(crate::audio::AudioDevice {
                id: "replacement-headphones".into(),
                name: "Replacement headphones".into(),
                default: false,
            });
        let changed = desired(&state, &output, &[], false).unwrap();
        for (name, spec) in &after {
            if spec.kind == Kind::AudioReceive {
                assert!(
                    !equivalent(spec, &changed[name]),
                    "output device was ignored for {name}"
                );
            }
        }
    }

    #[test]
    fn native_profile_keeps_identity_permissions_and_token_when_switching_between_three_targets() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "desk-switch-group-profile-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        let identity = config::create_identity(&root).unwrap();
        config::write_atomic(&root.join("wayland-restore-token"), "existing-portal-grant").unwrap();
        let mut state = crate::group::model::tests::fixture(4);
        state.settings.edge = true;
        let source = state.plan().input.unwrap();
        let endpoint = InputEndpoint {
            info: crate::audio::AudioDevice {
                id: "combined-hid".into(),
                name: "Fixture HID".into(),
                default: true,
            },
            path: root.join("fixture-event"),
            #[cfg(target_os = "linux")]
            generation: None,
            keyboard: true,
            mouse: true,
        };
        let inputs = vec![endpoint.clone(), endpoint];
        for target in state
            .members
            .keys()
            .filter(|key| *key != &source)
            .cloned()
            .collect::<Vec<_>>()
        {
            state.active_input = Some(target.clone());
            let wanted = desired(&state, &source, &inputs, true).unwrap();
            assert_eq!(wanted["input"].capture.len(), 1);
            let profile = configure(&root, &wanted["input"]).unwrap();
            assert_eq!(profile, root.join("group-services").join("input"));
            assert_eq!(
                config::load_identity(&profile).unwrap().public,
                identity.public
            );
            assert_eq!(config::hex(&config::load_peer(&profile).unwrap()), target);
            let settings = config::read_settings(&profile).unwrap();
            assert!(
                settings.monitor.is_none(),
                "native workers must not send another DDC command"
            );
            assert!(
                settings.edge.is_none(),
                "groups must not enable the legacy right-edge cycle"
            );
            assert_eq!(settings.routes, Some(wanted["input"].routes.clone()));
            assert_eq!(
                std::fs::read_to_string(profile.join("wayland-restore-token")).unwrap(),
                "existing-portal-grant"
            );
            assert!(!root.join("wayland-restore-token").exists());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                for path in [
                    profile.join("identity"),
                    profile.join("peer"),
                    profile.join("settings.toml"),
                    profile.join("wayland-restore-token"),
                ] {
                    assert_eq!(
                        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                        0o600
                    );
                }
                assert_eq!(
                    std::fs::metadata(profile).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
        }
        config::write_atomic(&root.join("wayland-restore-token"), "stale-original-grant").unwrap();
        let wanted = desired(&state, &source, &inputs, true).unwrap();
        let profile = configure(&root, &wanted["input"]).unwrap();
        assert_eq!(
            std::fs::read_to_string(profile.join("wayland-restore-token")).unwrap(),
            "existing-portal-grant"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn four_computers_generate_real_worker_arguments_for_audio_and_microphone_fanout() {
        let mut state = crate::group::model::tests::fixture(4);
        state.settings.microphone_enabled = true;
        state.bind_microphone();
        let plan = state.plan();
        let output = plan.audio.unwrap();
        let microphone = plan.microphone.unwrap().computer;
        let listeners = desired(&state, &output, &[], true).unwrap();
        assert_eq!(
            listeners
                .values()
                .filter(|spec| spec.kind == Kind::AudioReceive)
                .count(),
            3
        );
        let senders = desired(&state, &microphone, &[], true).unwrap();
        assert_eq!(
            senders
                .values()
                .filter(|spec| spec.kind == Kind::MicrophoneSend)
                .count(),
            3
        );
        let ports = senders
            .values()
            .filter(|spec| spec.kind == Kind::MicrophoneSend)
            .map(|spec| spec.args[2].clone())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ports.len(), 3);
        for member in state
            .members
            .values()
            .filter(|member| member.node.key != microphone)
        {
            let receiver = desired(&state, &member.node.key, &[], true).unwrap();
            let spec = &receiver["microphone-receive"];
            let port = spec.args[2].parse::<SocketAddr>().unwrap().port();
            assert!(ports.contains(&format!("[::]:{port}")));
        }
    }
}
