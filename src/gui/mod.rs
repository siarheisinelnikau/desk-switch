mod model;
#[cfg(debug_assertions)]
mod preview;
mod session;
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
    MonitorStatus,
    #[cfg(target_os = "linux")]
    InputAccess,
}

pub struct Desktop {
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
    last_update_check: Option<Instant>,
    last_version_error: Option<String>,
    prepared_update: Option<rust_auto_updator::Prepared>,
    install_job: Option<crate::updater::InstallJob>,
    install_pending: bool,
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
        let (tray, tray_error) = match tray::Tray::new(send, ctx) {
            Ok(tray) => (Some(tray), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        Self {
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
            last_update_check: None,
            last_version_error: None,
            prepared_update: None,
            install_job: None,
            install_pending: false,
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
        }
    }

    fn input_running(&self) -> bool {
        self.input.as_ref().is_some_and(Worker::running)
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
        let mut worker = Worker::start(Kind::Input, &args, &self.dir)?;
        worker.monitoring = self.form.monitor_enabled;
        self.input = Some(worker);
        Ok(())
    }

    fn toggle_audio(&mut self) {
        self.prefs.workspace.shared.audio_enabled = !self.prefs.workspace.shared.audio_enabled;
        self.shared_changed();
    }

    fn switch(&mut self) {
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
        let Some(inputs) = self
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
                return local.monitor_input == Some(inputs.host);
            }
            self.monitor.as_ref().is_some_and(|worker| {
                worker.running() && !worker.stopping && worker.monitor_input == Some(inputs.host)
            })
        }
    }

    fn tray_state(&self) -> tray::State {
        tray::State {
            connected: self.connected() && self.pair.synced,
            emergency: self.input_running(),
            sound: self.audio_running(),
            monitor: self.monitor_switch_ready(),
            combined: self.pair.plan.as_ref().is_some_and(|p| p.monitor.is_some()),
        }
    }

    fn switch_monitor(&mut self, ctx: &egui::Context) {
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
                self.hidden = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            Action::Switch => self.switch(),
            Action::MonitorSwitch => self.switch_monitor(ctx),
            Action::MonitorSetup => {
                self.page = Page::Overview;
                self.monitor_setup_open = true;
                self.action(Action::Show, ctx);
            }
            Action::Emergency => {
                if let Some(input) = &mut self.input {
                    let result = input.command("emergency");
                    self.message(result, "Запрошен аварийный возврат управления.");
                }
            }
            Action::Sound => self.toggle_audio(),
            Action::Hide => {
                if self.tray.as_ref().is_some_and(tray::Tray::available) {
                    self.hidden = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
            }
            Action::Quit => {
                if self.install_job.is_some() || self.install_pending {
                    self.notice = Some((
                        "Устанавливаем обновление. На Ubuntu подтвердите системный запрос пароля."
                            .into(),
                        false,
                    ));
                    self.hidden = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    return;
                }
                self.quitting = true;
                self.quitting_since.get_or_insert_with(Instant::now);
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
                args.extend(self.form.capture_devices.lines().map(str::to_owned));
                args
            }
            QueryKind::Monitors | QueryKind::MonitorStatus => vec![
                "monitor-read".into(),
                self.prefs.workspace.monitor.id.clone(),
            ],
        };
        match Worker::start(Kind::Query, &args, &self.dir) {
            Ok(worker) => self.query = Some((kind, worker)),
            Err(error) => self.notice = Some((format!("{error:#}"), true)),
        }
    }

    #[cfg(target_os = "linux")]
    fn input_access_selection(&self) -> String {
        use std::os::unix::fs::MetadataExt;
        self.form
            .capture_devices
            .lines()
            .map(|path| {
                let inode = std::fs::metadata(path).map(|m| (m.dev(), m.ino())).ok();
                format!("{path}:{inode:?}")
            })
            .collect::<Vec<_>>()
            .join("\n")
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
        let version_error = [&self.input, &self.audio, &self.microphone]
            .into_iter()
            .flatten()
            .find_map(|worker| worker.version_error.clone());
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
            match result {
                Ok(crate::updater::Outcome::Current) => {
                    self.update_status = "Установлена актуальная версия.".into()
                }
                Ok(crate::updater::Outcome::Unpublished) => {
                    self.update_status = "Канал настроен. Релизы ещё не опубликованы.".into()
                }
                Ok(crate::updater::Outcome::Ready(version)) => {
                    self.update_ready = true;
                    self.update_status = format!(
                        "Версия {version} проверена и скачана. Установится при следующем запуске."
                    );
                    if self.prefs.auto_update
                        && !self.input_running()
                        && !self.monitor_running()
                        && !self.audio_running()
                        && !self.microphone_running()
                        && !self.quitting
                        && !self.dir.join("updates/last-error.txt").exists()
                    {
                        self.install_update(ctx);
                    }
                }
                Ok(crate::updater::Outcome::Prepared(version, prepared)) => {
                    self.prepared_update = Some(prepared);
                    self.update_ready = true;
                    self.update_status =
                        format!("Версия {version} проверена. Нажмите «Обновить и перезапустить».");
                }
                Err(error) => {
                    self.update_status = format!("Не удалось проверить обновления: {error:#}")
                }
            }
        }
        if self.install_pending
            && !self.input_running()
            && !self.monitor_running()
            && !self.audio_running()
            && !self.microphone_running()
        {
            self.install_pending = false;
            if let Some(prepared) = self.prepared_update.take() {
                match crate::updater::InstallJob::start(prepared, &self.dir, ctx.clone()) {
                    Ok(job) => self.install_job = Some(job),
                    Err(error) => self.update_status = error.to_string(),
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
                Ok(()) => self.action(Action::Quit, ctx),
                Err(error) => {
                    self.prepared_update = Some(prepared);
                    self.update_ready = true;
                    self.update_status = format!("Установка отменена или не удалась: {error}");
                }
            }
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
                        self.retry_devices();
                    }
                    QueryKind::Monitors => {
                        let monitor: MonitorInfo = serde_json::from_value(value)?;
                        if monitor.id == self.prefs.workspace.monitor.id {
                            anyhow::ensure!(
                                monitor.current.is_some_and(|input| input != 0),
                                "The monitor did not report a usable current input. No input was changed."
                            );
                            self.prefs.workspace.monitor.input = monitor.current;
                            self.pair.monitor_input = monitor.current;
                            self.prefs.workspace.monitor.fingerprint = monitor.fingerprint;
                            self.prefs.save(&self.dir)?;
                        }
                    }
                    QueryKind::MonitorStatus => {
                        let monitor: MonitorInfo = serde_json::from_value(value)?;
                        if monitor.id == self.prefs.workspace.monitor.id
                            && monitor.fingerprint == self.prefs.workspace.monitor.fingerprint
                        {
                            self.pair.monitor_input = monitor.current;
                        } else {
                            self.pair.monitor_input = None;
                        }
                    }
                }
                Ok(())
            })();
            if matches!(kind, QueryKind::MonitorStatus) {
                if result.is_err() {
                    self.pair.monitor_input = None;
                }
            } else {
                self.message(result, "Список устройств обновлён.");
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
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.action(Action::Quit, ctx);
        }
        if self
            .quitting_since
            .is_some_and(|time| time.elapsed() > Duration::from_secs(6))
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
                worker.force_stop();
            }
        }
        if self.quitting
            && !self.input_running()
            && !self.monitor_running()
            && !self.audio_running()
            && !self.microphone_running()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(200));
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
        ctx.request_repaint();
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
        let failed_update = if managed {
            None
        } else {
            std::fs::read_to_string(self.dir.join("updates/last-error.txt")).ok()
        };
        if let Some(error) = &failed_update {
            self.notice = Some((format!("Обновление не установлено: {error}"), true));
        }
        if self.prefs.auto_update && failed_update.is_none() {
            if !std::env::current_exe().is_ok_and(|path| crate::updater::shared_managed(&path))
                && crate::updater::staged(&self.dir).is_ok()
            {
                self.update_ready = true;
                self.install_update(ctx);
                if self.quitting {
                    return;
                }
            }
            self.check_updates(ctx);
        }
    }

    fn save_application(&mut self) {
        let result = (|| {
            self.prefs.save(&self.dir)?;
            crate::autostart::sync(self.prefs.autostart, &std::env::current_exe()?, &self.dir)
        })();
        self.message(result, "Настройки запуска сохранены.");
    }

    fn check_updates(&mut self, ctx: &egui::Context) {
        if self.update_job.is_some()
            || self.install_job.is_some()
            || self.install_pending
            || self.quitting
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
                self.update_status = "Проверяем и скачиваем подписанное обновление…".into();
            }
            Err(error) => self.update_status = error.to_string(),
        }
    }

    fn install_update(&mut self, ctx: &egui::Context) {
        if self.prepared_update.is_some() {
            if let Err(error) = self.save() {
                self.notice = Some((error.to_string(), true));
                return;
            }
            self.stop_pair();
            self.query.take();
            self.install_pending = true;
            self.update_ready = false;
            self.update_status =
                "Устанавливаем обновление. На Ubuntu подтвердите системный запрос пароля.".into();
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

impl eframe::App for Desktop {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.tick(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.render(ui);
        self.presented = true;
    }
}
