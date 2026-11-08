//! GNOME's companion raises only our own fixed-title windows. Requests run
//! outside the GUI thread; the worker sleeps indefinitely between user actions.
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone, Copy)]
pub(super) enum Role {
    Main,
    Quick,
    Transfer,
}
impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Quick => "quick",
            Self::Transfer => "transfer",
        }
    }
}
type Shared = Arc<(Mutex<Option<Role>>, Condvar)>;

pub(super) fn request(role: Role) {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !desktop.contains("gnome") && !desktop.contains("ubuntu") {
        return;
    }
    static WORKER: OnceLock<Option<Shared>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let shared: Shared = Arc::new((Mutex::new(None), Condvar::new()));
        let queue = shared.clone();
        std::thread::Builder::new()
            .name("Desk Switch GNOME activation".into())
            .spawn(move || {
                let mut connection = None;
                loop {
                    let role = {
                        let mut pending = queue.0.lock().unwrap();
                        while pending.is_none() {
                            pending = queue.1.wait(pending).unwrap();
                        }
                        pending.take().unwrap()
                    };
                    if activate(&mut connection, role).is_err() {
                        // An absent/disabled companion leaves the normal native
                        // show/focus behavior in place. Retry on the next action.
                        connection = None;
                    }
                }
            })
            .ok()
            .map(|_| shared)
    });
    if let Some(worker) = worker {
        *worker.0.lock().unwrap() = Some(role);
        worker.1.notify_one();
    }
}

fn activate(connection: &mut Option<zbus::blocking::Connection>, role: Role) -> zbus::Result<()> {
    if connection.is_none() {
        *connection = Some(
            zbus::blocking::connection::Builder::session()?
                .method_timeout(Duration::from_secs(2))
                .build()?,
        );
    }
    let proxy = zbus::blocking::Proxy::new(
        connection.as_ref().unwrap(),
        "io.github.siarheisinelnikau.DeskSwitch.Clipboard",
        "/io/github/siarheisinelnikau/DeskSwitch/Clipboard",
        "io.github.siarheisinelnikau.DeskSwitch.Clipboard",
    )?;
    if matches!(role, Role::Transfer) {
        let version: u32 = proxy.call("Version", &())?;
        if version < 2 {
            return Err(zbus::Error::Failure(
                "Reload Desk Switch Tray in Extensions to show transfer windows.".into(),
            ));
        }
    }
    proxy.call("ActivateWindow", &(role.name(),))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_roles_travel_over_a_private_bus_without_a_real_shell() {
        const MARKER: &str = "DESK_SWITCH_ACTIVATION_PRIVATE_BUS";
        if std::env::var(MARKER).as_deref() != Ok("1") {
            let output = std::process::Command::new("dbus-run-session")
                .arg("--")
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "gui::linux_activation::tests::window_roles_travel_over_a_private_bus_without_a_real_shell",
                    "--nocapture",
                ])
                .env(MARKER, "1")
                .env(
                    "DESK_SWITCH_ACTIVATION_PARENT_BUS",
                    std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default(),
                )
                .output()
                .expect("dbus-run-session is required for the private activation protocol test");
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
        assert!(!address.is_empty());
        assert_ne!(
            Some(address),
            std::env::var("DESK_SWITCH_ACTIVATION_PARENT_BUS").ok()
        );
        struct Mock(Arc<Mutex<Vec<String>>>, Arc<std::sync::atomic::AtomicU32>);
        #[zbus::interface(name = "io.github.siarheisinelnikau.DeskSwitch.Clipboard")]
        impl Mock {
            fn version(&self) -> u32 {
                self.1.load(std::sync::atomic::Ordering::Relaxed)
            }
            fn activate_window(&self, role: String) {
                self.0.lock().unwrap().push(role);
            }
        }
        let roles = Arc::new(Mutex::new(Vec::new()));
        let version = Arc::new(std::sync::atomic::AtomicU32::new(1));
        let _server = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name("io.github.siarheisinelnikau.DeskSwitch.Clipboard")
            .unwrap()
            .serve_at(
                "/io/github/siarheisinelnikau/DeskSwitch/Clipboard",
                Mock(roles.clone(), version.clone()),
            )
            .unwrap()
            .build()
            .unwrap();
        let mut connection = None;
        activate(&mut connection, Role::Main).unwrap();
        activate(&mut connection, Role::Quick).unwrap();
        let error = activate(&mut connection, Role::Transfer).unwrap_err();
        assert!(error.to_string().contains("Reload Desk Switch Tray"));
        assert_eq!(*roles.lock().unwrap(), ["main", "quick"]);
        version.store(2, std::sync::atomic::Ordering::Relaxed);
        activate(&mut connection, Role::Transfer).unwrap();
        assert_eq!(*roles.lock().unwrap(), ["main", "quick", "transfer"]);
    }
}
