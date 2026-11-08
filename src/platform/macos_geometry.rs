//! CoreGraphics point coordinates and a topology epoch shared with the helper.
use crate::edge::{NativeScreen, ScreenRect};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub(in crate::platform) struct Screen {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub epoch: u64,
}
impl Screen {
    pub fn native(&self) -> Result<NativeScreen> {
        for value in [self.x, self.y, self.width, self.height] {
            ensure!(
                value.is_finite() && value.fract() == 0.,
                "Invalid macOS screen geometry"
            );
        }
        ensure!(
            self.x >= f64::from(i32::MIN)
                && self.x <= f64::from(i32::MAX)
                && self.y >= f64::from(i32::MIN)
                && self.y <= f64::from(i32::MAX)
                && self.width >= 2.
                && self.width <= f64::from(u32::MAX)
                && self.height >= 2.
                && self.height <= f64::from(u32::MAX),
            "macOS screen geometry is out of range"
        );
        let native = NativeScreen {
            native_id: self.id.clone(),
            rect: ScreenRect {
                x: self.x as i32,
                y: self.y as i32,
                width: self.width as u32,
                height: self.height as u32,
            },
            epoch: self.epoch,
        };
        native.validate()?;
        Ok(native)
    }
    pub fn rect(&self) -> crate::edge::Rect {
        crate::edge::Rect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}
pub(in crate::platform) fn validate(screens: &[Screen]) -> Result<Vec<NativeScreen>> {
    ensure!(screens.len() <= 128, "Too many macOS screens");
    let native: Vec<_> = screens.iter().map(Screen::native).collect::<Result<_>>()?;
    let mut ids = std::collections::BTreeSet::new();
    ensure!(
        native.iter().all(|screen| ids.insert(&screen.native_id)),
        "Duplicate macOS screen identity"
    );
    Ok(native)
}
pub(in crate::platform) fn screens() -> Result<Vec<Screen>> {
    let screens: Vec<Screen> = crate::macos_native::query("screen-geometry", &[])?;
    validate(&screens)?;
    Ok(screens)
}
pub(in crate::platform) fn snapshot() -> Result<Vec<NativeScreen>> {
    validate(&screens()?)
}

#[repr(C)]
struct Point {
    x: f64,
    y: f64,
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    #[link_name = "CGWarpMouseCursorPosition"]
    fn warp_mouse_cursor_position(point: Point) -> i32;
    #[link_name = "CGEventCreate"]
    fn event_create(source: *const std::ffi::c_void) -> *const std::ffi::c_void;
    #[link_name = "CGEventGetLocation"]
    fn event_location(event: *const std::ffi::c_void) -> Point;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    #[link_name = "CFRelease"]
    fn release(value: *const std::ffi::c_void);
}

pub(in crate::platform) fn position(x: f64, y: f64) -> Result<()> {
    ensure!(
        x.is_finite() && y.is_finite(),
        "Invalid macOS cursor position"
    );
    ensure!(
        // SAFETY: the argument is a plain `repr(C)` copy of two doubles with no
        // pointers, and the call only reads it.
        unsafe { warp_mouse_cursor_position(Point { x, y }) } == 0,
        "macOS refused the cursor position"
    );
    Ok(())
}
pub(in crate::platform) fn cursor() -> Result<(f64, f64)> {
    // SAFETY: a null source is the documented way to create an event that
    // describes the current state. The returned event is owned by this
    // function and released below.
    let event = unsafe { event_create(std::ptr::null()) };
    ensure!(!event.is_null(), "Cannot read the macOS cursor position");
    // SAFETY: `event` was checked to be non-null and is still owned here, so
    // the call reads a live event.
    let point = unsafe { event_location(event) };
    unsafe {
        // SAFETY: `event` came from event_create, is owned by this function
        // and is released exactly once.
        release(event);
    }
    ensure!(
        point.x.is_finite() && point.y.is_finite(),
        "Invalid macOS cursor position"
    );
    Ok((point.x, point.y))
}

pub(in crate::platform) fn enter_current(
    entry: crate::edge::Entry,
    activation: u64,
    routes: &tokio::sync::watch::Receiver<crate::edge::Routes>,
    canceled: &std::sync::Mutex<bool>,
) -> Result<()> {
    // The helper query runs off the network loop. Only the final native warp
    // holds the route/cancellation guards, so a superseded entry cannot land.
    entry.resolve(&snapshot()?)?;
    let (x, y) = entry.point();
    commit_entry(activation, routes, canceled, || position(x, y))
        .context("Entering the macOS display")
}

fn commit_entry(
    activation: u64,
    routes: &tokio::sync::watch::Receiver<crate::edge::Routes>,
    canceled: &std::sync::Mutex<bool>,
    position: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let canceled = canceled
        .lock()
        .map_err(|_| anyhow::anyhow!("macOS cursor entry was canceled"))?;
    let current = routes.borrow();
    ensure!(
        !*canceled && current.activation == activation,
        "The display activation changed before the cursor could enter"
    );
    position()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::{Entry, Side};

    fn screen() -> Screen {
        Screen {
            id: "1".into(),
            x: -1920.,
            y: 0.,
            width: 1920.,
            height: 1080.,
            epoch: u64::MAX,
        }
    }
    #[test]
    fn native_geometry_rejects_invalid_coordinates() {
        let mut value = screen();
        value.width = 1.;
        assert!(validate(std::slice::from_ref(&value)).is_err());
        value = screen();
        value.x = 0.5;
        assert!(validate(std::slice::from_ref(&value)).is_err());
        value = screen();
        value.height = f64::NAN;
        assert!(validate(std::slice::from_ref(&value)).is_err());
        value = screen();
        value.epoch = 0;
        assert!(validate(&[value]).is_err());
        assert!(validate(&[screen(), screen()]).is_err());
    }
    #[test]
    fn native_entry_requires_exact_unique_topology() {
        let mut native = validate(&[screen()]).unwrap();
        let entry = Entry {
            screen: native[0].rect,
            geometry: u64::MAX,
            side: Side::Right,
            position: 12345,
        };
        assert!(entry.resolve(&native).is_ok());
        native[0].epoch = 1;
        assert!(entry.resolve(&native).is_err());
        native[0].epoch = u64::MAX;
        native.push(NativeScreen {
            native_id: "2".into(),
            ..native[0].clone()
        });
        assert!(entry.resolve(&native).is_err());
    }
    #[test]
    fn superseded_or_canceled_source_entry_never_moves_the_cursor() {
        let (_send, routes) = tokio::sync::watch::channel(crate::edge::Routes {
            activation: 7,
            ..Default::default()
        });
        let canceled = std::sync::Mutex::new(false);
        let mut moved = false;
        assert!(
            commit_entry(6, &routes, &canceled, || {
                moved = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!moved);
        *canceled.lock().unwrap() = true;
        assert!(
            commit_entry(7, &routes, &canceled, || {
                moved = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!moved);
        *canceled.lock().unwrap() = false;
        commit_entry(7, &routes, &canceled, || {
            moved = true;
            Ok(())
        })
        .unwrap();
        assert!(moved);
    }
}
