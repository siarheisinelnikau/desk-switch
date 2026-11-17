//! Bounded startup setup for our packaged GNOME tray integration.
use anyhow::{Result, bail, ensure};
use eframe::egui;
use std::{
    collections::HashMap,
    sync::mpsc,
    time::{Duration, Instant},
};

const UUID: &str = "desk-switch@siarheisinelnikau";

#[derive(Default)]
pub(super) struct Integration {
    started: bool,
    pending: Option<mpsc::Receiver<Result<bool, String>>>,
    pub ready: bool,
    message: String,
}

impl Integration {
    pub fn tick(&mut self, ctx: &egui::Context) {
        if !self.started && is_gnome() {
            self.start(ctx, true);
        }
        if let Some(result) = self
            .pending
            .as_ref()
            .and_then(|pending| pending.try_recv().ok())
        {
            self.pending = None;
            match result {
                Ok(ready) => {
                    self.ready = ready;
                    self.message = if ready {
                        "Left-click opens the device popup. Right-click opens the desktop menu."
                    } else {
                        "The tray popup is unavailable. Repair the integration or check the desktop session status."
                    }.into();
                }
                Err(error) => {
                    self.ready = false;
                    self.message = error;
                }
            }
        }
    }

    pub fn start(&mut self, ctx: &egui::Context, enable: bool) {
        if self.pending.is_some() {
            return;
        }
        self.started = true;
        let (send, receive) = mpsc::channel();
        let ctx = ctx.clone();
        self.message = "Checking GNOME tray integration…".into();
        match std::thread::Builder::new()
            .name("Desk Switch tray setup".into())
            .spawn(move || {
                let result = check(enable).map_err(|error| format!("{error:#}"));
                let _ = send.send(result);
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }) {
            Ok(_) => self.pending = Some(receive),
            Err(error) => self.message = format!("Could not check tray integration: {error}"),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        if !is_gnome() {
            return;
        }
        egui::Frame::group(ui.style())
            .inner_margin(16)
            .show(ui, |ui| {
                ui.strong(crate::i18n::tr("Tray device popup"));
                ui.label(crate::i18n::tr(&self.message));
                if !self.ready
                    && ui
                        .add_enabled(
                            self.pending.is_none(),
                            egui::Button::new(crate::i18n::tr("Repair tray popup")),
                        )
                        .clicked()
                {
                    self.start(ui.ctx(), true);
                }
                if ui
                    .add_enabled(
                        self.pending.is_none(),
                        egui::Button::new(crate::i18n::tr("Check tray integration")),
                    )
                    .clicked()
                {
                    self.start(ui.ctx(), false);
                }
            });
    }
}

fn is_gnome() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    desktop.contains("gnome") || desktop.contains("ubuntu")
}

fn check(enable: bool) -> Result<bool> {
    let connection = zbus::blocking::connection::Builder::session()?
        .method_timeout(Duration::from_secs(3))
        .build()?;
    check_on(&connection, enable)
}

fn check_on(connection: &zbus::blocking::Connection, enable: bool) -> Result<bool> {
    // A current, running bridge needs no extension settings changes at all.
    if tray_ready(connection) {
        return Ok(true);
    }
    let shell: zbus::blocking::Proxy<'_> = zbus::blocking::proxy::Builder::new(connection)
        .destination("org.gnome.Shell.Extensions")?
        .path("/org/gnome/Shell/Extensions")?
        .interface("org.gnome.Shell.Extensions")?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()?;
    let info = extension_info(&shell, enable)?;
    if !enable {
        return Ok(false);
    }
    ensure!(
        shell.get_property::<bool>("UserExtensionsEnabled")?,
        "GNOME extensions are disabled for this desktop session. The tray popup cannot run while this desktop policy is disabled."
    );
    extension_error(&info)?;
    if !matches!(extension_state(&info), Some(1 | 8)) {
        // Enable only this application's UUID. Preserve other extensions, the
        // global extension switch and the desktop's version validation.
        // ReloadExtension has been unsupported since GNOME 45. Disabling and
        // enabling cannot replace JavaScript already imported by Shell either.
        ensure!(
            shell.call::<_, _, bool>("EnableExtension", &(UUID,))?,
            "GNOME could not enable Desk Switch Tray"
        );
    }
    // EnableExtension returns before the asynchronous import and bus export
    // finish. Wait only on this worker, then stop until the next launch or an
    // explicit repair. There is no recurring idle/hidden-window polling.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if tray_ready(connection) {
            return Ok(true);
        }
        let info = shell.call("GetExtensionInfo", &(UUID,))?;
        extension_error(&info)?;
        ensure!(
            Instant::now() < deadline,
            "GNOME has not loaded the current tray integration. Sign out and sign back in once to load the installed version; Desk Switch will enable it automatically."
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn tray_ready(connection: &zbus::blocking::Connection) -> bool {
    let Ok(bridge) = zbus::blocking::Proxy::new(
        connection,
        "io.github.siarheisinelnikau.DeskSwitch.Clipboard",
        "/io/github/siarheisinelnikau/DeskSwitch/Clipboard",
        "io.github.siarheisinelnikau.DeskSwitch.Clipboard",
    ) else {
        return false;
    };
    bridge
        .call::<_, _, u32>("TrayVersion", &())
        .is_ok_and(|version| version >= 1)
}

fn extension_info(
    shell: &zbus::blocking::Proxy<'_>,
    wait_for_startup: bool,
) -> Result<HashMap<String, zbus::zvariant::OwnedValue>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // Autostart can reach the Shell bus before its extension loader has
        // initialized. Give that bounded startup work time to finish.
        let info = shell.call::<_, _, HashMap<String, zbus::zvariant::OwnedValue>>(
            "GetExtensionInfo",
            &(UUID,),
        );
        match info {
            Ok(info) if !info.is_empty() => return Ok(info),
            Ok(_) if !wait_for_startup || Instant::now() >= deadline => bail!(
                "GNOME has not discovered the newly installed tray integration. Sign out and sign back in once; Desk Switch will enable it automatically."
            ),
            Err(error) if !wait_for_startup || Instant::now() >= deadline => {
                return Err(error.into());
            }
            _ => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn extension_state(info: &HashMap<String, zbus::zvariant::OwnedValue>) -> Option<u32> {
    let value = info.get("state")?;
    // GNOME serializes JavaScript numbers as D-Bus doubles.
    f64::try_from(value)
        .ok()
        .map(|state| state as u32)
        .or_else(|| u32::try_from(value).ok())
}

fn extension_error(info: &HashMap<String, zbus::zvariant::OwnedValue>) -> Result<()> {
    ensure!(
        !matches!(extension_state(info), Some(3 | 4)),
        "GNOME could not load the tray integration: {}. Sign out and sign back in to load the installed version.",
        info.get("error")
            .and_then(|value| <&str>::try_from(value).ok())
            .filter(|error| !error.is_empty())
            .unwrap_or("the extension is incompatible or reported an error")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering::SeqCst},
    };

    #[derive(Default)]
    struct Desktop {
        calls: Mutex<Vec<String>>,
        discovered: AtomicBool,
        allowed: AtomicBool,
        state: AtomicU32,
        version: AtomicU32,
    }

    struct Shell(Arc<Desktop>);
    #[zbus::interface(name = "org.gnome.Shell.Extensions")]
    impl Shell {
        fn get_extension_info(&self, uuid: &str) -> HashMap<String, zbus::zvariant::OwnedValue> {
            assert_eq!(uuid, UUID);
            if !self.0.discovered.load(SeqCst) {
                return HashMap::new();
            }
            HashMap::from([("state".into(), f64::from(self.0.state.load(SeqCst)).into())])
        }
        #[zbus(property)]
        fn user_extensions_enabled(&self) -> bool {
            self.0.allowed.load(SeqCst)
        }
        fn enable_extension(&self, uuid: &str) -> bool {
            assert_eq!(uuid, UUID, "Never enable another application's extension");
            self.0.calls.lock().unwrap().push(uuid.into());
            self.0.state.store(8, SeqCst);
            let desktop = self.0.clone();
            // Like GNOME, return before the module and its bridge finish loading.
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                desktop.version.store(1, SeqCst);
                desktop.state.store(1, SeqCst);
            });
            true
        }
        fn reload_extension(&self, uuid: &str) -> zbus::fdo::Result<()> {
            panic!("ReloadExtension is unsupported on the supported GNOME releases: {uuid}");
        }
    }
    struct Bridge(Arc<Desktop>);
    #[zbus::interface(name = "io.github.siarheisinelnikau.DeskSwitch.Clipboard")]
    impl Bridge {
        fn tray_version(&self) -> u32 {
            self.0.version.load(SeqCst)
        }
    }

    #[test]
    fn startup_enables_only_our_companion_and_waits_for_its_bridge() {
        const MARKER: &str = "DESK_SWITCH_TRAY_SETUP_PRIVATE_BUS";
        if std::env::var(MARKER).as_deref() != Ok("1") {
            let output = std::process::Command::new("dbus-run-session").arg("--")
                .arg(std::env::current_exe().unwrap()).args(["--exact",
                    "gui::linux_tray::tests::startup_enables_only_our_companion_and_waits_for_its_bridge",
                    "--nocapture"]).env(MARKER, "1")
                .env("XDG_CURRENT_DESKTOP", "GNOME")
                .env("DESK_SWITCH_TRAY_SETUP_PARENT_BUS", std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default())
                .output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        assert_ne!(
            std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap(),
            std::env::var("DESK_SWITCH_TRAY_SETUP_PARENT_BUS").unwrap()
        );
        let desktop = Arc::new(Desktop::default());
        desktop.discovered.store(true, SeqCst);
        desktop.allowed.store(true, SeqCst);
        desktop.state.store(6, SeqCst);
        let _server = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name("org.gnome.Shell.Extensions")
            .unwrap()
            .name("io.github.siarheisinelnikau.DeskSwitch.Clipboard")
            .unwrap()
            .serve_at("/org/gnome/Shell/Extensions", Shell(desktop.clone()))
            .unwrap()
            .serve_at(
                "/io/github/siarheisinelnikau/DeskSwitch/Clipboard",
                Bridge(desktop.clone()),
            )
            .unwrap()
            .build()
            .unwrap();
        let client = zbus::blocking::connection::Builder::session()
            .unwrap()
            .method_timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        assert!(!check_on(&client, false).unwrap());
        assert!(
            desktop.calls.lock().unwrap().is_empty(),
            "Status checks are read-only"
        );

        // Drive the same one-shot startup path as the real app, without a GUI.
        // Simulate login reaching the bus before extension discovery completes.
        desktop.discovered.store(false, SeqCst);
        let loading = desktop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            loading.discovered.store(true, SeqCst);
        });
        let ctx = egui::Context::default();
        let mut integration = Integration::default();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !integration.ready && Instant::now() < deadline {
            integration.tick(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(integration.ready, "{}", integration.message);
        assert_eq!(*desktop.calls.lock().unwrap(), [UUID]);
        for _ in 0..10 {
            integration.tick(&ctx);
        }
        assert!(check_on(&client, true).unwrap());
        assert_eq!(
            desktop.calls.lock().unwrap().len(),
            1,
            "Ready startup is idempotent"
        );

        // An undiscovered install and desktop policy must not report readiness
        // or change global policy. These are distinct from a disabled companion.
        desktop.version.store(0, SeqCst);
        desktop.discovered.store(false, SeqCst);
        assert!(
            check_on(&client, false)
                .unwrap_err()
                .to_string()
                .contains("not discovered")
        );
        desktop.discovered.store(true, SeqCst);
        desktop.state.store(2, SeqCst);
        desktop.allowed.store(false, SeqCst);
        assert!(
            check_on(&client, true)
                .unwrap_err()
                .to_string()
                .contains("disabled for this desktop")
        );
        desktop.allowed.store(true, SeqCst);
        desktop.state.store(3, SeqCst);
        assert!(
            check_on(&client, true)
                .unwrap_err()
                .to_string()
                .contains("could not load")
        );
        assert_eq!(desktop.calls.lock().unwrap().len(), 1);
    }
}
