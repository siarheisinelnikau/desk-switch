mod model;
mod tray;
mod view;
mod worker;

use crate::{audio::AudioDevice, config, lifecycle::InputStatus, platform::MonitorInfo};
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
    Emergency,
    Sound,
    Quit,
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Overview,
    Devices,
    Monitor,
    Sound,
    Application,
    Diagnostics,
}

#[derive(Clone, Copy)]
enum QueryKind {
    Monitors,
    Audio,
    #[cfg(target_os = "linux")]
    Wayland,
}

pub struct Desktop {
    dir: PathBuf,
    prefs: Preferences,
    form: Form,
    page: Page,
    public_key: String,
    peer_key: String,
    pair_entry: String,
    input: Option<Worker>,
    audio: Option<Worker>,
    query: Option<(QueryKind, Worker)>,
    monitors: Vec<MonitorInfo>,
    devices: Vec<AudioDevice>,
    #[cfg(target_os = "linux")]
    wayland_screens: Vec<String>,
    notice: Option<(String, bool)>,
    tray: Option<tray::Tray>,
    tray_error: Option<String>,
    tray_snapshot: (bool, bool, bool),
    actions: Receiver<Action>,
    hidden: bool,
    start_hidden: bool,
    quitting: bool,
    last_switch: Option<Instant>,
    dirty: bool,
    startup_pending: bool,
    startup_valid: bool,
    update_job: Option<crate::updater::Job>,
    update_status: String,
    update_ready: bool,
    last_update_check: Option<Instant>,
    last_version_error: Option<String>,
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
        view::style(&ctx);
        let (prefs, prefs_error) = match Preferences::load(&dir) {
            Ok(prefs) => (prefs, None),
            Err(error) => (Preferences::default(), Some(format!("{error:#}"))),
        };
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
            pair_entry: String::new(),
            input: None,
            audio: None,
            query: None,
            monitors: Vec::new(),
            devices: Vec::new(),
            #[cfg(target_os = "linux")]
            wayland_screens: Vec::new(),
            notice: prefs_error.or(settings_error).map(|error| (error, true)),
            tray,
            tray_error,
            tray_snapshot: (false, false, false),
            actions,
            hidden: false,
            start_hidden,
            quitting: false,
            last_switch: None,
            dirty: false,
            startup_pending: true,
            startup_valid,
            update_job: None,
            update_status: "Проверка ещё не выполнялась.".into(),
            update_ready: false,
            last_update_check: None,
            last_version_error: None,
        }
    }

    fn input_running(&self) -> bool {
        self.input.as_ref().is_some_and(Worker::running)
    }
    fn audio_running(&self) -> bool {
        self.audio.as_ref().is_some_and(Worker::running)
    }
    fn connected(&self) -> bool {
        self.input.as_ref().is_some_and(Worker::connected)
    }

    fn save(&mut self) -> Result<()> {
        config::save_settings(&self.dir, &self.form.document()?)?;
        self.prefs.save(&self.dir)?;
        self.dirty = false;
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
        self.input = Some(Worker::start(Kind::Input, &args, &self.dir)?);
        Ok(())
    }

    fn toggle_input(&mut self) {
        if self.input_running() {
            self.input.as_mut().unwrap().stop();
        } else {
            let result = self.start_input();
            self.message(
                result,
                "Служба запускается. На Ubuntu подтвердите доступ в системном диалоге.",
            );
        }
    }

    fn toggle_audio(&mut self) {
        if self.audio_running() {
            self.prefs.auto_audio = false;
            if let Err(error) = self.prefs.save(&self.dir) {
                self.notice = Some((error.to_string(), true));
            }
            self.audio.as_mut().unwrap().stop();
            return;
        }
        let result = (|| {
            self.prefs.save(&self.dir)?;
            config::load_identity(&self.dir)?;
            config::load_peer(&self.dir).context("сначала выполните сопряжение")?;
            let args = self.prefs.audio_args()?;
            let kind = if self.prefs.audio_mode == "send" {
                Kind::AudioSend
            } else {
                Kind::AudioReceive
            };
            self.audio = Some(Worker::start(kind, &args, &self.dir)?);
            self.prefs.auto_audio = true;
            self.prefs.save(&self.dir)?;
            Ok(())
        })();
        self.message(result, "Аудиослужба запускается.");
    }

    fn switch(&mut self) {
        if !self.connected()
            || self
                .last_switch
                .is_some_and(|time| time.elapsed() < Duration::from_millis(800))
        {
            return;
        }
        if !self.form.monitor_enabled {
            self.notice = Some((
                "Выберите общий монитор и его входы в настройках.".into(),
                true,
            ));
            self.page = Page::Monitor;
            return;
        }
        let result = self.input.as_mut().unwrap().command("switch");
        self.last_switch = Some(Instant::now());
        self.message(
            result,
            "Запрос переключения отправлен. Отпустите удерживаемые клавиши и кнопки мыши.",
        );
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
            Action::Emergency => {
                if let Some(input) = &mut self.input {
                    let result = input.command("emergency");
                    self.message(result, "Запрошен аварийный возврат управления.");
                }
            }
            Action::Sound => {
                let starting = !self.audio_running();
                self.toggle_audio();
                if starting && self.notice.as_ref().is_some_and(|(_, error)| *error) {
                    self.action(Action::Show, ctx);
                }
            }
            Action::Quit => {
                self.quitting = true;
                if let Some(input) = &mut self.input {
                    input.stop();
                }
                if let Some(audio) = &mut self.audio {
                    audio.stop();
                }
                self.query.take();
            }
        }
    }

    fn scan(&mut self, kind: QueryKind) {
        if self.query.is_some() {
            return;
        }
        let args: Vec<String> = match kind {
            QueryKind::Monitors => vec!["probe", "--json"],
            QueryKind::Audio => vec!["audio", "outputs", "--json"],
            #[cfg(target_os = "linux")]
            QueryKind::Wayland => vec!["portal-check", "--consent", "--json"],
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        match Worker::start(Kind::Query, &args, &self.dir) {
            Ok(worker) => self.query = Some((kind, worker)),
            Err(error) => self.notice = Some((format!("{error:#}"), true)),
        }
    }

    fn sound_mix(&mut self) {
        if let Some(audio) = &mut self.audio
            && audio.running()
            && audio.kind == Kind::AudioReceive
        {
            let result = audio
                .command(&format!("volume {}", self.prefs.volume))
                .and_then(|()| audio.command(if self.prefs.muted { "mute" } else { "unmute" }));
            if let Err(error) = result {
                self.notice = Some((error.to_string(), true));
            }
        }
        if let Err(error) = self.prefs.save(&self.dir) {
            self.notice = Some((error.to_string(), true));
        }
    }

    fn tick(&mut self, ctx: &egui::Context) {
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
        for worker in [&mut self.input, &mut self.audio].into_iter().flatten() {
            let was_running = worker.running();
            worker.poll();
            if was_running && !worker.running() && worker.exit_ok == Some(false) {
                self.notice = Some((
                    worker
                        .logs
                        .back()
                        .cloned()
                        .unwrap_or_else(|| "Служба завершилась с ошибкой.".into()),
                    true,
                ));
            }
        }
        let version_error = [&self.input, &self.audio]
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
                        && !self.audio_running()
                        && !self.quitting
                        && !self.dir.join("updates/last-error.txt").exists()
                    {
                        self.install_update(ctx);
                    }
                }
                Err(error) => {
                    self.update_status = format!("Не удалось проверить обновления: {error:#}")
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
                    QueryKind::Monitors => self.monitors = serde_json::from_value(value)?,
                    QueryKind::Audio => self.devices = serde_json::from_value(value)?,
                    #[cfg(target_os = "linux")]
                    QueryKind::Wayland => self.wayland_screens = serde_json::from_value(value)?,
                }
                Ok(())
            })();
            self.message(result, "Список устройств обновлён.");
        }
        let tray_available = self.tray.as_ref().is_some_and(tray::Tray::available);
        if self.hidden && !tray_available {
            self.action(Action::Show, ctx);
        }
        let snapshot = (
            self.connected() && self.form.monitor_enabled,
            self.input_running(),
            self.audio_running(),
        );
        if snapshot != self.tray_snapshot {
            if let Some(tray) = &mut self.tray {
                tray.update(snapshot.0, snapshot.1, snapshot.2);
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
            if tray_available {
                self.hidden = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                self.action(Action::Quit, ctx);
            }
        }
        if self.quitting && !self.input_running() && !self.audio_running() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    fn input_label(&self) -> (&'static str, bool) {
        let Some(worker) = &self.input else {
            return ("Не подключено", false);
        };
        if worker.version_error.is_some() {
            return ("Версии не совпадают", false);
        }
        if worker.stopping && worker.running() {
            return ("Отключение…", false);
        }
        if !worker.running() {
            return ("Остановлено", false);
        }
        match worker.input_status {
            Some(InputStatus::Here) => ("Управление на этом ПК", true),
            Some(InputStatus::Peer) => ("Управление на другом ПК", true),
            Some(InputStatus::Recovering) => ("Возврат управления…", false),
            _ => ("Ожидание второго ПК", false),
        }
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
        let failed_update = std::fs::read_to_string(self.dir.join("updates/last-error.txt")).ok();
        if let Some(error) = &failed_update {
            self.notice = Some((format!("Обновление не установлено: {error}"), true));
        }
        if self.prefs.auto_update && failed_update.is_none() {
            if crate::updater::staged(&self.dir).is_ok() {
                self.update_ready = true;
                self.install_update(ctx);
                if self.quitting {
                    return;
                }
            }
            self.check_updates(ctx);
        }
        let paired = !self.public_key.is_empty() && !self.peer_key.is_empty();
        if paired && self.prefs.auto_connect {
            self.toggle_input();
        }
        if paired && self.prefs.auto_audio {
            self.toggle_audio();
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
        if self.update_job.is_some() || self.quitting {
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
    }
}
