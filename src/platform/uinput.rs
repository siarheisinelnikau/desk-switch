//! Relative input through the Linux kernel's virtual keyboard and mouse.
use super::receiver::Sink;
use crate::wire::{Input, Mode};
use anyhow::{Context, Result, ensure};
use evdev::{
    AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode, RelativeAxisCode,
    RelativeAxisEvent, uinput::VirtualDevice,
};
use futures_util::StreamExt;
use std::{collections::BTreeSet, time::Duration};
use tokio::{sync::watch, task::JoinHandle};
use zbus::{Connection, Proxy, zvariant::OwnedObjectPath};

pub(crate) struct NativeInput {
    keyboard: VirtualDevice,
    mouse: VirtualDevice,
    held: BTreeSet<u16>,
    wheel: [i32; 2],
    active: bool,
    seat: LocalSeat,
}

impl NativeInput {
    pub(crate) async fn open() -> Result<Self> {
        let keyboard_keys: AttributeSet<KeyCode> = (1..=248).map(KeyCode).collect();
        let mouse_buttons: AttributeSet<KeyCode> = (272..=276).map(KeyCode).collect();
        let axes: AttributeSet<_> = [
            RelativeAxisCode::REL_X,
            RelativeAxisCode::REL_Y,
            RelativeAxisCode::REL_HWHEEL,
            RelativeAxisCode::REL_WHEEL,
            RelativeAxisCode::REL_HWHEEL_HI_RES,
            RelativeAxisCode::REL_WHEEL_HI_RES,
        ]
        .into_iter()
        .collect();
        let mut keyboard = VirtualDevice::builder()
            .context("opening /dev/uinput; enable native keyboard and mouse in Settings")?
            .name("Desk Switch virtual keyboard")
            .input_id(InputId::new(BusType::BUS_VIRTUAL, 0, 0, 1))
            .with_keys(&keyboard_keys)?
            .build()
            .context("creating the Desk Switch virtual keyboard")?;
        let mut mouse = VirtualDevice::builder()?
            .name("Desk Switch virtual mouse")
            .input_id(InputId::new(BusType::BUS_VIRTUAL, 0, 0, 1))
            .with_keys(&mouse_buttons)?
            .with_relative_axes(&axes)?
            .build()
            .context("creating the Desk Switch virtual mouse")?;
        // Kernel registration precedes desktop discovery. Wait for both device
        // nodes to be initialized so a newly authenticated peer's first input
        // isn't sent before the standard input seat learns about the devices.
        tokio::try_join!(wait_for_device(&mut keyboard), wait_for_device(&mut mouse))?;
        let seat = tokio::time::timeout(Duration::from_secs(2), LocalSeat::open())
            .await
            .context("waiting for the graphical login session")??;
        Ok(Self {
            keyboard,
            mouse,
            held: BTreeSet::new(),
            wheel: [0; 2],
            active: false,
            seat,
        })
    }

    pub(crate) async fn wait_for_active_session(&mut self) -> Result<()> {
        self.seat.available.wait_for(|available| *available).await?;
        Ok(())
    }

    #[cfg(feature = "android-remote")]
    pub(crate) fn phone_ready(&self) -> bool {
        self.active && self.seat.check().is_ok()
    }

    #[cfg(feature = "android-remote")]
    pub(crate) async fn poll_phone(&mut self) -> Result<()> {
        self.seat.available.changed().await?;
        if self.seat.check().is_ok() {
            self.active = true;
        } else {
            self.release()?;
        }
        Ok(())
    }

    fn key(&mut self, code: u16, down: bool) -> Result<()> {
        if down == self.held.contains(&code) {
            return Ok(());
        }
        // Record the intended press before writing. If only part of a frame is
        // emitted, cleanup must still attempt the matching release.
        if down {
            self.held.insert(code);
        }
        let device = if code >= 272 {
            &mut self.mouse
        } else {
            &mut self.keyboard
        };
        device.emit(&[InputEvent::new(EventType::KEY.0, code, i32::from(down))])?;
        if !down {
            self.held.remove(&code);
        }
        Ok(())
    }

    fn scroll(&mut self, dx: i16, dy: i16) -> Result<()> {
        let x = i32::from(dx);
        let y = -i32::from(dy);
        let mut remainder = self.wheel;
        let coarse_x = detents(&mut remainder[0], x);
        let coarse_y = detents(&mut remainder[1], y);
        self.mouse.emit(&[
            *RelativeAxisEvent::new(RelativeAxisCode::REL_HWHEEL_HI_RES, x),
            *RelativeAxisEvent::new(RelativeAxisCode::REL_WHEEL_HI_RES, y),
            *RelativeAxisEvent::new(RelativeAxisCode::REL_HWHEEL, coarse_x),
            *RelativeAxisEvent::new(RelativeAxisCode::REL_WHEEL, coarse_y),
        ])?;
        self.wheel = remainder;
        Ok(())
    }
}

async fn wait_for_device(device: &mut VirtualDevice) -> Result<()> {
    let node = device
        .enumerate_dev_nodes()
        .await?
        .next_entry()
        .await?
        .context("the virtual input device has no event node")?;
    let sysfs = device.get_syspath()?.join(
        node.file_name()
            .context("the virtual input node has no name")?,
    );
    let status = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new("/usr/bin/udevadm")
            .args(["wait", "--timeout=2"])
            .arg(sysfs)
            .kill_on_drop(true)
            .status(),
    )
    .await
    .context("waiting for virtual input device registration")??;
    ensure!(
        status.success(),
        "udev could not initialize the virtual input device"
    );
    Ok(())
}

fn detents(remainder: &mut i32, delta: i32) -> i32 {
    let total = *remainder + delta;
    *remainder = total % 120;
    total / 120
}

/// Kernel input goes to the seat's current session. Stop using it when another
/// session becomes active or GNOME locks this desktop.
struct LocalSeat {
    available: watch::Receiver<bool>,
    monitor: JoinHandle<()>,
}

impl LocalSeat {
    async fn open() -> Result<Self> {
        const LOGIN: &str = "org.freedesktop.login1";
        let connection = Connection::system().await?;
        let manager = Proxy::new(
            &connection,
            LOGIN,
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )
        .await?;
        let uid = rustix::process::getuid().as_raw();
        let id = std::env::var("XDG_SESSION_ID")
            .ok()
            .filter(|id| !id.is_empty());
        let path: OwnedObjectPath = if let Some(id) = id {
            manager.call("GetSession", &(id,)).await?
        } else {
            // GNOME can start the app as a user service outside the logind
            // session scope. Use logind's own graphical session for this UID.
            let user_path: OwnedObjectPath = manager.call("GetUser", &(uid,)).await?;
            let user =
                Proxy::new(&connection, LOGIN, user_path, "org.freedesktop.login1.User").await?;
            let (_, path): (String, OwnedObjectPath) = user.get_property("Display").await?;
            path
        };
        drop(manager);
        let session =
            Proxy::new_owned(connection, LOGIN, path, "org.freedesktop.login1.Session").await?;
        let (owner, _): (u32, OwnedObjectPath) = session.get_property("User").await?;
        let (seat, _): (String, OwnedObjectPath) = session.get_property("Seat").await?;
        let kind: String = session.get_property("Type").await?;
        let remote: bool = session.get_property("Remote").await?;
        ensure!(
            owner == uid
                && seat == "seat0"
                && !remote
                && matches!(kind.as_str(), "wayland" | "x11"),
            "native input requires the local graphical session on the default input seat"
        );
        let mut active_changes = session.receive_property_changed::<bool>("Active").await;
        let mut lock_changes = session.receive_property_changed::<bool>("LockedHint").await;
        let mut owner_changes = session.receive_owner_changed().await?;
        let mut active: bool = session.get_property("Active").await?;
        let mut locked: bool = session.get_property("LockedHint").await?;
        let (publish, available) = watch::channel(active && !locked);
        let monitor = tokio::spawn(async move {
            loop {
                let changed = tokio::select! {
                    Some(change) = active_changes.next() => change.get().await.map(|value| active = value),
                    Some(change) = lock_changes.next() => change.get().await.map(|value| locked = value),
                    _ = owner_changes.next() => break,
                };
                if changed.is_err() {
                    break;
                }
                publish.send_replace(active && !locked);
            }
            publish.send_replace(false);
        });
        Ok(Self { available, monitor })
    }

    fn check(&self) -> Result<()> {
        ensure!(
            *self.available.borrow(),
            "The Ubuntu desktop is locked or inactive"
        );
        Ok(())
    }
}

impl Drop for LocalSeat {
    fn drop(&mut self) {
        self.monitor.abort();
    }
}

impl Sink for NativeInput {
    async fn poll(&mut self) -> Result<()> {
        self.seat.available.changed().await?;
        if self.active
            && let Err(error) = self.seat.check()
        {
            self.release()?;
            return Err(error);
        }
        Ok(())
    }

    fn prepare(&self, edge: Option<u16>, _: Mode) -> Result<()> {
        ensure!(!self.active, "Virtual input devices are busy");
        ensure!(edge.is_none(), "Screen edges require Wayland input access");
        self.seat.check()
    }

    fn activate(&mut self, edge: Option<u16>, mode: Mode) -> Result<()> {
        self.prepare(edge, mode)?;
        self.active = true;
        Ok(())
    }

    fn event(&mut self, event: Input) -> Result<Option<u16>> {
        ensure!(
            self.active && event.valid(),
            "Invalid or inactive input event"
        );
        self.seat.check()?;
        match event {
            Input::Key { code, down } | Input::Button { code, down } => self.key(code, down)?,
            Input::Move { dx, dy } => self.mouse.emit(&[
                *RelativeAxisEvent::new(RelativeAxisCode::REL_X, dx),
                *RelativeAxisEvent::new(RelativeAxisCode::REL_Y, dy),
            ])?,
            Input::Scroll { dx, dy } => self.scroll(dx, dy)?,
        }
        Ok(None)
    }

    fn release(&mut self) -> Result<()> {
        self.active = false;
        self.wheel = [0; 2];
        let mut result = Ok(());
        for code in self.held.clone() {
            result = result.and(self.key(code, false));
        }
        result
    }
}

impl Drop for NativeInput {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::detents;

    #[test]
    fn wheel_preserves_subdetent_motion_and_direction_changes() {
        let mut remainder = 0;
        assert_eq!(detents(&mut remainder, 80), 0);
        assert_eq!(detents(&mut remainder, -20), 0);
        assert_eq!(detents(&mut remainder, 70), 1);
        assert_eq!(remainder, 10);
        assert_eq!(detents(&mut remainder, -250), -2);
        assert_eq!(remainder, 0);
    }
}
