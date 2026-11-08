//! Read-only GNOME/Mutter geometry. Missing or ambiguous information disables
//! optional screen edges; it must never prevent ordinary relative input.
use crate::edge::{NativeScreen, ScreenRect};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};
use zbus::zvariant::OwnedValue;

type Properties = HashMap<String, OwnedValue>;
type Spec = (String, String, String, String);
type Mode = (String, i32, i32, f64, f64, Vec<f64>, Properties);
type Physical = (Spec, Vec<Mode>, Properties);
type Logical = (i32, i32, f64, u32, bool, Vec<Spec>, Properties);
type CurrentState = (u32, Vec<Physical>, Vec<Logical>, Properties);
const DISPLAY_NAME: &str = "org.gnome.Mutter.DisplayConfig";
const DISPLAY_PATH: &str = "/org/gnome/Mutter/DisplayConfig";
const COMPANION_NAME: &str = "io.github.siarheisinelnikau.DeskSwitch.Clipboard";
const COMPANION_PATH: &str = "/io/github/siarheisinelnikau/DeskSwitch/Clipboard";
const TIMEOUT: Duration = Duration::from_millis(800);

/// Query once per inventory refresh or native region change, never per motion.
/// The connector names are native Mutter identifiers; EIS regions must also
/// match the complete rectangle uniquely before an edge can be enabled.
pub(crate) fn snapshot() -> Option<Vec<NativeScreen>> {
    let connection = zbus::blocking::connection::Builder::session()
        .ok()?
        .method_timeout(TIMEOUT)
        .build()
        .ok()?;
    let proxy =
        zbus::blocking::Proxy::new(&connection, DISPLAY_NAME, DISPLAY_PATH, DISPLAY_NAME).ok()?;
    let state: CurrentState = proxy.call("GetCurrentState", &()).ok()?;
    from_state(state)
}

/// Return mappings under the original DRM inventory identities. A connector
/// appearing on more than one DRM card cannot be identified by a Mutter name.
pub(crate) fn screens(connectors: &[String]) -> BTreeMap<String, NativeScreen> {
    if connectors.is_empty() {
        return BTreeMap::new();
    }
    let Some(native) = snapshot() else {
        return BTreeMap::new();
    };
    map_connectors(connectors, native)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PointerError {
    EnableCompanion,
    ReloadCompanion,
    NotAuthorized,
    Unavailable,
}
impl std::fmt::Display for PointerError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str(match self {
            Self::EnableCompanion => "Screen edges are unavailable. Enable Desk Switch Tray in GNOME Extensions. Keyboard and mouse input still work.",
            Self::ReloadCompanion => "Screen edges need the updated Desk Switch Tray companion. Toggle it off and on in GNOME Extensions, or sign out and back in. Keyboard and mouse input still work.",
            Self::NotAuthorized => "Screen edges could not verify the Desk Switch input worker. Keyboard and mouse input still work.",
            Self::Unavailable => "Screen edges could not read the pointer position. Keyboard and mouse input still work.",
        })
    }
}
impl std::error::Error for PointerError {}
fn pointer_error(error: zbus::Error) -> PointerError {
    match error {
        zbus::Error::MethodError(name, _, _) => match name.as_str() {
            "org.freedesktop.DBus.Error.UnknownMethod" => PointerError::ReloadCompanion,
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner" => PointerError::EnableCompanion,
            "org.freedesktop.DBus.Error.AccessDenied"
            | "io.github.siarheisinelnikau.DeskSwitch.Clipboard.PointerAccessDenied" => {
                PointerError::NotAuthorized
            }
            _ => PointerError::Unavailable,
        },
        zbus::Error::FDO(error) => match *error {
            zbus::fdo::Error::UnknownMethod(_) => PointerError::ReloadCompanion,
            zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_) => {
                PointerError::EnableCompanion
            }
            zbus::fdo::Error::AccessDenied(_) => PointerError::NotAuthorized,
            _ => PointerError::Unavailable,
        },
        _ => PointerError::Unavailable,
    }
}

/// The companion only observes the current pointer; it never warps it. Call
/// through spawn_blocking at start/resume, not from a GUI or motion callback.
pub(crate) fn pointer() -> Result<(i32, i32), PointerError> {
    let connection = zbus::blocking::connection::Builder::session()
        .map_err(pointer_error)?
        .method_timeout(TIMEOUT)
        .build()
        .map_err(pointer_error)?;
    let proxy =
        zbus::blocking::Proxy::new(&connection, COMPANION_NAME, COMPANION_PATH, COMPANION_NAME)
            .map_err(pointer_error)?;
    proxy.call("PointerState", &()).map_err(pointer_error)
}

fn connector(value: &str) -> Option<String> {
    let mut value = value.strip_prefix("/sys/class/drm/").unwrap_or(value);
    if let Some((name, digest)) = value.split_once(':') {
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        value = name;
    }
    if let Some(rest) = value.strip_prefix("card") {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        value = rest
            .get(digits..)?
            .strip_prefix('-')
            .or_else(|| rest.get(digits..)?.strip_prefix('/'))?;
    }
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    Some(
        value
            .strip_prefix("HDMI-A-")
            .map(|suffix| format!("HDMI-{suffix}"))
            .unwrap_or_else(|| value.into()),
    )
}

fn map_connectors(
    connectors: &[String],
    native: Vec<NativeScreen>,
) -> BTreeMap<String, NativeScreen> {
    let normalized: Vec<_> = connectors.iter().map(|id| (id, connector(id))).collect();
    normalized
        .iter()
        .filter_map(|(id, name)| {
            let name = name.as_ref()?;
            if normalized
                .iter()
                .filter(|(_, candidate)| candidate.as_ref() == Some(name))
                .count()
                != 1
            {
                return None;
            }
            let mut matches = native
                .iter()
                .filter(|screen| connector(&screen.native_id).as_ref() == Some(name));
            let screen = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            Some(((*id).clone(), screen.clone()))
        })
        .collect()
}

#[derive(Clone, Serialize)]
struct Output {
    spec: Spec,
    current: Option<(String, i32, i32, u64)>,
}
#[derive(Serialize)]
struct Layout {
    x: i32,
    y: i32,
    scale_bits: u64,
    transform: u32,
    primary: bool,
    monitors: Vec<Spec>,
}

fn from_state((_serial, physical, logical, properties): CurrentState) -> Option<Vec<NativeScreen>> {
    if physical.len() > 128 || logical.len() > 128 {
        return None;
    }
    let mode = match properties.get("layout-mode") {
        Some(value) => u32::try_from(value).ok()?,
        None => 1, // The documented default is logical coordinates.
    };
    if ![1, 2].contains(&mode) {
        return None;
    }
    let mut outputs = Vec::new();
    for (spec, modes, _) in physical {
        if !valid_spec(&spec) || modes.len() > 2048 {
            return None;
        }
        let mut current = None;
        for (id, width, height, refresh, _, _, properties) in modes {
            let active = match properties.get("is-current") {
                Some(value) => bool::try_from(value).ok()?,
                None => false,
            };
            if active {
                if current.is_some() || id.len() > 512 || !refresh.is_finite() {
                    return None;
                }
                current = Some((id, width, height, refresh.to_bits()));
            }
        }
        outputs.push(Output { spec, current });
    }
    outputs.sort_by(|a, b| a.spec.cmp(&b.spec).then(a.current.cmp(&b.current)));
    let mut layouts = Vec::new();
    for (x, y, scale, transform, primary, mut monitors, _) in logical {
        if monitors.is_empty()
            || monitors.len() > 32
            || monitors.iter().any(|spec| !valid_spec(spec))
            || !scale.is_finite()
            || !(0.25..=16.0).contains(&scale)
            || transform > 7
        {
            return None;
        }
        monitors.sort();
        layouts.push(Layout {
            x,
            y,
            scale_bits: scale.to_bits(),
            transform,
            primary,
            monitors,
        });
    }
    layouts.sort_by(|a, b| {
        (a.x, a.y, &a.monitors, a.scale_bits, a.transform, a.primary).cmp(&(
            b.x,
            b.y,
            &b.monitors,
            b.scale_bits,
            b.transform,
            b.primary,
        ))
    });
    // Hash the actual configuration, not the query serial or a polling counter.
    // Connecting even an inactive output invalidates a saved geometry epoch.
    let bytes = serde_json::to_vec(&(mode, &outputs, &layouts)).ok()?;
    let digest = Sha256::digest(bytes);
    let epoch = u64::from_be_bytes(digest[..8].try_into().ok()?).max(1);
    let mut areas = Vec::new();
    for layout in &layouts {
        let mut rect = None;
        for spec in &layout.monitors {
            let mut matches = outputs.iter().filter(|output| &output.spec == spec);
            let output = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            let candidate = rectangle(layout, output, mode)?;
            if rect.is_some_and(|rect| rect != candidate) {
                return None;
            }
            rect = Some(candidate);
        }
        areas.push(rect?);
    }
    let mut result = Vec::new();
    for (index, layout) in layouts.iter().enumerate() {
        // Mirrored outputs and overlapping logical areas are not unique screens.
        if layout.monitors.len() != 1
            || areas
                .iter()
                .enumerate()
                .any(|(other, area)| other != index && overlaps(areas[index], *area))
        {
            continue;
        }
        let spec = &layout.monitors[0];
        let normalized = connector(&spec.0)?;
        if outputs
            .iter()
            .filter(|output| connector(&output.spec.0).as_ref() == Some(&normalized))
            .count()
            != 1
            || layouts
                .iter()
                .flat_map(|layout| &layout.monitors)
                .filter(|candidate| connector(&candidate.0).as_ref() == Some(&normalized))
                .count()
                != 1
        {
            continue;
        }
        result.push(NativeScreen {
            native_id: spec.0.clone(),
            rect: areas[index],
            epoch,
        });
    }
    Some(result)
}

fn valid_spec(spec: &Spec) -> bool {
    connector(&spec.0).is_some()
        && [&spec.0, &spec.1, &spec.2, &spec.3]
            .iter()
            .all(|part| part.len() <= 256 && !part.chars().any(char::is_control))
}
fn rectangle(layout: &Layout, output: &Output, mode: u32) -> Option<ScreenRect> {
    let (_, width, height, _) = output.current.as_ref()?;
    if *width < 2 || *height < 2 || *width > 65_536 || *height > 65_536 {
        return None;
    }
    let scale = f64::from_bits(layout.scale_bits) as f32;
    // Mutter derives logical sizes using roundf, then applies rotation.
    let scaled = |size: i32| {
        if mode == 1 {
            (size as f32 / scale).round() as u32
        } else {
            size as u32
        }
    };
    let (mut width, mut height) = (scaled(*width), scaled(*height));
    if layout.transform % 2 == 1 {
        std::mem::swap(&mut width, &mut height);
    }
    if width < 2
        || height < 2
        || width > 65_536
        || height > 65_536
        || i64::from(layout.x) + i64::from(width) > i64::from(i32::MAX)
        || i64::from(layout.y) + i64::from(height) > i64::from(i32::MAX)
    {
        return None;
    }
    Some(ScreenRect {
        x: layout.x,
        y: layout.y,
        width,
        height,
    })
}
fn overlaps(a: ScreenRect, b: ScreenRect) -> bool {
    i64::from(a.x) < i64::from(b.x) + i64::from(b.width)
        && i64::from(b.x) < i64::from(a.x) + i64::from(a.width)
        && i64::from(a.y) < i64::from(b.y) + i64::from(b.height)
        && i64::from(b.y) < i64::from(a.y) + i64::from(a.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_pointer_preserves_actionable_companion_status() {
        let classify = |error| pointer_error(zbus::Error::FDO(Box::new(error)));
        assert_eq!(
            classify(zbus::fdo::Error::UnknownMethod("fixture".into())),
            PointerError::ReloadCompanion
        );
        assert_eq!(
            classify(zbus::fdo::Error::ServiceUnknown("fixture".into())),
            PointerError::EnableCompanion
        );
        assert_eq!(
            classify(zbus::fdo::Error::AccessDenied("fixture".into())),
            PointerError::NotAuthorized
        );
        assert_eq!(
            pointer_error(zbus::Error::Failure("fixture".into())),
            PointerError::Unavailable
        );
    }
    fn output(name: &str, width: i32, height: i32) -> Physical {
        (
            (name.into(), "Vendor".into(), "Panel".into(), name.into()),
            vec![(
                "current".into(),
                width,
                height,
                60.0,
                1.0,
                vec![1.0, 1.25],
                HashMap::from([("is-current".into(), true.into())]),
            )],
            HashMap::new(),
        )
    }
    fn logical(output: &Physical, x: i32, y: i32, scale: f64, transform: u32) -> Logical {
        (
            x,
            y,
            scale,
            transform,
            false,
            vec![output.0.clone()],
            HashMap::new(),
        )
    }
    #[test]
    fn maps_negative_scaled_rotated_geometry_and_ignores_query_serials() {
        let a = output("DP-1", 2560, 1440);
        let b = output("HDMI-1", 1920, 1080);
        let la = logical(&a, -2048, 100, 1.25, 0);
        let lb = logical(&b, 0, 0, 1.0, 1);
        let first = from_state((1, vec![a, b], vec![la, lb], HashMap::new())).unwrap();
        assert_eq!(
            first[0].rect,
            ScreenRect {
                x: -2048,
                y: 100,
                width: 2048,
                height: 1152
            }
        );
        assert_eq!(
            first[1].rect,
            ScreenRect {
                x: 0,
                y: 0,
                width: 1080,
                height: 1920
            }
        );
        let a = output("DP-1", 2560, 1440);
        let b = output("HDMI-1", 1920, 1080);
        let la = logical(&a, -2048, 100, 1.25, 0);
        let lb = logical(&b, 0, 0, 1.0, 1);
        let repeated = from_state((99, vec![b, a], vec![lb, la], HashMap::new())).unwrap();
        assert_eq!(first, repeated);
        let ids = vec![
            format!("card1-HDMI-A-1:{}", "a".repeat(64)),
            "card0-DP-1".into(),
        ];
        assert_eq!(map_connectors(&ids, first.clone()).len(), 2);
        let ambiguous = vec!["card0-DP-1".into(), "card1-DP-1".into()];
        assert!(map_connectors(&ambiguous, first).is_empty());
    }
    #[test]
    fn rejects_mirrors_overlaps_and_missing_current_modes() {
        let a = output("DP-1", 1920, 1080);
        let b = output("HDMI-1", 1920, 1080);
        let mut mirrored = logical(&a, 0, 0, 1.0, 0);
        mirrored.5.push(b.0.clone());
        assert!(
            from_state((1, vec![a, b], vec![mirrored], HashMap::new()))
                .unwrap()
                .is_empty()
        );
        let a = output("DP-1", 1920, 1080);
        let b = output("HDMI-1", 1920, 1080);
        let la = logical(&a, 0, 0, 1.0, 0);
        let lb = logical(&b, 100, 100, 1.0, 0);
        assert!(
            from_state((1, vec![a, b], vec![la, lb], HashMap::new()))
                .unwrap()
                .is_empty()
        );
        let mut a = output("DP-1", 1920, 1080);
        let la = logical(&a, 0, 0, 1.0, 0);
        a.1.clear();
        assert!(from_state((1, vec![a], vec![la], HashMap::new())).is_none());
    }

    #[test]
    fn epochs_change_with_scale_or_hotplug_and_card_slash_names_are_exact() {
        assert_eq!(connector("card2/HDMI-A-1"), Some("HDMI-1".into()));
        assert!(connector("cardwrong-HDMI-A-1").is_none());
        assert!(connector("card1-DP-1:not-an-edid").is_none());
        let a = output("DP-1", 1920, 1080);
        let la = logical(&a, 0, 0, 1.0, 0);
        let original = from_state((1, vec![a], vec![la], HashMap::new())).unwrap();
        let a = output("DP-1", 1920, 1080);
        let la = logical(&a, 0, 0, 1.5, 0);
        let scaled = from_state((1, vec![a], vec![la], HashMap::new())).unwrap();
        assert_ne!(original[0].epoch, scaled[0].epoch);
        let a = output("DP-1", 1920, 1080);
        let la = logical(&a, 0, 0, 1.0, 0);
        let mut inactive = output("HDMI-1", 1920, 1080);
        inactive.1[0].6.clear();
        let connected = from_state((1, vec![a, inactive], vec![la], HashMap::new())).unwrap();
        assert_ne!(original[0].epoch, connected[0].epoch);
        assert_eq!(connected.len(), 1);
    }

    #[test]
    fn geometry_rpc_uses_a_private_bus_only() {
        const MARKER: &str = "DESK_SWITCH_GEOMETRY_PRIVATE_BUS";
        if std::env::var(MARKER).as_deref() != Ok("1") {
            let result = std::process::Command::new("dbus-run-session")
                .arg("--")
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "platform::linux_geometry::tests::geometry_rpc_uses_a_private_bus_only",
                    "--nocapture",
                ])
                .env(MARKER, "1")
                .env(
                    "DESK_SWITCH_GEOMETRY_PARENT_BUS",
                    std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default(),
                )
                .output()
                .expect("dbus-run-session is required for the isolated geometry RPC test");
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
        assert!(!address.is_empty());
        assert_ne!(
            Some(address),
            std::env::var("DESK_SWITCH_GEOMETRY_PARENT_BUS").ok()
        );
        struct DisplayFixture;
        #[zbus::interface(name = "org.gnome.Mutter.DisplayConfig")]
        impl DisplayFixture {
            fn get_current_state(&self) -> CurrentState {
                let display = output("DP-1", 2560, 1440);
                let area = logical(&display, -2048, 100, 1.25, 0);
                (1, vec![display], vec![area], HashMap::new())
            }
        }
        struct PointerFixture;
        #[zbus::interface(name = "io.github.siarheisinelnikau.DeskSwitch.Clipboard")]
        impl PointerFixture {
            fn pointer_state(&self) -> (i32, i32) {
                (-1000, 500)
            }
        }
        let _display = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name(DISPLAY_NAME)
            .unwrap()
            .serve_at(DISPLAY_PATH, DisplayFixture)
            .unwrap()
            .build()
            .unwrap();
        let _pointer = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name(COMPANION_NAME)
            .unwrap()
            .serve_at(COMPANION_PATH, PointerFixture)
            .unwrap()
            .build()
            .unwrap();
        let screens = snapshot().unwrap();
        assert_eq!(screens.len(), 1);
        assert_eq!(screens[0].native_id, "DP-1");
        assert_eq!(
            screens[0].rect,
            ScreenRect {
                x: -2048,
                y: 100,
                width: 2048,
                height: 1152
            }
        );
        assert_eq!(pointer(), Ok((-1000, 500)));
    }
}
