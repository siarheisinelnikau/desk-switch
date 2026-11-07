mod group;
mod group_compat;
mod group_data;
#[cfg(target_os = "linux")]
mod linux_activation;
mod management;
mod model;
mod paired_update;
mod popup;
#[cfg(debug_assertions)]
mod preview;
mod session;
#[cfg(windows)]
mod software;
mod tray;
mod view;
mod worker;
#[cfg(debug_assertions)]
pub use preview::run as run_preview;

#[cfg(debug_assertions)]
use crate::lifecycle::InputStatus;
use crate::{config, platform::MonitorInfo};
use anyhow::{Context, Result};
use eframe::egui;
use model::{Form, Preferences};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
use worker::{Kind, Worker};

#[derive(Clone, Copy, PartialEq)]
pub enum Action {
    Show,
    Switch,
    MonitorSwitch,
    MonitorSetup,
    Emergency,
    Sound,
    Quit,
    Hide,
    QuickOpen { x: i32, y: i32 },
    QuickClose,
    Quick(popup::Command),
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Overview,
    Application,
    Diagnostics,
}

#[derive(Clone, Copy)]
enum QueryKind {
    Monitors,
    #[cfg(target_os = "linux")]
    InputAccess,
}

pub struct Desktop {
    group: group::Group,
    ctx: egui::Context,
    dir: PathBuf,
    prefs: Preferences,
    form: Form,
    page: Page,
    public_key: String,
    peer_key: String,
    input: Option<Worker>,
    monitor: Option<Worker>,
    audio: Option<Worker>,
    microphone: Option<Worker>,
    query: Option<(QueryKind, Worker)>,
    monitors: Vec<MonitorInfo>,
    notice: Option<(String, bool)>,
    device_notice: Option<(Kind, String)>,
    tray: Option<tray::Tray>,
    tray_error: Option<String>,
    tray_snapshot: tray::State,
    quick: popup::Popup,
    monitor_setup_open: bool,
    actions: Receiver<Action>,
    hidden: bool,
    start_hidden: bool,
    quitting: bool,
    last_switch: Option<Instant>,
    startup_pending: bool,
    startup_valid: bool,
    update_job: Option<crate::updater::Job>,
    update_status: String,
    update_ready: bool,
    update_version: Option<String>,
    update_channel_version: Option<String>,
    update_failed: bool,
    update_confirmation: bool,
    paired_update: Option<crate::workspace::update::Coordinator>,
    last_update_check: Option<Instant>,
    last_version_error: Option<String>,
    prepared_update: Option<crate::updater::Prepared>,
    candidate_update: Option<crate::updater::Candidate>,
    download_action: Option<paired_update::DownloadAction>,
    install_job: Option<crate::updater::InstallJob>,
    install_pending: bool,
    cable_open: bool,
    cable_remote: bool,
    cable_platform: crate::cable::Platform,
    cable_job: Option<crate::cable::Job>,
    cable_reply: Option<u64>,
    cable_result: Option<Result<PathBuf, String>>,
    cable_remote_done: bool,
    settings_remote: bool,
    management_pending: Option<management::Pending>,
    peer_restart: Option<(String, Instant)>,
    peer_update_status: Option<(String, bool)>,
    presented: bool,
    discovery: Option<crate::discovery::Discovery>,
    discovered: std::collections::BTreeMap<String, crate::discovery::Peer>,
    discovery_error: Option<String>,
    pair: session::Pair,
    applied_mix: String,
    applied_microphone_volume: Option<u8>,
    quitting_since: Option<Instant>,
    #[cfg(debug_assertions)]
    preview: Option<crate::workspace::Node>,
    #[cfg(debug_assertions)]
    preview_scroll: f32,
    #[cfg(debug_assertions)]
    preview_popup: bool,
}

pub fn run(start_hidden: bool) -> Result<()> {
    run_at(start_hidden, config::state_dir()?)
}

pub fn run_at(start_hidden: bool, dir: PathBuf) -> Result<()> {
    anyhow::ensure!(dir.is_absolute(), "путь профиля должен быть абсолютным");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir)?;
    let _lock =
        config::lock(&dir, "desktop.lock").context("Desk Switch уже открыт для этого профиля")?;
    let icon = egui::IconData {
        rgba: tray::icon_rgba(64),
        width: 64,
        height: 64,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Desk Switch")
            .with_app_id("com.deskswitch.Desktop")
            .with_inner_size([1080.0, 760.0])
            .with_min_inner_size([880.0, 650.0])
            .with_icon(icon),
        ..Default::default()
    };
    #[cfg(windows)]
    return software::run(options.viewport, move |cc| {
        Ok(Box::new(Desktop::new(
            cc.egui_ctx.clone(),
            dir,
            start_hidden,
        )))
    })
    .map_err(anyhow::Error::msg);
    #[cfg(not(windows))]
    eframe::run_native(
        "Desk Switch",
        options,
        Box::new(move |cc| {
            Ok(Box::new(Desktop::new(
                cc.egui_ctx.clone(),
                dir,
                start_hidden,
            )))
        }),
    )
    .map_err(|error| anyhow::anyhow!("открытие окна Desk Switch: {error}"))
}

impl Desktop {
    fn new(ctx: egui::Context, dir: PathBuf, start_hidden: bool) -> Self {
        let (prefs, prefs_error) = match Preferences::load(&dir) {
            Ok(prefs) => (prefs, None),
            Err(error) => (Preferences::default(), Some(format!("{error:#}"))),
        };
        crate::i18n::set_language(&prefs.language);
        view::style(&ctx, &prefs.theme);
        let (document, settings_error) = match config::read_settings(&dir) {
            Ok(document) => (document, None),
            Err(error) => (config::FileSettings::default(), Some(format!("{error:#}"))),
        };
        let public_key = config::load_identity(&dir)
            .map(|key| config::hex(&key.public))
            .unwrap_or_default();
        let peer_key = config::load_peer(&dir)
            .map(|key| config::hex(&key))
            .unwrap_or_default();
        let (send, actions) = mpsc::channel();
        let startup_valid = prefs_error.is_none() && settings_error.is_none();
        let quick = popup::Popup::new(send.clone());
        let (tray, tray_error) = match tray::Tray::new(send, ctx.clone()) {
            Ok(tray) => (Some(tray), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        Self {
            group: Default::default(),
            ctx,
            dir,
            prefs,
            form: Form::from_document(document),
            page: Page::Overview,
            public_key,
            peer_key,
            input: None,
            monitor: None,
            audio: None,
            microphone: None,
            query: None,
            monitors: Vec::new(),
            notice: prefs_error.or(settings_error).map(|error| (error, true)),
            device_notice: None,
            tray,
            tray_error,
            tray_snapshot: tray::State::default(),
            quick,
            monitor_setup_open: false,
            actions,
            hidden: false,
            start_hidden,
            quitting: false,
            last_switch: None,
            startup_pending: true,
            startup_valid,
            update_job: None,
            update_status: "Проверка ещё не выполнялась.".into(),
            update_ready: false,
            update_version: None,
            update_channel_version: None,
            update_failed: false,
            update_confirmation: false,
            paired_update: None,
            last_update_check: None,
            last_version_error: None,
            prepared_update: None,
            candidate_update: None,
            download_action: None,
            install_job: None,
            install_pending: false,
            cable_open: false,
            cable_remote: false,
            cable_platform: Default::default(),
            cable_job: None,
            cable_reply: None,
            cable_result: None,
            cable_remote_done: false,
            settings_remote: false,
            management_pending: None,
            peer_restart: None,
            peer_update_status: None,
            presented: false,
            discovery: None,
            discovered: std::collections::BTreeMap::new(),
            discovery_error: None,
            pair: session::Pair::default(),
            applied_mix: String::new(),
            applied_microphone_volume: None,
            quitting_since: None,
            #[cfg(debug_assertions)]
            preview: None,
            #[cfg(debug_assertions)]
            preview_scroll: 0.0,
            #[cfg(debug_assertions)]
            preview_popup: false,
        }
    }

    fn input_running(&self) -> bool {
        self.input.as_ref().is_some_and(Worker::running)
    }

    fn apply_input_monitor_requirement(&mut self) {
        if let Some(worker) = &mut self.input
            && worker.running()
            && !worker.stopping
            && let Err(error) = worker.require_monitor(self.prefs.workspace.shared.monitor_enabled)
        {
            self.notice = Some((error.to_string(), true));
        }
    }
    fn audio_running(&self) -> bool {
        self.audio.as_ref().is_some_and(Worker::running)
    }
    fn monitor_running(&self) -> bool {
        self.monitor.as_ref().is_some_and(Worker::running)
    }
    fn microphone_running(&self) -> bool {
        self.microphone.as_ref().is_some_and(Worker::running)
    }
    fn connected(&self) -> bool {
        #[cfg(debug_assertions)]
        if let Some(node) = &self.preview {
            return self.pair.synced && node.input.is_some();
        }
        self.input.as_ref().is_some_and(Worker::connected)
    }

    fn save(&mut self) -> Result<()> {
        config::save_settings(&self.dir, &self.form.document()?)?;
        self.prefs.save(&self.dir)?;
        Ok(())
    }

    fn message(&mut self, result: Result<()>, success: &str) {
        self.notice = Some(match result {
            Ok(()) => (success.into(), false),
            Err(error) => (format!("{error:#}"), true),
        });
    }

    fn start_input(&mut self) -> Result<()> {
        self.save()?;
        config::load_identity(&self.dir).context("сначала создайте ключ этого компьютера")?;
        config::load_peer(&self.dir)
            .context("сначала выполните сопряжение с другим компьютером")?;
        let args = self.prefs.input_args()?;
        self.pair.monitor_input = None;
        let mut worker = Worker::start(Kind::Input, &args, &self.dir, &self.ctx)?;
        worker.require_monitor(self.prefs.workspace.shared.monitor_enabled)?;
        worker.monitoring = self.form.monitor_enabled;
        self.input = Some(worker);
        Ok(())
    }

    fn toggle_audio(&mut self) {
        if let Some(state) = self.group.workspace.clone() {
            let mut value = state.settings;
            value.audio_enabled = !value.audio_enabled;
            self.group_action(crate::group::net::Action::Settings {
                expected: state.revision,
                value,
            });
            return;
        }
        self.prefs.workspace.shared.audio_enabled = !self.prefs.workspace.shared.audio_enabled;
        self.shared_changed();
    }

    fn switch(&mut self) {
        if self.group.workspace.is_some() {
            self.group_cycle();
            return;
        }
        if self.prefs.workspace.shared.monitor_enabled
            && self.pair.plan.as_ref().is_none_or(|p| p.monitor.is_none())
            && !self
                .pair
                .remote
                .as_ref()
                .is_some_and(|r| crate::workspace::Plan::input_in_use(&self.local_node(), r))
        {
            self.notice = Some((
                "Remember the video connection on both PCs before switching the shared monitor."
                    .into(),
                true,
            ));
            return;
        }
        #[cfg(debug_assertions)]
        if let Some(local) = &mut self.preview {
            local.input = Some(if local.input == Some(InputStatus::Here) {
                InputStatus::Peer
            } else {
                InputStatus::Here
            });
            if let Some(remote) = &mut self.pair.remote {
                remote.input = Some(if local.input == Some(InputStatus::Here) {
                    InputStatus::Peer
                } else {
                    InputStatus::Here
                });
            }
            return;
        }
        if !self.connected()
            || self
                .last_switch
                .is_some_and(|time| time.elapsed() < Duration::from_millis(800))
        {
            return;
        }
        let result = self.input.as_mut().unwrap().command("switch");
        self.last_switch = Some(Instant::now());
        self.message(
            result,
            "Запрос переключения отправлен. Отпустите удерживаемые клавиши и кнопки мыши.",
        );
    }

    fn monitor_switch_ready(&self) -> bool {
        if !self.pair.synced {
            return false;
        }
        let local = self.local_node();
        let Some(_inputs) = self
            .pair
            .remote
            .as_ref()
            .and_then(|r| crate::workspace::Plan::monitor_inputs(&local, r))
        else {
            return false;
        };
        if self
            .pair
            .plan
            .as_ref()
            .is_some_and(|p| p.input_source.is_some())
        {
            self.connected() && self.pair.plan.as_ref().is_some_and(|p| p.monitor.is_some())
        } else {
            if !self.pair.remote.as_ref().is_some_and(|remote| {
                crate::workspace::Plan::monitor_only_available(&local, remote)
            }) {
                return false;
            }
            #[cfg(debug_assertions)]
            if self.preview.is_some() {
                return true;
            }
            self.monitor
                .as_ref()
                .is_some_and(|worker| worker.connected())
        }
    }

    fn switch_input_for_setup(&mut self) {
        if !self.connected() || !self.pair.synced {
            return;
        }
        #[cfg(debug_assertions)]
        if self.preview.is_some() {
            self.notice = Some(("Select the other PC's input in the monitor menu, then remember its connection there. Return input with F11; use the monitor menu to return the picture during this first setup.".into(), false));
            return;
        }
        let result = self
            .input
            .as_mut()
            .context("input service is not ready")
            .and_then(|w| w.command("setup-switch"));
        self.message(result, "Select the other PC's input in the monitor menu, then remember its connection there. Return input with F11; use the monitor menu to return the picture during this first setup.");
    }

    fn input_switch_ready(&self) -> bool {
        self.connected()
            && self.pair.synced
            && (!self.prefs.workspace.shared.monitor_enabled
                || self.pair.plan.as_ref().is_some_and(|p| p.monitor.is_some())
                || self
                    .pair
                    .remote
                    .as_ref()
                    .is_some_and(|r| crate::workspace::Plan::input_in_use(&self.local_node(), r)))
    }

    fn tray_state(&self) -> tray::State {
        if let Some(workspace) = &self.group.workspace {
            let available = workspace
                .members
                .values()
                .filter(|member| member.compatible())
                .count()
                > 1;
            return tray::State {
                connected: available && !workspace.switch_pending(),
                emergency: self.group.data.input.is_some(),
                sound: workspace.settings.audio_enabled,
                monitor: false,
                combined: workspace.settings.monitor_enabled,
            };
        }
        tray::State {
            connected: self.input_switch_ready(),
            emergency: self.input_running(),
            sound: self.audio_running(),
            monitor: self.monitor_switch_ready(),
            combined: self.pair.plan.as_ref().is_some_and(|p| p.monitor.is_some()),
        }
    }

    fn switch_monitor(&mut self, ctx: &egui::Context) {
        if self.group.workspace.is_some() {
            self.group_cycle();
            return;
        }
        if !self.monitor_switch_ready() {
            self.action(Action::MonitorSetup, ctx);
        } else if self.connected() {
            self.switch();
        } else if self
            .last_switch
            .is_none_or(|time| time.elapsed() >= Duration::from_millis(800))
        {
            self.last_switch = Some(Instant::now());
            let result = self
                .monitor
                .as_mut()
                .context("shared monitor service is not ready")
                .and_then(|worker| worker.command("switch"));
            self.message(result, "Shared monitor switch requested. Use Desk Switch in the other PC's tray to switch back.");
        }
    }

    fn action(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Show => {
                #[cfg(target_os = "linux")]
                linux_activation::request(linux_activation::Role::Main);
                self.quick.close(ctx);
                self.hidden = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            Action::Switch => self.switch(),
            Action::MonitorSwitch => self.switch_monitor(ctx),
            Action::MonitorSetup => {
                if self.group.workspace.is_some() {
                    self.group.add_open = false;
                    self.group.selected = self.public_key.clone();
                    self.page = Page::Application;
                } else {
                    self.page = Page::Overview;
                    self.monitor_setup_open = true;
                }
                self.action(Action::Show, ctx);
            }
            Action::Emergency => {
                if self.group.workspace.is_some() {
                    if let Err(error) = self.group.data.emergency() {
                        self.notice = Some((error.to_string(), true));
                    }
                    self.group.return_pending = true;
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                    return;
                }
                if let Some(input) = &mut self.input {
                    let result = input.command("emergency");
                    self.message(result, "Запрошен аварийный возврат управления.");
                }
            }
            Action::Sound => self.toggle_audio(),
            Action::QuickOpen { x, y } => self.quick.open(ctx, x, y),
            Action::QuickClose => self.quick.close(ctx),
            Action::Quick(command) => self.quick_action(command),
            Action::Hide => {
                if self.tray.as_ref().is_some_and(tray::Tray::available) {
                    self.hidden = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
            }
            Action::Quit => {
                self.quick.close(ctx);
                if self.install_job.is_some() || self.install_pending {
                    self.notice = Some(("Installing the verified update…".into(), false));
                    self.hidden = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    return;
                }
                self.quitting = true;
                self.quitting_since.get_or_insert_with(Instant::now);
                self.group.data.stop();
                self.group.return_pending = false;
                self.group.service.take();
                self.group.clipboard.take();
                self.group.probe.take();
                self.group.monitor_job.take();
                self.group.monitor_wait = None;
                self.stop_pair();
                if let Some(input) = &mut self.input {
                    input.stop();
                }
                if let Some(audio) = &mut self.audio {
                    audio.stop();
                }
                self.query.take();
                self.discovery.take();
            }
        }
    }

    fn close_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.action(
            if self.tray.as_ref().is_some_and(tray::Tray::available) {
                Action::Hide
            } else {
                Action::Quit
            },
            ctx,
        );
    }

    fn scan(&mut self, kind: QueryKind) {
        #[cfg(debug_assertions)]
        if self.preview.is_some() {
            return;
        }
        if self.query.is_some() {
            return;
        }
        let args: Vec<String> = match kind {
            #[cfg(target_os = "linux")]
            QueryKind::InputAccess => {
                let selection = self.input_access_selection();
                self.pair.input_access_attempts.insert(selection);
                let mut args = vec!["input-access".into()];
                args.extend(self.input_permission_paths());
                args
            }
            QueryKind::Monitors => vec![
                "monitor-read".into(),
                self.prefs.workspace.monitor.id.clone(),
            ],
        };
        match Worker::start(Kind::Query, &args, &self.dir, &self.ctx) {
            Ok(worker) => self.query = Some((kind, worker)),
            Err(error) => self.notice = Some((format!("{error:#}"), true)),
        }
    }

    #[cfg(target_os = "linux")]
    fn input_access_selection(&self) -> String {
        input_generation(&self.input_permission_paths())
    }

    fn tick(&mut self, ctx: &egui::Context) {
        #[cfg(debug_assertions)]
        if self.preview.is_some() {
            self.preview_tick(ctx);
            return;
        }
        if self.presented {
            crate::updater::acknowledge_shared_start(&self.dir);
        }
        if self.startup_pending {
            self.startup_pending = false;
            self.startup(ctx);
        }
        for _ in 0..32 {
            let Ok(action) = self.actions.try_recv() else {
                break;
            };
            self.action(action, ctx);
        }
        for (worker, failed) in [
            (&mut self.input, &mut self.pair.input_failed),
            (&mut self.monitor, &mut self.pair.monitor_failed),
            (&mut self.audio, &mut self.pair.audio_failed),
            (&mut self.microphone, &mut self.pair.microphone_failed),
        ] {
            let Some(worker) = worker else { continue };
            if worker.poll() {
                *failed = true;
                let error = worker
                    .logs
                    .back()
                    .cloned()
                    .unwrap_or_else(|| "Служба завершилась с ошибкой.".into());
                self.device_notice = Some((worker.kind, error.clone()));
                self.notice = Some((error, true));
            }
            if let Some(error) = worker.new_switch_error.take() {
                self.notice = Some((format!("Switch failed: {error}"), true));
            }
        }
        if let Some(level) = self
            .audio
            .as_mut()
            .filter(|worker| worker.kind == Kind::AudioReceive && !worker.stopping)
            .and_then(|worker| worker.output_level.changed.take())
            && self.prefs.workspace.shared.audio_level(&self.public_key) != level
        {
            self.prefs
                .workspace
                .shared
                .audio_levels
                .insert(self.public_key.clone(), level);
            self.shared_changed();
        }
        self.tick_pair(ctx);
        #[cfg(target_os = "linux")]
        self.request_input_access_if_needed();
        if let Some((kind, error)) = &self.device_notice {
            let ready = [&self.input, &self.monitor, &self.audio, &self.microphone]
                .into_iter()
                .flatten()
                .find(|worker| worker.kind == *kind)
                .is_some_and(|worker| {
                    if *kind == Kind::Input {
                        worker.connected()
                    } else if *kind == Kind::Monitor {
                        worker.running() && worker.monitor_input.is_some()
                    } else {
                        worker.running() && worker.audio_playing
                    }
                });
            if ready {
                if self
                    .notice
                    .as_ref()
                    .is_some_and(|(message, failed)| *failed && message == error)
                {
                    self.notice = None;
                }
                self.device_notice = None;
            }
        }
        let version_error = self
            .pair
            .remote
            .as_ref()
            .and_then(|remote| {
                crate::compatibility::Version::current()
                    .require(remote.version.as_ref())
                    .err()
            })
            .map(|error| error.to_string())
            .or_else(|| {
                [&self.input, &self.audio, &self.microphone]
                    .into_iter()
                    .flatten()
                    .find_map(|worker| worker.version_error.clone())
            });
        if let Some(error) = &version_error
            && self.last_version_error.as_ref() != Some(error)
        {
            self.notice = Some((error.clone(), true));
            self.action(Action::Show, ctx);
            if self.prefs.auto_update
                && self
                    .last_update_check
                    .is_none_or(|time| time.elapsed() > Duration::from_secs(60))
            {
                self.check_updates(ctx);
            }
        }
        self.last_version_error = version_error;
        if let Some(result) = self.update_job.as_ref().and_then(crate::updater::Job::poll) {
            self.update_job = None;
            let download_action = self.download_action.take();
            let downloaded = matches!(
                &result,
                Ok(crate::updater::Outcome::Ready(_) | crate::updater::Outcome::Prepared(..))
            );
            match result {
                Ok(crate::updater::Outcome::Current(version)) => {
                    self.update_channel_version = Some(version.clone());
                    self.update_failed = false;
                    self.update_ready = false;
                    self.update_version = None;
                    self.prepared_update = None;
                    self.candidate_update = None;
                    self.update_status = if version == crate::compatibility::VERSION {
                        "Установлена актуальная версия.".into()
                    } else {
                        format!(
                            "Installed version is newer than the latest signed release ({version})."
                        )
                    };
                    self.fail_paired_download(
                        "The requested signed update is not available on this PC.".into(),
                        ctx,
                    );
                }
                Ok(crate::updater::Outcome::Unpublished) => {
                    self.update_failed = false;
                    self.update_status = "Канал настроен. Релизы ещё не опубликованы.".into();
                    self.fail_paired_download("No signed release is available.".into(), ctx);
                }
                Ok(crate::updater::Outcome::Available(candidate)) => {
                    let version = candidate.version().to_owned();
                    if self.update_version.as_ref() != Some(&version) {
                        self.prepared_update = None;
                    }
                    self.update_failed = false;
                    self.update_channel_version = Some(version.clone());
                    self.update_version = Some(version.clone());
                    self.candidate_update = Some(candidate);
                    self.update_ready = true;
                    self.update_status = format!(
                        "Version {version} is available. Choose Download and install to update."
                    );
                }
                Ok(crate::updater::Outcome::Ready(version)) => {
                    self.update_failed = false;
                    self.update_channel_version = Some(version.clone());
                    self.update_version = Some(version.clone());
                    self.update_ready = true;
                    self.candidate_update = None;
                    self.update_status =
                        format!("Версия {version} проверена. Нажмите «Обновить и перезапустить».");
                    if self.paired_update.is_some() {
                        self.paired_download_ready(ctx);
                    }
                }
                Ok(crate::updater::Outcome::Prepared(version, prepared)) => {
                    self.update_failed = false;
                    self.update_channel_version = Some(version.clone());
                    self.update_version = Some(version.clone());
                    self.prepared_update = Some(prepared);
                    self.candidate_update = None;
                    self.update_ready = true;
                    self.update_status =
                        format!("Версия {version} проверена. Нажмите «Обновить и перезапустить».");
                    self.paired_download_ready(ctx);
                }
                Err(error) => {
                    self.update_failed = true;
                    self.update_status = format!("Не удалось проверить обновления: {error:#}");
                    self.fail_paired_download(format!("{error:#}"), ctx);
                }
            }
            if downloaded {
                match download_action {
                    Some(paired_update::DownloadAction::Install) => self.begin_install_update(ctx),
                    Some(paired_update::DownloadAction::Pair) => self.start_paired_update(ctx),
                    None => {}
                }
            }
        }
        if self.install_pending {
            self.install_pending = false;
            if let Some(prepared) = self.prepared_update.take() {
                match crate::updater::InstallJob::start(prepared, &self.dir, ctx.clone()) {
                    Ok(job) => self.install_job = Some(job),
                    Err(error) => {
                        self.update_failed = true;
                        self.update_status = error.to_string();
                        self.fail_paired_update(error.to_string(), ctx);
                    }
                }
            }
        }
        if let Some((prepared, result)) = self
            .install_job
            .as_ref()
            .and_then(crate::updater::InstallJob::poll)
        {
            self.install_job = None;
            match result {
                Ok(()) => {
                    if let Some(coordinator) = &mut self.paired_update {
                        let effects = coordinator.installed();
                        self.apply_update_effects(effects, ctx);
                    } else {
                        self.action(Action::Quit, ctx);
                    }
                }
                Err(error) => {
                    self.update_failed = true;
                    self.prepared_update = Some(prepared);
                    self.update_ready = true;
                    self.update_status = format!("Установка отменена или не удалась: {error}");
                    self.fail_paired_update(error, ctx);
                }
            }
        }
        self.tick_paired_update(ctx);
        self.tick_management(ctx);
        self.tick_cable(ctx);
        if let Some(service) = &self.pair.service {
            service.publish(self.local_node());
        }
        if self.prefs.auto_update
            && !self.quitting
            && self.update_job.is_none()
            && self
                .last_update_check
                .is_some_and(|time| time.elapsed() > Duration::from_secs(6 * 60 * 60))
        {
            self.check_updates(ctx);
        }
        if let Some((_, query)) = &mut self.query {
            query.poll();
        }
        if self
            .query
            .as_ref()
            .is_some_and(|(_, query)| query.settled())
        {
            let (kind, mut query) = self.query.take().unwrap();
            let result = (|| {
                let value =
                    query
                        .data
                        .take()
                        .context(query.logs.back().cloned().unwrap_or_else(|| {
                            "Устройства не ответили. Проверьте системные зависимости.".into()
                        }))?;
                match kind {
                    #[cfg(target_os = "linux")]
                    QueryKind::InputAccess => {
                        anyhow::ensure!(
                            value["input_access"] == true,
                            "input access was not granted"
                        );
                        anyhow::ensure!(
                            value["persistent"] == true,
                            "Install the current Debian package to enable saved input permissions."
                        );
                        self.retry_devices();
                    }
                    QueryKind::Monitors => {
                        let monitor: MonitorInfo = serde_json::from_value(value)?;
                        anyhow::ensure!(
                            monitor.id == self.prefs.workspace.monitor.id,
                            "Monitor settings changed while reading the current input. Try again."
                        );
                        {
                            anyhow::ensure!(
                                monitor.current.is_some_and(|input| input != 0),
                                "The monitor did not report a usable current input. No input was changed."
                            );
                            let mut saved = self.prefs.workspace.monitor.clone();
                            saved.input = monitor.current;
                            saved.fingerprint = monitor.fingerprint;
                            self.prefs
                                .workspace
                                .record_monitor(saved, !self.monitor_setup_is_local());
                            self.pair.monitor_input = monitor.current;
                            self.prefs.save(&self.dir)?;
                        }
                    }
                }
                Ok(())
            })();
            if matches!(kind, QueryKind::Monitors) {
                self.finish_monitor_read(&result, ctx);
            }
            {
                let success = match kind {
                    #[cfg(target_os = "linux")]
                    QueryKind::InputAccess => {
                        "Keyboard and mouse permission is saved, including after USB reconnection and restart."
                    }
                    _ => "Список устройств обновлён.",
                };
                self.message(result, success);
            }
        }
        let tray_available = self.tray.as_ref().is_some_and(tray::Tray::available);
        if self.hidden && !tray_available {
            self.action(Action::Show, ctx);
        }
        let snapshot = self.tray_state();
        if snapshot != self.tray_snapshot {
            if let Some(tray) = &mut self.tray {
                tray.update(snapshot);
            }
            self.tray_snapshot = snapshot;
        }
        if self.start_hidden {
            self.start_hidden = false;
            if tray_available {
                self.hidden = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }
        if ctx.input(|input| input.viewport().close_requested()) && !self.quitting {
            self.close_window(ctx);
        }
        if self
            .quitting_since
            .is_some_and(|time| time.elapsed() > Duration::from_secs(6))
        {
            self.group.data.force_stop();
            for worker in [
                &mut self.input,
                &mut self.monitor,
                &mut self.audio,
                &mut self.microphone,
            ]
            .into_iter()
            .flatten()
            {
                worker.force_stop();
            }
        }
        if self.quitting
            && self.group.data.quiet()
            && !self.input_running()
            && !self.monitor_running()
            && !self.audio_running()
            && !self.microphone_running()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Services wake the GUI on changed data. Only unfinished operations need
        // a short deadline; a settled window has no recurring render timer.
        if self.quitting
            || self.pair.busy
            || self
                .paired_update
                .as_ref()
                .is_some_and(|update| !update.done())
            || [&self.input, &self.monitor, &self.audio, &self.microphone]
                .into_iter()
                .flatten()
                .any(Worker::needs_poll)
        {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if let Some((_, worker)) = &self.query {
            ctx.request_repaint_after(worker.next_poll());
        }
        if let Some(since) = self.last_switch {
            let remaining = Duration::from_millis(800).saturating_sub(since.elapsed());
            if !remaining.is_zero() {
                ctx.request_repaint_after(remaining);
            }
        }
        if self.prefs.auto_update
            && let Some(since) = self.last_update_check
        {
            ctx.request_repaint_after(
                Duration::from_secs(6 * 60 * 60).saturating_sub(since.elapsed()),
            );
        }
    }

    fn appearance_changed(&mut self, ctx: &egui::Context) {
        crate::i18n::set_language(&self.prefs.language);
        view::style(ctx, &self.prefs.theme);
        if let Some(tray) = &mut self.tray {
            tray.update(self.tray_snapshot);
        }
        if let Err(error) = self.prefs.save(&self.dir) {
            self.notice = Some((error.to_string(), true));
        }
        ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
    }

    fn startup(&mut self, ctx: &egui::Context) {
        if !self.startup_valid {
            return;
        }
        let result = std::env::current_exe()
            .map_err(anyhow::Error::from)
            .and_then(|exe| crate::autostart::sync(self.prefs.autostart, &exe, &self.dir));
        if let Err(error) = result {
            self.notice = Some((format!("{error:#}"), true));
        }
        let managed =
            std::env::current_exe().is_ok_and(|path| crate::updater::shared_managed(&path));
        let failed_update = if managed && !cfg!(target_os = "macos") {
            None
        } else {
            std::fs::read_to_string(self.dir.join("updates/last-error.txt")).ok()
        };
        if let Some(error) = &failed_update {
            self.notice = Some((format!("Обновление не установлено: {error}"), true));
        }
        if self.prefs.auto_update && failed_update.is_none() {
            if !std::env::current_exe().is_ok_and(|path| crate::updater::shared_managed(&path))
                && let Ok(version) = crate::updater::staged(&self.dir)
            {
                self.update_ready = true;
                self.update_version = Some(version.clone());
                self.update_status =
                    format!("Версия {version} проверена. Нажмите «Обновить и перезапустить».");
            }
            self.check_updates(ctx);
        }
    }

    fn check_updates(&mut self, ctx: &egui::Context) {
        if self.update_job.is_some()
            || self.install_job.is_some()
            || self.install_pending
            || self.quitting
            || self.paired_update.is_some()
        {
            return;
        }
        let result = std::env::current_exe()
            .map_err(anyhow::Error::from)
            .and_then(|exe| crate::updater::Job::start(self.dir.clone(), exe, ctx.clone()));
        self.last_update_check = Some(Instant::now());
        match result {
            Ok(job) => {
                self.update_job = Some(job);
                self.update_failed = false;
                self.update_status = "Checking the signed release…".into();
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            Err(error) => {
                self.update_failed = true;
                self.update_status = error.to_string();
            }
        }
    }

    fn prepare_update(&mut self, version: String, ctx: &egui::Context) {
        if self.update_job.is_some()
            || self.install_job.is_some()
            || self.install_pending
            || self.quitting
        {
            self.download_action = None;
            self.fail_paired_download(
                "Another update operation is already in progress.".into(),
                ctx,
            );
            return;
        }
        let candidate = self
            .candidate_update
            .as_ref()
            .filter(|candidate| candidate.version() == version)
            .cloned();
        let result = std::env::current_exe()
            .map_err(anyhow::Error::from)
            .and_then(|exe| {
                crate::updater::Job::prepare(self.dir.clone(), exe, version, candidate, ctx.clone())
            });
        match result {
            Ok(job) => {
                self.update_job = Some(job);
                self.update_failed = false;
                self.update_status = "Downloading and verifying the signed update…".into();
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            Err(error) => {
                self.download_action = None;
                self.update_failed = true;
                self.update_status = error.to_string();
                self.fail_paired_download(error.to_string(), ctx);
            }
        }
    }

    fn begin_install_update(&mut self, ctx: &egui::Context) {
        if self.prepared_update.is_none()
            && let Some(candidate) = &self.candidate_update
        {
            let version = candidate.version().to_owned();
            self.download_action = Some(paired_update::DownloadAction::Install);
            self.prepare_update(version, ctx);
            return;
        }
        if self.prepared_update.is_some() {
            if let Err(error) = self.save() {
                self.notice = Some((error.to_string(), true));
                self.fail_paired_update(error.to_string(), ctx);
                return;
            }
            // Keep the pair and existing devices alive during installation.
            // Quit releases them only after the verified installer succeeds.
            self.install_pending = true;
            self.update_ready = false;
            self.update_status = "Installing the update…".into();
            return;
        }
        match crate::updater::launch_apply(&self.dir) {
            Ok(()) => {
                let _ = std::fs::remove_file(self.dir.join("updates/last-error.txt"));
                self.action(Action::Quit, ctx);
            }
            Err(error) => self.notice = Some((format!("{error:#}"), true)),
        }
    }
}

#[cfg(target_os = "linux")]
fn input_permission_error(line: &str) -> bool {
    line.contains("Permission denied") || line.contains("os error 13")
}

#[cfg(target_os = "linux")]
fn input_generation(paths: &[String]) -> String {
    use std::os::unix::fs::MetadataExt;
    paths
        .iter()
        .map(|path| {
            let inode = std::fs::metadata(path).map(|m| (m.dev(), m.ino())).ok();
            format!("{path}:{inode:?}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(all(test, target_os = "linux"))]
mod hotplug_tests {
    use super::*;
    #[test]
    fn an_unplugged_device_does_not_request_administrator_permission() {
        let context = "opening /dev/input/event2; grant this desktop user access to this selected event device during setup: ";
        assert!(!input_permission_error(&format!(
            "{context}No such file or directory (os error 2)"
        )));
        assert!(!input_permission_error(&format!(
            "{context}No such device (os error 19)"
        )));
        assert!(input_permission_error(&format!(
            "{context}Permission denied (os error 13)"
        )));
        assert!(input_permission_error("localized error text (os error 13)"));
    }
    #[test]
    fn a_recreated_event_path_is_a_new_input_generation_even_with_the_same_name() {
        let dir =
            std::env::temp_dir().join(format!("desk-switch-generation-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("event2");
        let replacement = dir.join("new-event");
        std::fs::write(&path, "old device").unwrap();
        let paths = [path.to_string_lossy().into_owned()];
        let old = input_generation(&paths);
        assert_eq!(old, input_generation(&paths));
        std::fs::write(&replacement, "replugged device").unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert_ne!(old, input_generation(&paths));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

impl eframe::App for Desktop {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.tick(ctx);
        self.quick_popup(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.render(ui);
        self.paired_update_dialog(ui.ctx());
        self.cable_dialog(ui.ctx());
        self.presented = true;
    }
}

#[cfg(windows)]
impl software::App for Desktop {
    fn logic(&mut self, ctx: &egui::Context) {
        self.tick(ctx);
        self.quick_popup(ctx);
    }
    fn ui(&mut self, ui: &mut egui::Ui) {
        self.render(ui);
        self.paired_update_dialog(ui.ctx());
        self.cable_dialog(ui.ctx());
        self.presented = true;
    }
}
