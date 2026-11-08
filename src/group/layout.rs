//! A workspace arrangement describes input edges; it never changes OS display
//! settings or issues DDC commands. OS-relative groups move as rigid components.
use super::model::{Key, Workspace};
use crate::edge::{Boundary, Crossing, Entry, NativeScreen, Routes, ScreenRect, Side};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const SHARED: &str = "shared";
const LIMIT: i32 = 1_000_000;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Layout {
    pub revision: u64,
    pub positions: BTreeMap<String, Point>,
    pub shared_size: Option<Size>,
}
impl Layout {
    pub fn validate_shape(&self) -> Result<()> {
        ensure!(
            self.revision < u64::MAX && self.positions.len() <= 4096,
            "Invalid display arrangement size or revision"
        );
        ensure!(
            self.positions.iter().all(|(key, point)| (key == SHARED
                || super::model::valid_key(key).is_ok())
                && point.x.abs_diff(0) <= LIMIT as u32
                && point.y.abs_diff(0) <= LIMIT as u32),
            "Invalid display arrangement component or position"
        );
        if let Some(size) = self.shared_size {
            ensure!(
                size.width >= 2 && size.height >= 2 && size.width <= 65536 && size.height <= 65536,
                "Invalid shared display size"
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DisplayTarget {
    Ordinary { computer: Key, display: String },
    Shared { computer: Key },
}
impl DisplayTarget {
    pub fn computer(&self) -> &str {
        match self {
            Self::Ordinary { computer, .. } | Self::Shared { computer } => computer,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Activation {
    pub target: DisplayTarget,
    pub entry: Entry,
}
#[derive(Clone, Debug)]
pub struct Surface {
    pub id: String,
    pub label: String,
    pub computer: Option<Key>,
    pub component: String,
    pub rect: Option<ScreenRect>,
    pub target: Option<DisplayTarget>,
    pub shared: bool,
    pub online: bool,
    pub available: bool,
    pub reason: Option<String>,
    pub native: Option<NativeScreen>,
}
#[derive(Clone, Debug)]
pub struct Component {
    pub id: String,
    pub label: String,
    pub surfaces: Vec<String>,
    pub bounds: Option<ScreenRect>,
    pub origin: Point,
}
#[derive(Clone, Debug)]
pub struct Contact {
    pub from: String,
    pub to: String,
    pub side: Side,
    pub start: u16,
    pub end: u16,
    pub id: u64,
}
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub surfaces: Vec<Surface>,
    pub components: Vec<Component>,
    pub contacts: Vec<Contact>,
    pub problems: Vec<String>,
    pub revision: u64,
}

pub fn configured_shared(state: &Workspace, computer: &str) -> bool {
    state.members.get(computer).is_some_and(|member| {
        member.node.monitor.enabled
            && !member.node.monitor.id.is_empty()
            && member.node.monitor.input.is_some_and(|input| input != 0)
    })
}
pub fn shared_binding<'a>(
    state: &'a Workspace,
    computer: &str,
) -> Option<&'a crate::workspace::devices::Display> {
    let member = state.members.get(computer)?;
    let monitor = &member.node.monitor;
    configured_shared(state, computer)
        .then(|| {
            member
                .node
                .devices
                .displays
                .iter()
                .find(|display| display.id == monitor.id)
        })
        .flatten()
}
pub fn native_target<'a>(state: &'a Workspace, target: &DisplayTarget) -> Result<&'a NativeScreen> {
    let member = state
        .members
        .get(target.computer())
        .filter(|member| member.compatible())
        .context("The destination is offline or needs an update")?;
    let display = match target {
        DisplayTarget::Ordinary { display, .. } => {
            ensure!(
                shared_binding(state, target.computer()).is_none_or(|shared| shared.id != *display),
                "Use the shared monitor target for this display"
            );
            member
                .node
                .devices
                .displays
                .iter()
                .find(|found| found.id == *display)
                .context("The destination display is unavailable")?
        }
        DisplayTarget::Shared { .. } => shared_binding(state, target.computer())
            .context("Configure this computer's shared monitor connection first")?,
    };
    let native = display
        .native
        .as_ref()
        .context("Exact input geometry is unavailable for this display")?;
    native.validate()?;
    ensure!(
        member
            .node
            .devices
            .displays
            .iter()
            .filter_map(|display| display.native.as_ref())
            .filter(|found| found.rect == native.rect)
            .count()
            == 1,
        "The destination display geometry is ambiguous"
    );
    Ok(native)
}

fn id(computer: &str, display: &str) -> String {
    format!("{computer}:{:x}", Sha256::digest(display.as_bytes()))
}
fn hash(bytes: &[u8]) -> u64 {
    u64::from_be_bytes(Sha256::digest(bytes)[..8].try_into().unwrap()).max(1)
}
fn right(rect: ScreenRect) -> i64 {
    i64::from(rect.x) + i64::from(rect.width)
}
fn bottom(rect: ScreenRect) -> i64 {
    i64::from(rect.y) + i64::from(rect.height)
}
fn union(a: ScreenRect, b: ScreenRect) -> ScreenRect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    ScreenRect {
        x,
        y,
        width: (right(a).max(right(b)) - i64::from(x)) as u32,
        height: (bottom(a).max(bottom(b)) - i64::from(y)) as u32,
    }
}
fn valid_world(rect: ScreenRect) -> bool {
    rect.validate().is_ok()
        && rect.x.abs_diff(0) <= LIMIT as u32
        && rect.y.abs_diff(0) <= LIMIT as u32
        && right(rect) <= i64::from(LIMIT)
        && bottom(rect) <= i64::from(LIMIT)
}
fn at(
    native: ScreenRect,
    anchor: ScreenRect,
    origin: Point,
    size: Option<Size>,
) -> Option<ScreenRect> {
    let scale = size
        .map(|size| f64::from(size.width) / f64::from(anchor.width))
        .unwrap_or(1.0);
    if let Some(size) = size
        && u64::from(size.width) * u64::from(anchor.height)
            != u64::from(size.height) * u64::from(anchor.width)
    {
        return None;
    }
    let x =
        (f64::from(origin.x) + (f64::from(native.x) - f64::from(anchor.x)) * scale).round() as i32;
    let y =
        (f64::from(origin.y) + (f64::from(native.y) - f64::from(anchor.y)) * scale).round() as i32;
    let edge_x =
        (f64::from(origin.x) + (right(native) - i64::from(anchor.x)) as f64 * scale).round() as i32;
    let edge_y = (f64::from(origin.y) + (bottom(native) - i64::from(anchor.y)) as f64 * scale)
        .round() as i32;
    let rect = ScreenRect {
        x,
        y,
        width: (i64::from(edge_x) - i64::from(x)).max(0) as u32,
        height: (i64::from(edge_y) - i64::from(y)).max(0) as u32,
    };
    valid_world(rect).then_some(rect)
}

fn local_bounds(state: &Workspace, layout: &Layout, component: &str) -> Option<ScreenRect> {
    if component != SHARED {
        let bounds = state
            .members
            .get(component)?
            .node
            .devices
            .displays
            .iter()
            .filter_map(|display| display.native.as_ref())
            .map(|native| native.rect)
            .reduce(union)?;
        return Some(ScreenRect {
            x: 0,
            y: 0,
            width: bounds.width,
            height: bounds.height,
        });
    }
    let size = layout.shared_size?;
    let mut bounds = ScreenRect {
        x: 0,
        y: 0,
        width: size.width,
        height: size.height,
    };
    for computer in state
        .members
        .keys()
        .filter(|key| configured_shared(state, key))
    {
        if let Some(shared) =
            shared_binding(state, computer).and_then(|display| display.native.as_ref())
        {
            for native in state.members[computer]
                .node
                .devices
                .displays
                .iter()
                .filter_map(|display| display.native.as_ref())
            {
                if let Some(rect) = at(native.rect, shared.rect, Point::default(), Some(size)) {
                    bounds = union(bounds, rect);
                }
            }
        }
    }
    Some(bounds)
}

pub fn draft(state: &Workspace) -> Layout {
    let mut layout = state.layout.clone();
    if layout.shared_size.is_none() {
        layout.shared_size = state
            .members
            .keys()
            .filter_map(|key| shared_binding(state, key))
            .filter_map(|display| display.native.as_ref())
            .next()
            .map(|native| Size {
                width: native.rect.width,
                height: native.rect.height,
            });
    }
    let known = state
        .members
        .keys()
        .map(|key| {
            if configured_shared(state, key) {
                SHARED.to_owned()
            } else {
                key.clone()
            }
        })
        .collect::<BTreeSet<_>>();
    layout.positions.retain(|key, _| known.contains(key));
    let mut x = layout
        .positions
        .iter()
        .filter_map(|(key, origin)| {
            local_bounds(state, &layout, key).map(|bounds| i64::from(origin.x) + right(bounds) + 64)
        })
        .max()
        .unwrap_or(0)
        .clamp(0, i64::from(LIMIT)) as i32;
    for key in state.members.keys() {
        let component = if configured_shared(state, key) {
            SHARED
        } else {
            key
        };
        if !layout.positions.contains_key(component) {
            let bounds = local_bounds(state, &layout, component);
            let origin = bounds
                .map(|bounds| Point {
                    x: x.saturating_sub(bounds.x),
                    y: bounds.y.saturating_neg(),
                })
                .unwrap_or(Point { x, y: 0 });
            layout.positions.insert(component.into(), origin);
            x = x
                .saturating_add(
                    bounds
                        .map(|bounds| bounds.width.min(LIMIT as u32) as i32)
                        .unwrap_or(0),
                )
                .saturating_add(64);
        }
    }
    layout
}

#[derive(Clone, PartialEq, Serialize)]
struct RoutingMember {
    key: Key,
    compatible: bool,
    binding: Option<String>,
    displays: Vec<(String, Option<NativeScreen>)>,
}
#[derive(Clone, PartialEq, Serialize)]
struct RoutingStamp {
    layout: Layout,
    owner: Option<Key>,
    members: Vec<RoutingMember>,
}
impl RoutingStamp {
    fn capture(state: &Workspace, layout: &Layout) -> Self {
        Self {
            layout: layout.clone(),
            owner: state.confirmed_monitor.clone(),
            members: state
                .members
                .iter()
                .map(|(key, member)| RoutingMember {
                    key: key.clone(),
                    compatible: member.compatible(),
                    binding: configured_shared(state, key).then(|| member.node.monitor.id.clone()),
                    displays: member
                        .node
                        .devices
                        .displays
                        .iter()
                        .map(|display| (display.id.clone(), display.native.clone()))
                        .collect(),
                })
                .collect(),
        }
    }
    fn matches(&self, state: &Workspace) -> bool {
        self.layout == state.layout
            && self.owner == state.confirmed_monitor
            && self.members.len() == state.members.len()
            && self
                .members
                .iter()
                .zip(&state.members)
                .all(|(saved, (key, member))| {
                    saved.key == *key
                        && saved.compatible == member.compatible()
                        && saved.binding.as_deref()
                            == configured_shared(state, key)
                                .then_some(member.node.monitor.id.as_str())
                        && saved.displays.len() == member.node.devices.displays.len()
                        && saved
                            .displays
                            .iter()
                            .zip(&member.node.devices.displays)
                            .all(|((id, native), display)| {
                                id == &display.id && native == &display.native
                            })
                })
    }
}

#[derive(Default)]
pub struct RoutingCache {
    stamp: Option<RoutingStamp>,
    scene: Option<Scene>,
    routes: BTreeMap<Key, Routes>,
    #[cfg(test)]
    builds: usize,
}
impl RoutingCache {
    pub fn routes_for(&mut self, state: &Workspace, computer: &str) -> Routes {
        if !routing_enabled(state) {
            return Routes {
                activation: state.activation,
                ..Default::default()
            };
        }
        if !self
            .stamp
            .as_ref()
            .is_some_and(|stamp| stamp.matches(state))
        {
            self.stamp = Some(RoutingStamp::capture(state, &state.layout));
            self.scene = Some(scene(state, &state.layout));
            self.routes.clear();
            #[cfg(test)]
            {
                self.builds += 1;
            }
        }
        let scene = self.scene.as_ref().unwrap();
        let routes = self
            .routes
            .entry(computer.into())
            .or_insert_with(|| routes_from_scene(state, scene, computer));
        let mut routes = routes.clone();
        routes.activation = state.activation;
        routes
    }
}

pub fn scene(state: &Workspace, layout: &Layout) -> Scene {
    let mut output = Scene::default();
    let bindings = state
        .members
        .keys()
        .filter(|key| configured_shared(state, key))
        .collect::<Vec<_>>();
    let size = layout.shared_size;
    let shared_origin = layout.positions.get(SHARED).copied();
    for (computer, member) in &state.members {
        let binding = shared_binding(state, computer);
        let participating = configured_shared(state, computer);
        let component = if participating { SHARED } else { computer };
        let origin = layout.positions.get(component).copied();
        let anchor = if participating {
            binding
                .and_then(|binding| binding.native.as_ref())
                .map(|native| native.rect)
        } else {
            member
                .node
                .devices
                .displays
                .iter()
                .filter_map(|display| display.native.as_ref())
                .map(|native| native.rect)
                .reduce(union)
        };
        for display in &member.node.devices.displays {
            if binding.is_some_and(|shared| shared.id == display.id) {
                continue;
            }
            let native = display.native.clone();
            let rect =
                origin
                    .zip(anchor)
                    .zip(native.as_ref())
                    .and_then(|((origin, anchor), native)| {
                        at(
                            native.rect,
                            anchor,
                            origin,
                            if participating { size } else { None },
                        )
                    });
            let target = DisplayTarget::Ordinary {
                computer: computer.clone(),
                display: display.id.clone(),
            };
            let reason = if !member.compatible() {
                Some("The computer is offline or needs an update".into())
            } else if rect.is_none() {
                Some("Arrange this component after exact display geometry is available".into())
            } else {
                native_target(state, &target)
                    .err()
                    .map(|error| error.to_string())
            };
            output.surfaces.push(Surface {
                id: id(computer, &display.id),
                label: format!("{} · {}", member.node.name, display.name),
                computer: Some(computer.clone()),
                component: component.into(),
                rect,
                target: Some(target),
                shared: false,
                online: member.online,
                available: reason.is_none(),
                reason,
                native,
            });
        }
    }
    if !bindings.is_empty() {
        let owner = state
            .confirmed_monitor
            .as_ref()
            .filter(|key| configured_shared(state, key))
            .cloned();
        let target = owner
            .clone()
            .map(|computer| DisplayTarget::Shared { computer });
        let native = target
            .as_ref()
            .and_then(|target| native_target(state, target).ok())
            .cloned();
        let rect = shared_origin
            .zip(size)
            .map(|(origin, size)| ScreenRect {
                x: origin.x,
                y: origin.y,
                width: size.width,
                height: size.height,
            })
            .filter(|rect| valid_world(*rect));
        let reason = if owner.is_none() {
            Some("Choose the computer currently shown on the shared monitor".into())
        } else if native.is_none() {
            Some("Exact input geometry is unavailable for the shared monitor".into())
        } else if rect.is_none() {
            Some("Arrange the shared monitor".into())
        } else {
            None
        };
        output.surfaces.push(Surface {
            id: SHARED.into(),
            label: "Shared monitor".into(),
            computer: owner.clone(),
            component: SHARED.into(),
            rect,
            target,
            shared: true,
            online: owner.as_ref().is_some_and(|key| state.members[key].online),
            available: reason.is_none(),
            reason,
            native,
        });
    }
    for first in 0..output.surfaces.len() {
        for second in first + 1..output.surfaces.len() {
            let a = &output.surfaces[first];
            let b = &output.surfaces[second];
            if let Some((ar, br)) = a.rect.zip(b.rect)
                && i64::from(ar.x) < right(br)
                && i64::from(br.x) < right(ar)
                && i64::from(ar.y) < bottom(br)
                && i64::from(br.y) < bottom(ar)
            {
                let reason = format!("Displays overlap: {} and {}", a.label, b.label);
                output.problems.push(reason.clone());
                for index in [first, second] {
                    output.surfaces[index].available = false;
                    output.surfaces[index].reason = Some(reason.clone());
                }
            }
        }
    }
    for surface in &output.surfaces {
        if let Some(component) = output
            .components
            .iter_mut()
            .find(|component| component.id == surface.component)
        {
            component.surfaces.push(surface.id.clone());
            component.bounds = match (component.bounds, surface.rect) {
                (Some(a), Some(b)) => Some(union(a, b)),
                (a, b) => a.or(b),
            };
        } else {
            output.components.push(Component {
                id: surface.component.clone(),
                label: if surface.component == SHARED {
                    "Shared monitor group".into()
                } else {
                    state.members[&surface.component].node.name.clone()
                },
                surfaces: vec![surface.id.clone()],
                bounds: surface.rect,
                origin: layout
                    .positions
                    .get(&surface.component)
                    .copied()
                    .unwrap_or_default(),
            });
        }
    }
    for source in &output.surfaces {
        for target in &output.surfaces {
            if source.id == target.id {
                continue;
            }
            if let Some((a, b)) = source.rect.zip(target.rect) {
                for side in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
                    let touching = match side {
                        Side::Left => i64::from(a.x) == right(b),
                        Side::Right => right(a) == i64::from(b.x),
                        Side::Top => i64::from(a.y) == bottom(b),
                        Side::Bottom => bottom(a) == i64::from(b.y),
                    };
                    if !touching {
                        continue;
                    }
                    let (origin, extent, start, end) = match side {
                        Side::Left | Side::Right => (
                            i64::from(a.y),
                            a.height,
                            i64::from(a.y.max(b.y)),
                            bottom(a).min(bottom(b)),
                        ),
                        Side::Top | Side::Bottom => (
                            i64::from(a.x),
                            a.width,
                            i64::from(a.x.max(b.x)),
                            right(a).min(right(b)),
                        ),
                    };
                    let Some((start, end)) = normalized_interval(origin, extent, start, end) else {
                        continue;
                    };
                    let identity = format!("{}|{}|{side:?}|{start}|{end}", source.id, target.id);
                    output.contacts.push(Contact {
                        from: source.id.clone(),
                        to: target.id.clone(),
                        side,
                        start,
                        end,
                        id: hash(identity.as_bytes()),
                    });
                }
            }
        }
    }
    output.revision = hash(
        &serde_json::to_vec(&RoutingStamp::capture(state, layout))
            .expect("Integer display routing metadata is serializable"),
    );
    output
}

pub fn validate(state: &Workspace, layout: &Layout) -> Result<()> {
    layout.validate_shape()?;
    ensure!(
        layout.revision < u64::MAX,
        "Display arrangement revision exhausted"
    );
    if let Some(size) = layout.shared_size {
        ensure!(
            size.width >= 2 && size.height >= 2 && size.width <= 65536 && size.height <= 65536,
            "Invalid shared display size"
        );
    }
    let expected = state
        .members
        .keys()
        .map(|key| {
            if configured_shared(state, key) {
                SHARED.to_owned()
            } else {
                key.clone()
            }
        })
        .collect::<BTreeSet<_>>();
    ensure!(
        layout.positions.len() <= expected.len()
            && layout.positions.keys().all(|key| expected.contains(key)),
        "Unknown display arrangement component"
    );
    ensure!(layout.positions.values().all(|point|point.x.abs_diff(0)<=LIMIT as u32 && point.y.abs_diff(0)<=LIMIT as u32),"Display arrangement is out of range");
    for (component, origin) in &layout.positions {
        if let Some(bounds) = local_bounds(state, layout, component) {
            let x = i64::from(origin.x) + i64::from(bounds.x);
            let y = i64::from(origin.y) + i64::from(bounds.y);
            ensure!(
                x >= -i64::from(LIMIT)
                    && y >= -i64::from(LIMIT)
                    && x + i64::from(bounds.width) <= i64::from(LIMIT)
                    && y + i64::from(bounds.height) <= i64::from(LIMIT),
                "Display component is out of range"
            );
        }
    }
    let scene = scene(state, layout);
    ensure!(scene.problems.is_empty(), "{}", scene.problems.join("; "));
    Ok(())
}
pub fn move_component(
    state: &Workspace,
    layout: &Layout,
    component: &str,
    mut origin: Point,
    snap: i32,
) -> Layout {
    origin.x = origin.x.clamp(-LIMIT, LIMIT);
    origin.y = origin.y.clamp(-LIMIT, LIMIT);
    let before = scene(state, layout);
    if snap > 0
        && let Some(bounds) = before
            .components
            .iter()
            .find(|found| found.id == component)
            .and_then(|component| component.bounds)
    {
        let old = layout.positions.get(component).copied().unwrap_or_default();
        let proposed = ScreenRect {
            x: bounds.x.saturating_add(origin.x.saturating_sub(old.x)),
            y: bounds.y.saturating_add(origin.y.saturating_sub(old.y)),
            ..bounds
        };
        let mut dx = 0i64;
        let mut dy = 0i64;
        for target in before
            .surfaces
            .iter()
            .filter(|surface| surface.component != component)
            .filter_map(|surface| surface.rect)
        {
            for difference in [
                right(target) - i64::from(proposed.x),
                i64::from(target.x) - right(proposed),
            ] {
                if difference.abs() <= i64::from(snap) && (dx == 0 || difference.abs() < dx.abs()) {
                    dx = difference;
                }
            }
            for difference in [
                bottom(target) - i64::from(proposed.y),
                i64::from(target.y) - bottom(proposed),
            ] {
                if difference.abs() <= i64::from(snap) && (dy == 0 || difference.abs() < dy.abs()) {
                    dy = difference;
                }
            }
        }
        origin.x = origin.x.saturating_add(dx as i32);
        origin.y = origin.y.saturating_add(dy as i32);
    }
    let mut moved = layout.clone();
    moved.positions.insert(component.into(), origin);
    moved
}

fn opposite(side: Side) -> Side {
    match side {
        Side::Left => Side::Right,
        Side::Right => Side::Left,
        Side::Top => Side::Bottom,
        Side::Bottom => Side::Top,
    }
}
fn routable<'a>(scene: &'a Scene, contact: &Contact) -> Option<(&'a Surface, &'a Surface)> {
    let source = scene
        .surfaces
        .iter()
        .find(|surface| surface.id == contact.from)?;
    let target = scene
        .surfaces
        .iter()
        .find(|surface| surface.id == contact.to)?;
    (source.available && target.available).then_some((source, target))
}
pub fn routes_for(state: &Workspace, computer: &str) -> Routes {
    if !routing_enabled(state) {
        return Routes {
            activation: state.activation,
            ..Default::default()
        };
    }
    let scene = scene(state, &state.layout);
    routes_from_scene(state, &scene, computer)
}
fn routing_enabled(state: &Workspace) -> bool {
    state.settings.edge
        && state.settings.mouse_enabled
        && state.layout.revision > 0
        && !state.switch_pending()
}
fn routes_from_scene(state: &Workspace, scene: &Scene, computer: &str) -> Routes {
    let mut routes = Routes {
        revision: scene.revision,
        activation: state.activation,
        boundaries: Vec::new(),
    };
    for contact in &scene.contacts {
        if let Some((source, target)) = routable(scene, contact)
            && source.computer.as_deref() == Some(computer)
            && target.computer.as_deref() != Some(computer)
            && let Some(native) = &source.native
        {
            routes.boundaries.push(Boundary {
                id: contact.id,
                screen: native.rect,
                geometry: native.epoch,
                side: contact.side,
                start: contact.start,
                end: contact.end,
            });
        }
    }
    // The OS can still expose this PC's shared output while its physical input
    // shows another PC. Protect only that adjacent native segment; ordinary
    // same-PC boundaries remain the operating system's responsibility.
    if state.confirmed_monitor.as_deref() != Some(computer)
        && let Some(shared) =
            shared_binding(state, computer).and_then(|display| display.native.as_ref())
        && let Some(member) = state.members.get(computer)
    {
        for display in &member.node.devices.displays {
            if let Some(native) = &display.native
                && native.rect != shared.rect
                && member
                    .node
                    .devices
                    .displays
                    .iter()
                    .filter_map(|display| display.native.as_ref())
                    .filter(|screen| screen.rect == native.rect)
                    .count()
                    == 1
            {
                for side in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
                    if let Some((start, end)) = native_segment(native.rect, shared.rect, side) {
                        let represented = routes
                            .boundaries
                            .iter()
                            .filter(|boundary| {
                                boundary.screen == native.rect
                                    && boundary.geometry == native.epoch
                                    && boundary.side == side
                            })
                            .copied()
                            .collect::<Vec<_>>();
                        let mut remaining = vec![(start, end)];
                        for boundary in represented {
                            remaining = remaining
                                .into_iter()
                                .flat_map(|(a, b)| {
                                    let mut pieces = Vec::new();
                                    if boundary.end <= a || boundary.start >= b {
                                        pieces.push((a, b));
                                    } else {
                                        if a < boundary.start {
                                            pieces.push((a, boundary.start));
                                        }
                                        if boundary.end < b {
                                            pieces.push((boundary.end, b));
                                        }
                                    }
                                    pieces
                                })
                                .collect();
                        }
                        for (start, end) in remaining {
                            if start < end {
                                routes.boundaries.push(Boundary {
                                    id: 0,
                                    screen: native.rect,
                                    geometry: native.epoch,
                                    side,
                                    start,
                                    end,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    if routes.validate().is_err() {
        routes.boundaries.clear();
    }
    routes
}

fn native_segment(a: ScreenRect, b: ScreenRect, side: Side) -> Option<(u16, u16)> {
    let touching = match side {
        Side::Left => i64::from(a.x) == right(b),
        Side::Right => right(a) == i64::from(b.x),
        Side::Top => i64::from(a.y) == bottom(b),
        Side::Bottom => bottom(a) == i64::from(b.y),
    };
    if !touching {
        return None;
    }
    let (origin, extent, start, end) = match side {
        Side::Left | Side::Right => (
            i64::from(a.y),
            a.height,
            i64::from(a.y.max(b.y)),
            bottom(a).min(bottom(b)),
        ),
        Side::Top | Side::Bottom => (
            i64::from(a.x),
            a.width,
            i64::from(a.x.max(b.x)),
            right(a).min(right(b)),
        ),
    };
    normalized_interval(origin, extent, start, end)
}

fn normalized_interval(origin: i64, extent: u32, start: i64, end: i64) -> Option<(u16, u16)> {
    if extent < 2 || start >= end {
        return None;
    }
    let normalize = |coordinate: i64| {
        (((coordinate - origin) * 65535 + (i64::from(extent) - 2)) / (i64::from(extent) - 1))
            .clamp(0, 65535) as u16
    };
    let start = normalize(start);
    let mut normalized_end = normalize(end);
    // MAX is inclusive in the native boundary protocol. A partial neighbor
    // ending at the source's last pixel must therefore stop below MAX.
    if normalized_end == u16::MAX && end < origin + i64::from(extent) {
        normalized_end = u16::MAX - 1;
    }
    let end = normalized_end;
    (start < end).then_some((start, end))
}
pub fn resolve_crossing(
    state: &Workspace,
    actor: &str,
    crossing: Crossing,
) -> Result<Option<Activation>> {
    ensure!(
        state.plan().input.as_deref() == Some(actor),
        "Only the current input owner may route an edge crossing"
    );
    if state.switch_pending()
        || !state.settings.edge
        || !state.settings.mouse_enabled
        || state.layout.revision == 0
        || crossing.activation != state.activation
    {
        return Ok(None);
    }
    let scene = scene(state, &state.layout);
    if crossing.revision != scene.revision {
        return Ok(None);
    }
    let Some(contact) = scene
        .contacts
        .iter()
        .find(|contact| contact.id == crossing.boundary)
    else {
        return Ok(None);
    };
    let (source, target) =
        routable(&scene, contact).context("The neighboring display is unavailable")?;
    ensure!(
        source.computer.as_deref() == state.active_input.as_deref(),
        "The edge belongs to another input target"
    );
    ensure!(
        routes_for(state, source.computer.as_deref().unwrap())
            .boundaries
            .iter()
            .any(
                |boundary| boundary.id == crossing.boundary && boundary.contains(crossing.position)
            ),
        "Unknown active edge boundary"
    );
    let native = source
        .native
        .as_ref()
        .context("Source screen geometry is unavailable")?;
    ensure!(
        Boundary {
            id: contact.id,
            screen: native.rect,
            geometry: native.epoch,
            side: contact.side,
            start: contact.start,
            end: contact.end
        }
        .contains(crossing.position),
        "The pointer is outside the neighboring edge segment"
    );
    let from = source.rect.unwrap();
    let to = target.rect.unwrap();
    let (from_start, from_span, to_start, to_span) = match contact.side {
        Side::Left | Side::Right => (from.y, from.height, to.y, to.height),
        Side::Top | Side::Bottom => (from.x, from.width, to.x, to.width),
    };
    let numerator = (i64::from(from_start) - i64::from(to_start)) * 65535
        + i64::from(crossing.position) * i64::from(from_span - 1);
    let position =
        ((numerator + i64::from(to_span - 1) / 2) / i64::from(to_span - 1)).clamp(0, 65535) as u16;
    let native = target
        .native
        .as_ref()
        .context("Destination screen geometry is unavailable")?;
    Ok(Some(Activation {
        target: target
            .target
            .clone()
            .context("Choose the shared monitor owner first")?,
        entry: Entry {
            screen: native.rect,
            geometry: native.epoch,
            side: opposite(contact.side),
            position,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::devices::Display;

    fn display(id: &str, rect: ScreenRect, epoch: u64) -> Display {
        Display {
            id: id.into(),
            name: id.into(),
            fingerprint: String::new(),
            native: Some(NativeScreen {
                native_id: id.into(),
                rect,
                epoch,
            }),
        }
    }
    fn fixture(count: usize) -> (Workspace, Vec<String>) {
        let mut state = super::super::model::tests::fixture(count);
        let keys = state.members.keys().cloned().collect::<Vec<_>>();
        for (index, key) in keys.iter().enumerate() {
            let member = state.members.get_mut(key).unwrap();
            member.node.monitor.enabled = false;
            member.node.devices.displays = vec![display(
                &format!("screen-{index}"),
                ScreenRect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                index as u64 + 1,
            )];
        }
        state.settings.edge = true;
        state.active_input = Some(keys[0].clone());
        state.activation = 1;
        state.layout = Layout {
            revision: 1,
            positions: keys
                .iter()
                .enumerate()
                .map(|(index, key)| {
                    (
                        key.clone(),
                        Point {
                            x: index as i32 * 100,
                            y: 0,
                        },
                    )
                })
                .collect(),
            shared_size: None,
        };
        (state, keys)
    }
    fn crossing(state: &Workspace, computer: &str, side: Side, position: u16) -> Crossing {
        let routes = routes_for(state, computer);
        let boundary = routes
            .boundaries
            .iter()
            .find(|boundary| boundary.id != 0 && boundary.side == side)
            .unwrap();
        Crossing {
            revision: routes.revision,
            activation: routes.activation,
            boundary: boundary.id,
            position,
        }
    }

    #[test]
    fn partial_neighbor_maps_to_the_exact_destination_screen_and_ordinary_activation_never_selects_ddc()
     {
        let (mut state, keys) = fixture(2);
        state
            .members
            .get_mut(&keys[0])
            .unwrap()
            .node
            .devices
            .displays[0]
            .native
            .as_mut()
            .unwrap()
            .rect
            .height = 200;
        state
            .members
            .get_mut(&keys[1])
            .unwrap()
            .node
            .devices
            .displays[0]
            .native
            .as_mut()
            .unwrap()
            .rect
            .x = 500;
        state
            .layout
            .positions
            .insert(keys[1].clone(), Point { x: 100, y: 100 });
        let event = crossing(&state, &keys[0], Side::Right, 49151);
        let activation = resolve_crossing(&state, &keys[0], event).unwrap().unwrap();
        assert_eq!(activation.entry.screen.x, 500);
        assert_eq!(activation.entry.side, Side::Left);
        assert!(activation.entry.position.abs_diff(u16::MAX / 2) < 1000);
        assert!(
            resolve_crossing(
                &state,
                &keys[0],
                Crossing {
                    position: 1000,
                    ..event
                }
            )
            .is_err()
        );
        let target = activation.target.clone();
        let entry = activation.entry;
        state
            .activate_display(&keys[0], target.clone(), Some(entry))
            .unwrap();
        assert!(state.monitor_command.is_none() && state.confirmed_monitor.is_none());
        let previous = state.activation;
        state
            .activate_display(&keys[0], target, Some(entry))
            .unwrap();
        assert_eq!(
            state.activation,
            previous + 1,
            "An explicit display entry can retry a failed native reposition"
        );
        assert!(state.monitor_command.is_none());
        assert!(resolve_crossing(&state, &keys[0], event).unwrap().is_none());
    }

    #[test]
    fn four_sides_use_positive_edge_intervals_without_corner_neighbors_and_stale_routes_fail_closed()
     {
        let (mut state, keys) = fixture(6);
        for (key, point) in keys.iter().zip([
            Point { x: 0, y: 0 },
            Point { x: -100, y: 0 },
            Point { x: 100, y: 0 },
            Point { x: 0, y: -100 },
            Point { x: 0, y: 100 },
            Point { x: 100, y: 100 },
        ]) {
            state.layout.positions.insert(key.clone(), point);
        }
        let routes = routes_for(&state, &keys[0]);
        assert_eq!(
            routes
                .boundaries
                .iter()
                .map(|boundary| boundary.side)
                .collect::<BTreeSet<_>>()
                .len(),
            4
        );
        assert!(
            !scene(&state, &state.layout)
                .contacts
                .iter()
                .any(|contact| contact.from == id(&keys[0], "screen-0")
                    && contact.to == id(&keys[5], "screen-5"))
        );
        let event = crossing(&state, &keys[0], Side::Right, 32767);
        assert!(resolve_crossing(&state, &keys[1], event).is_err());
        state
            .members
            .get_mut(&keys[2])
            .unwrap()
            .node
            .devices
            .displays[0]
            .native
            .as_mut()
            .unwrap()
            .epoch += 1;
        assert!(resolve_crossing(&state, &keys[0], event).unwrap().is_none());
        state.members.get_mut(&keys[2]).unwrap().online = false;
        assert!(
            !routes_for(&state, &keys[0])
                .boundaries
                .iter()
                .any(|boundary| boundary.side == Side::Right)
        );
    }

    fn shared_fixture() -> (Workspace, Vec<String>) {
        let (mut state, keys) = fixture(3);
        for (index, key) in keys[..2].iter().enumerate() {
            let member = state.members.get_mut(key).unwrap();
            member.node.monitor.enabled = true;
            member.node.monitor.id = format!("screen-{index}");
            member.node.monitor.input = Some(15 + 2 * index as u16);
        }
        state
            .members
            .get_mut(&keys[0])
            .unwrap()
            .node
            .devices
            .displays
            .push(display(
                "private-0",
                ScreenRect {
                    x: 100,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                10,
            ));
        state
            .members
            .get_mut(&keys[1])
            .unwrap()
            .node
            .devices
            .displays
            .push(display(
                "private-1",
                ScreenRect {
                    x: -100,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                11,
            ));
        state.confirmed_monitor = Some(keys[1].clone());
        state.active_monitor = Some(keys[1].clone());
        state.layout = Layout {
            revision: 1,
            positions: BTreeMap::from([
                (SHARED.into(), Point::default()),
                (keys[2].clone(), Point { x: 0, y: -100 }),
            ]),
            shared_size: Some(Size {
                width: 100,
                height: 100,
            }),
        };
        (state, keys)
    }

    #[test]
    fn one_shared_tile_uses_confirmed_owner_and_regular_participant_entry_preserves_physical_selection()
     {
        let (mut state, keys) = shared_fixture();
        let scene = scene(&state, &state.layout);
        assert_eq!(
            scene
                .surfaces
                .iter()
                .filter(|surface| surface.shared)
                .count(),
            1
        );
        assert!(scene.problems.is_empty());
        assert!(
            !routes_for(&state, &keys[1])
                .boundaries
                .iter()
                .any(|boundary| boundary.side == Side::Left),
            "Same-PC ordinary edges remain native"
        );
        let event = crossing(&state, &keys[0], Side::Left, 32767);
        let activation = resolve_crossing(&state, &keys[0], event).unwrap().unwrap();
        assert_eq!(
            activation.target,
            DisplayTarget::Shared {
                computer: keys[1].clone()
            }
        );
        state
            .activate_display(&keys[0], activation.target, Some(activation.entry))
            .unwrap();
        assert!(state.monitor_command.is_none());
        let event = crossing(&state, &keys[1], Side::Right, 32767);
        let activation = resolve_crossing(&state, &keys[0], event).unwrap().unwrap();
        assert_eq!(
            activation.target,
            DisplayTarget::Ordinary {
                computer: keys[0].clone(),
                display: "private-0".into()
            }
        );
        state
            .activate_display(&keys[0], activation.target, Some(activation.entry))
            .unwrap();
        assert!(state.monitor_command.is_none());
        assert_eq!(state.confirmed_monitor.as_ref(), Some(&keys[1]));
        state.members.get_mut(&keys[1]).unwrap().online = false;
        assert!(
            routes_for(&state, &keys[0])
                .boundaries
                .iter()
                .any(|boundary| boundary.id == 0 && boundary.side == Side::Left)
        );
    }

    #[test]
    fn configured_shared_membership_survives_an_inactive_binding_disappearing_without_reflow() {
        let (mut state, keys) = shared_fixture();
        state
            .members
            .get_mut(&keys[0])
            .unwrap()
            .node
            .devices
            .displays
            .retain(|display| display.id != "screen-0");
        assert!(configured_shared(&state, &keys[0]));
        assert!(shared_binding(&state, &keys[0]).is_none());
        let changed = scene(&state, &state.layout);
        assert_eq!(
            changed
                .surfaces
                .iter()
                .filter(|surface| surface.shared)
                .count(),
            1
        );
        let private = changed
            .surfaces
            .iter()
            .find(|surface| surface.label.contains("private-0"))
            .unwrap();
        assert_eq!(private.component, SHARED);
        assert!(private.rect.is_none() && !private.available);
        assert_eq!(draft(&state).positions, state.layout.positions);
    }

    #[test]
    fn drafts_prune_old_components_unknown_geometry_does_not_block_known_peers_and_settings_preserve_layout()
     {
        let (mut state, keys) = fixture(3);
        state
            .members
            .get_mut(&keys[2])
            .unwrap()
            .node
            .devices
            .displays[0]
            .native = None;
        validate(&state, &state.layout).unwrap();
        let saved = state.layout.clone();
        let mut settings = state.settings.clone();
        settings.audio_enabled = false;
        state.set_settings(state.revision, settings).unwrap();
        assert_eq!(state.layout, saved);
        state
            .members
            .get_mut(&keys[0])
            .unwrap()
            .node
            .monitor
            .enabled = true;
        state.members.get_mut(&keys[0]).unwrap().node.monitor.id = "screen-0".into();
        state.members.get_mut(&keys[0]).unwrap().node.monitor.input = None;
        assert!(
            !configured_shared(&state, &keys[0]),
            "Auto-selected monitor metadata is not a shared declaration"
        );
        state.members.get_mut(&keys[0]).unwrap().node.monitor.input = Some(15);
        let changed = draft(&state);
        assert!(
            changed.positions.contains_key(SHARED) && !changed.positions.contains_key(&keys[0])
        );
        validate(&state, &changed).unwrap();
        state.set_layout(state.layout.revision, changed).unwrap();
        assert!(state.monitor_command.is_none());
    }

    #[test]
    fn offline_active_display_returns_input_without_leaving_inconsistent_focus_fields() {
        let (mut state, keys) = fixture(2);
        state
            .activate_display(
                &keys[0],
                DisplayTarget::Ordinary {
                    computer: keys[1].clone(),
                    display: "screen-1".into(),
                },
                None,
            )
            .unwrap();
        state.members.get_mut(&keys[1]).unwrap().online = false;
        state.reconcile_input();
        assert_eq!(state.active_input.as_ref(), Some(&keys[0]));
        assert!(
            state.active_display.is_none()
                && state.display_entry.is_none()
                && state.active_input_entry.is_none()
        );
        state.validate().unwrap();
    }

    #[test]
    fn component_moves_keep_native_offsets_and_overlap_disables_only_affected_routes() {
        let (mut state, keys) = fixture(5);
        state
            .members
            .get_mut(&keys[0])
            .unwrap()
            .node
            .devices
            .displays
            .push(display(
                "second",
                ScreenRect {
                    x: -100,
                    y: 0,
                    width: 100,
                    height: 100,
                },
                10,
            ));
        let before = scene(&state, &state.layout);
        let moved = move_component(&state, &state.layout, &keys[0], Point { x: 50, y: 300 }, 0);
        let after = scene(&state, &moved);
        let original = before
            .surfaces
            .iter()
            .filter(|surface| surface.computer.as_ref() == Some(&keys[0]))
            .filter_map(|surface| surface.rect)
            .collect::<Vec<_>>();
        let translated = after
            .surfaces
            .iter()
            .filter(|surface| surface.computer.as_ref() == Some(&keys[0]))
            .filter_map(|surface| surface.rect)
            .collect::<Vec<_>>();
        assert_eq!(
            original[0].x - original[1].x,
            translated[0].x - translated[1].x
        );
        state
            .layout
            .positions
            .insert(keys[2].clone(), Point { x: 100, y: 0 });
        state
            .layout
            .positions
            .insert(keys[3].clone(), Point { x: 400, y: 0 });
        state
            .layout
            .positions
            .insert(keys[4].clone(), Point { x: 500, y: 0 });
        assert!(validate(&state, &state.layout).is_err());
        assert!(routes_for(&state, &keys[0]).boundaries.is_empty());
        assert!(!routes_for(&state, &keys[3]).boundaries.is_empty());
        state.validate().unwrap();
    }

    #[test]
    fn routing_cache_ignores_unrelated_updates_and_activation_only_refreshes_the_header() {
        let (mut state, keys) = fixture(2);
        let mut cache = RoutingCache::default();
        let original = cache.routes_for(&state, &keys[0]);
        assert_eq!(cache.builds, 1);
        state.revision += 1;
        state
            .members
            .get_mut(&keys[0])
            .unwrap()
            .preferences
            .audio
            .volume = 37;
        state
            .members
            .get_mut(&keys[0])
            .unwrap()
            .node
            .diagnostics
            .line("Audio", "Continuity updated");
        assert_eq!(cache.routes_for(&state, &keys[0]), original);
        state.activation += 1;
        let changed = cache.routes_for(&state, &keys[0]);
        assert_eq!(cache.builds, 1);
        assert_eq!(changed.activation, state.activation);
        assert_eq!(changed.boundaries, original.boundaries);
        state
            .members
            .get_mut(&keys[1])
            .unwrap()
            .node
            .devices
            .displays[0]
            .native
            .as_mut()
            .unwrap()
            .epoch += 1;
        cache.routes_for(&state, &keys[0]);
        assert_eq!(cache.builds, 2);
        state.settings.edge = false;
        assert!(cache.routes_for(&state, &keys[0]).boundaries.is_empty());
        assert_eq!(cache.builds, 2);
    }

    #[test]
    fn fresh_wide_display_components_do_not_overlap_and_existing_origins_stay_put() {
        let (mut state, keys) = fixture(3);
        state.layout = Layout::default();
        for member in state.members.values_mut() {
            let native = member.node.devices.displays[0].native.as_mut().unwrap();
            native.rect.width = 3840;
            native.rect.height = 2160;
        }
        let proposed = draft(&state);
        assert!(scene(&state, &proposed).problems.is_empty());
        validate(&state, &proposed).unwrap();
        state.layout = proposed.clone();
        state
            .layout
            .positions
            .insert(keys[0].clone(), Point { x: -4000, y: 300 });
        assert_eq!(
            draft(&state).positions[&keys[0]],
            Point { x: -4000, y: 300 }
        );
    }

    #[test]
    fn fractional_component_scale_preserves_touching_edges_without_one_pixel_overlap() {
        let anchor = ScreenRect {
            x: 0,
            y: 0,
            width: 200,
            height: 200,
        };
        let size = Some(Size {
            width: 100,
            height: 100,
        });
        let first = at(
            ScreenRect {
                x: 201,
                y: 0,
                width: 101,
                height: 100,
            },
            anchor,
            Point::default(),
            size,
        )
        .unwrap();
        let second = at(
            ScreenRect {
                x: 302,
                y: 0,
                width: 101,
                height: 100,
            },
            anchor,
            Point::default(),
            size,
        )
        .unwrap();
        assert_eq!(right(first), i64::from(second.x));
    }

    #[test]
    fn a_partial_neighbor_ending_at_the_last_source_pixel_excludes_that_pixel() {
        let (mut state, keys) = fixture(2);
        state
            .members
            .get_mut(&keys[1])
            .unwrap()
            .node
            .devices
            .displays[0]
            .native
            .as_mut()
            .unwrap()
            .rect
            .height = 99;
        let routes = routes_for(&state, &keys[0]);
        let boundary = routes
            .boundaries
            .iter()
            .find(|boundary| boundary.side == Side::Right)
            .unwrap();
        assert!(!boundary.contains(u16::MAX));
        assert!(boundary.contains(u16::MAX / 2));
        let a = ScreenRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let b = ScreenRect {
            x: 100,
            y: 0,
            width: 100,
            height: 99,
        };
        assert_eq!(native_segment(a, b, Side::Right).unwrap().1, u16::MAX - 1);
    }

    #[test]
    fn known_component_far_edges_must_fit_while_unavailable_geometry_stays_permissive() {
        let (mut state, keys) = fixture(3);
        state
            .members
            .get_mut(&keys[2])
            .unwrap()
            .node
            .devices
            .displays[0]
            .native = None;
        state
            .layout
            .positions
            .insert(keys[2].clone(), Point { x: LIMIT, y: 0 });
        validate(&state, &state.layout).unwrap();
        state.layout.positions.insert(
            keys[1].clone(),
            Point {
                x: LIMIT - 10,
                y: 0,
            },
        );
        assert!(
            validate(&state, &state.layout)
                .unwrap_err()
                .to_string()
                .ends_with("out of range")
        );
    }
}
