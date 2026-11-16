//! One-shot, user-controlled GNOME integration setup. Never changes global extension settings.
use anyhow::{Context, Result, ensure};
use eframe::egui;
use std::{collections::HashMap, sync::mpsc, time::Duration};

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
            self.start(ctx, false);
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
                        "Enable Desk Switch Tray once to open the device popup with a single left-click."
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
                            egui::Button::new(crate::i18n::tr("Enable or reload tray popup")),
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
    let shell = zbus::blocking::Proxy::new(
        connection,
        "org.gnome.Shell.Extensions",
        "/org/gnome/Shell/Extensions",
        "org.gnome.Shell.Extensions",
    )?;
    let info: HashMap<String, zbus::zvariant::OwnedValue> =
        shell.call("GetExtensionInfo", &(UUID,))?;
    ensure!(
        !info.is_empty(),
        "GNOME has not discovered Desk Switch Tray yet. Sign out and sign in once after installing this update, then enable the tray popup here."
    );
    if enable {
        ensure!(
            shell.get_property::<bool>("UserExtensionsEnabled")?,
            "GNOME extensions are disabled for this session. Enable them in Extensions, then retry here."
        );
        // This affects only our own bundled companion and runs only after a deliberate click.
        let _: () = shell.call("ReloadExtension", &(UUID,)).context("Reloading Desk Switch Tray; sign out and sign in if GNOME still has the previous version loaded")?;
        ensure!(
            shell.call::<_, _, bool>("EnableExtension", &(UUID,))?,
            "GNOME could not enable Desk Switch Tray"
        );
    }
    let bridge = zbus::blocking::Proxy::new(
        connection,
        "io.github.siarheisinelnikau.DeskSwitch.Clipboard",
        "/io/github/siarheisinelnikau/DeskSwitch/Clipboard",
        "io.github.siarheisinelnikau.DeskSwitch.Clipboard",
    )?;
    match bridge.call::<_, _, u32>("Version", &()) {
        Ok(version) => Ok(version >= 3),
        Err(_) if !enable => Ok(false),
        Err(error) => Err(error).context("GNOME has not loaded the tray companion. Sign out and sign in, then enable the tray popup here"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering::SeqCst},
    };

    #[test]
    fn setup_is_read_only_until_requested_and_reloads_only_our_extension() {
        const MARKER: &str = "DESK_SWITCH_TRAY_SETUP_PRIVATE_BUS";
        if std::env::var(MARKER).as_deref() != Ok("1") {
            let output = std::process::Command::new("dbus-run-session").arg("--")
                .arg(std::env::current_exe().unwrap()).args(["--exact",
                    "gui::linux_tray::tests::setup_is_read_only_until_requested_and_reloads_only_our_extension",
                    "--nocapture"]).env(MARKER, "1")
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
        struct Shell(Arc<Mutex<Vec<String>>>);
        #[zbus::interface(name = "org.gnome.Shell.Extensions")]
        impl Shell {
            fn get_extension_info(
                &self,
                uuid: &str,
            ) -> HashMap<String, zbus::zvariant::OwnedValue> {
                assert_eq!(uuid, UUID);
                HashMap::from([("state".into(), 2u32.into())])
            }
            #[zbus(property)]
            fn user_extensions_enabled(&self) -> bool {
                true
            }
            fn reload_extension(&self, uuid: &str) {
                assert_eq!(uuid, UUID);
                self.0.lock().unwrap().push("reload".into());
            }
            fn enable_extension(&self, uuid: &str) -> bool {
                assert_eq!(uuid, UUID);
                self.0.lock().unwrap().push("enable".into());
                true
            }
        }
        struct Bridge(Arc<AtomicU32>);
        #[zbus::interface(name = "io.github.siarheisinelnikau.DeskSwitch.Clipboard")]
        impl Bridge {
            fn version(&self) -> u32 {
                self.0.load(SeqCst)
            }
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let version = Arc::new(AtomicU32::new(2));
        let _server = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name("org.gnome.Shell.Extensions")
            .unwrap()
            .name("io.github.siarheisinelnikau.DeskSwitch.Clipboard")
            .unwrap()
            .serve_at("/org/gnome/Shell/Extensions", Shell(calls.clone()))
            .unwrap()
            .serve_at(
                "/io/github/siarheisinelnikau/DeskSwitch/Clipboard",
                Bridge(version.clone()),
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
            calls.lock().unwrap().is_empty(),
            "Startup must not enable or reload extensions"
        );
        version.store(3, SeqCst);
        assert!(check_on(&client, true).unwrap());
        assert_eq!(*calls.lock().unwrap(), ["reload", "enable"]);
        assert!(check_on(&client, false).unwrap());
        assert_eq!(calls.lock().unwrap().len(), 2);
    }
}
