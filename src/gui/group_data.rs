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
    net::SocketAddr,
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
    entry: Option<u16>,
    generation: u64,
    desired_remote: bool,
    activate: bool,
    native: Option<AudioLevel>,
}
struct Task {
    spec: Spec,
    worker: Worker,
    switched: bool,
    was_remote: bool,
    cycling: bool,
    switch_failed: bool,
    restart_requested: bool,
    local_entry: Option<u64>,
}

fn equivalent(a: &Spec, b: &Spec) -> bool {
    let normalize = |spec: &Spec| {
        let mut spec = spec.clone();
        spec.activate = false;
        spec.native = None;
        spec.cursor = false;
        spec.entry = None;
        spec.generation = 0;
        spec.desired_remote = false;
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
    pub error: Option<String>,
    pub cycle: bool,
    pub cycle_position: Option<u16>,
    pub returned: bool,
    pub volume: Option<AudioLevel>,
    pub input: Option<crate::lifecycle::InputStatus>,
    pub audio_playing: bool,
    pub microphone_active: bool,
}

fn endpoint(member: &Member, port: u16) -> String {
    SocketAddr::new(member.address.ip(), port).to_string()
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

pub(super) fn desired(
    state: &Workspace,
    own: &str,
    inputs: &[InputEndpoint],
    cursor: bool,
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
        entry: None,
        generation: state.activation,
        desired_remote: false,
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
                "0.0.0.0:45876".into()
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
            let mut args = vec!["audio".into(), "serve".into(), "0.0.0.0:45877".into()];
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
                    format!("0.0.0.0:{}", microphone_port(state, &member.node.key)?),
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
            edge: spec.edge.then(|| config::EdgeSettings {
                mode: "input".into(),
                screen: Some("auto".into()),
                side: Some("right".into()),
            }),
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
        self.audio_playing = false;
        self.microphone_active = false;
        for (name, task) in &mut self.tasks {
            if task.worker.poll() {
                self.error = task.worker.logs.back().cloned();
                if task.spec.kind == Kind::Input && task.spec.desired_remote && !task.cycling {
                    self.returned = true;
                }
            }
            if desired
                .get(name)
                .is_some_and(|spec| spec.generation != task.spec.generation)
                && !task.worker.running()
            {
                task.restart_requested = true;
            }
            if task.worker.cycle_input {
                task.worker.cycle_input = false;
                task.cycling = true;
                self.cycle = true;
                self.cycle_position = task.worker.cycle_input_position.take();
            }
            if task.spec.kind == Kind::Input {
                self.input = task.worker.input_status;
                if task.spec.args.get(2).is_some_and(|role| role == "host") {
                    if task.worker.input_status == Some(crate::lifecycle::InputStatus::Peer) {
                        task.was_remote = true;
                    }
                    if task.was_remote
                        && task.worker.input_status == Some(crate::lifecycle::InputStatus::Here)
                        && !task.cycling
                        && !task.worker.stopping
                    {
                        self.returned = true;
                        task.was_remote = false;
                    }
                }
                if let Some(error) = task.worker.new_switch_error.take() {
                    task.switch_failed = true;
                    self.error = Some(error);
                    if task.spec.desired_remote && !task.cycling && !task.worker.stopping {
                        self.returned = true;
                    }
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
            if !desired
                .get(name)
                .is_some_and(|wanted| equivalent(&task.spec, wanted))
            {
                task.worker.stop();
            }
        }
        self.tasks.retain(|name, task| {
            !task.worker.settled()
                || (!task.restart_requested
                    && desired
                        .get(name)
                        .is_some_and(|spec| equivalent(&task.spec, spec)))
        });
        for (name, spec) in desired {
            if !self.tasks.contains_key(&name) {
                match configure(root, &spec)
                    .and_then(|dir| Worker::start(spec.kind, &spec.args, &dir, ctx))
                {
                    Ok(worker) => {
                        self.tasks.insert(
                            name.clone(),
                            Task {
                                spec: spec.clone(),
                                worker,
                                switched: false,
                                was_remote: false,
                                cycling: false,
                                switch_failed: false,
                                restart_requested: false,
                                local_entry: None,
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
            if task.spec.generation != spec.generation {
                task.spec.generation = spec.generation;
                task.cycling = false;
                task.switch_failed = false;
                // A new activation may retry a rejected switch, but must not
                // toggle an already active destination back to the source.
                task.switched = task.worker.input_status
                    == Some(crate::lifecycle::InputStatus::Peer)
                    || (!spec.desired_remote && task.switched);
            }
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
            if !spec.desired_remote
                && spec.edge
                && task.local_entry != Some(spec.generation)
                && task.worker.input_status == Some(crate::lifecycle::InputStatus::Here)
                && task.spec.args.get(2).is_some_and(|role| role == "host")
                && let Some(position) = spec.entry
                && task
                    .worker
                    .command(&format!("enter-edge {position}"))
                    .is_ok()
            {
                task.local_entry = Some(spec.generation);
            }
            task.spec.entry = spec.entry;
            if spec.activate
                && !task.switched
                && !task.switch_failed
                && task.worker.input_status == Some(crate::lifecycle::InputStatus::Here)
            {
                let command = spec
                    .entry
                    .filter(|_| spec.edge)
                    .map(|position| format!("switch-edge {position}"))
                    .unwrap_or_else(|| "switch".into());
                if let Err(error) = task.worker.command(&command) {
                    self.error = Some(error.to_string());
                    task.switch_failed = true;
                } else {
                    task.switched = true;
                }
            }
            if !spec.desired_remote && task.switched {
                let _ = task.worker.command("emergency");
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
                assert_eq!(spec.args[3], "0.0.0.0:45876");
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
            assert_eq!(settings.edge.unwrap().side.as_deref(), Some("right"));
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
            assert!(ports.contains(&format!("0.0.0.0:{port}")));
        }
    }
}
