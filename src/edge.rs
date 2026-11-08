use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
impl ScreenRect {
    pub fn validate(self) -> Result<()> {
        ensure!(
            self.width >= 2 && self.height >= 2,
            "Invalid screen dimensions"
        );
        ensure!(
            i64::from(self.x) + i64::from(self.width) - 1 <= i64::from(i32::MAX)
                && i64::from(self.y) + i64::from(self.height) - 1 <= i64::from(i32::MAX),
            "Screen geometry is out of range"
        );
        Ok(())
    }
    pub fn rect(self) -> Rect {
        Rect {
            x: f64::from(self.x),
            y: f64::from(self.y),
            width: f64::from(self.width),
            height: f64::from(self.height),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeScreen {
    pub native_id: String,
    pub rect: ScreenRect,
    pub epoch: u64,
}
impl NativeScreen {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.native_id.is_empty()
                && self.native_id.len() <= 1024
                && !self.native_id.chars().any(char::is_control)
                && self.epoch != 0,
            "Invalid native screen identity"
        );
        self.rect.validate()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub screen: ScreenRect,
    pub geometry: u64,
    pub side: Side,
    pub position: u16,
}
impl Entry {
    pub fn validate(self) -> Result<()> {
        self.screen.validate()?;
        ensure!(self.geometry != 0, "Invalid screen geometry generation");
        Ok(())
    }
    pub fn point(self) -> (f64, f64) {
        self.side.entry(self.screen.rect(), self.position)
    }
    pub fn resolve(self, screens: &[NativeScreen]) -> Result<&NativeScreen> {
        self.validate()?;
        let mut found = screens
            .iter()
            .filter(|screen| screen.rect == self.screen && screen.epoch == self.geometry);
        let screen = found
            .next()
            .ok_or_else(|| anyhow::anyhow!("The destination screen changed or is unavailable"))?;
        ensure!(
            found.next().is_none(),
            "The destination screen is ambiguous"
        );
        Ok(screen)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
    /// Zero reserves a blocked native edge: it never emits a crossing request.
    pub id: u64,
    pub screen: ScreenRect,
    pub geometry: u64,
    pub side: Side,
    /// Half-open normalized interval; u16::MAX includes the last coordinate.
    pub start: u16,
    pub end: u16,
}
impl Boundary {
    pub fn validate(self) -> Result<()> {
        self.screen.validate()?;
        ensure!(
            self.geometry != 0 && self.start < self.end,
            "Invalid edge boundary"
        );
        Ok(())
    }
    pub fn contains(self, position: u16) -> bool {
        position >= self.start
            && (position < self.end || self.end == u16::MAX && position == u16::MAX)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Crossing {
    pub revision: u64,
    pub activation: u64,
    pub boundary: u64,
    pub position: u16,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Routes {
    pub revision: u64,
    pub activation: u64,
    pub boundaries: Vec<Boundary>,
    #[serde(default)]
    pub policy: Policy,
}
impl Routes {
    pub fn validate(&self) -> Result<()> {
        self.policy.validate()?;
        ensure!(self.boundaries.len() <= 4096, "Too many edge boundaries");
        ensure!(
            self.boundaries.is_empty() || self.revision != 0,
            "Invalid edge route generation"
        );
        let mut previous: BTreeMap<(ScreenRect, u64, Side), Vec<Boundary>> = BTreeMap::new();
        let mut ids = std::collections::BTreeSet::new();
        for boundary in &self.boundaries {
            boundary.validate()?;
            ensure!(
                boundary.id == 0 || ids.insert(boundary.id),
                "Duplicate edge boundary identity"
            );
            previous
                .entry((boundary.screen, boundary.geometry, boundary.side))
                .or_default()
                .push(*boundary);
        }
        for boundaries in previous.values_mut() {
            boundaries.sort_by_key(|boundary| boundary.start);
            ensure!(
                !boundaries
                    .windows(2)
                    .any(|pair| pair[0].end > pair[1].start),
                "Overlapping edge boundaries"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub dwell_ms: u16,
    pub protect_corners: bool,
    pub paused: bool,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            dwell_ms: 150,
            protect_corners: true,
            paused: false,
        }
    }
}
impl Policy {
    pub fn validate(self) -> Result<()> {
        ensure!(self.dwell_ms <= 1000, "Invalid screen edge delay");
        Ok(())
    }
    fn protects(self, boundary: Boundary, position: u16) -> bool {
        if !self.protect_corners {
            return false;
        }
        let rect = boundary.screen.rect();
        let span = if boundary.side.vertical() {
            rect.height - 1.0
        } else {
            rect.width - 1.0
        };
        // Both the edge offset and this square are in the screen's native
        // coordinate space. No physical-pixel assumption is made on a scaled
        // Windows display or a compositor using logical coordinates.
        let corner = rect.width.min(rect.height) * 0.025;
        let offset = f64::from(position) / f64::from(u16::MAX) * span;
        offset < corner || span - offset < corner
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hit {
    Cross(Crossing),
    Block,
}

#[derive(Default)]
pub struct Router {
    routes: Routes,
    screens: BTreeMap<ScreenRect, Vec<Boundary>>,
    latch: Option<(ScreenRect, Side)>,
    cooldown: Option<Instant>,
    dwell: Option<Dwell>,
}
struct Dwell {
    boundary: Boundary,
    since: Instant,
    last: Instant,
}
impl Router {
    pub fn configure(&mut self, routes: Routes, screens: &[NativeScreen]) -> Result<()> {
        routes.validate()?;
        let mut indexed: BTreeMap<ScreenRect, Vec<Boundary>> = BTreeMap::new();
        for boundary in &routes.boundaries {
            let entry = Entry {
                screen: boundary.screen,
                geometry: boundary.geometry,
                side: boundary.side,
                position: 0,
            };
            if entry.resolve(screens).is_ok() {
                indexed.entry(boundary.screen).or_default().push(*boundary);
            }
        }
        if self.routes != routes || self.screens != indexed {
            self.latch = None;
            self.cancel_dwell();
        }
        self.screens = indexed;
        self.routes = routes;
        Ok(())
    }
    #[cfg(test)]
    pub fn enabled(&self) -> bool {
        !self.screens.is_empty()
    }
    pub fn routes(&self) -> &Routes {
        &self.routes
    }
    pub fn reset_entry(&mut self) {
        self.latch = None;
        self.cancel_dwell();
        self.cooldown = Some(Instant::now() + Duration::from_millis(120));
    }
    pub fn cancel_dwell(&mut self) {
        self.dwell = None;
    }
    /// A cheap, non-mutating probe for adapters that observe held OS inputs
    /// only when the pointer reaches a configured edge.
    #[cfg(any(windows, test))]
    pub fn would_cross(&self, x: f64, y: f64, dx: f64, dy: f64) -> bool {
        self.screens
            .iter()
            .find(|(screen, _)| screen.rect().contains(x, y))
            .is_some_and(|(_, boundaries)| {
                boundaries.iter().any(|boundary| {
                    boundary
                        .side
                        .crossing(boundary.screen.rect(), x, y, dx, dy)
                        .is_some_and(|position| boundary.contains(position))
                })
            })
    }
    #[cfg(test)]
    pub fn crossing(&mut self, x: f64, y: f64, dx: f64, dy: f64) -> Option<Hit> {
        self.crossing_if(x, y, dx, dy, true)
    }
    pub fn crossing_if(&mut self, x: f64, y: f64, dx: f64, dy: f64, allow: bool) -> Option<Hit> {
        self.crossing_at(x, y, dx, dy, allow, Instant::now())
    }
    fn crossing_at(
        &mut self,
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
        allow: bool,
        now: Instant,
    ) -> Option<Hit> {
        let Some((screen, candidates)) = self
            .screens
            .iter()
            .find(|(screen, _)| screen.rect().contains(x, y))
        else {
            self.cancel_dwell();
            return None;
        };
        if let Some((previous, side)) = self.latch {
            let rect = previous.rect();
            let distance = match side {
                Side::Left => x - rect.x,
                Side::Right => rect.right() - 1.0 - x,
                Side::Top => y - rect.y,
                Side::Bottom => rect.bottom() - 1.0 - y,
            };
            let inset = 16.0_f64.min(
                if side.vertical() {
                    rect.width
                } else {
                    rect.height
                } / 3.0,
            );
            if previous != *screen || distance >= inset {
                self.latch = None;
            }
        }
        for boundary in candidates {
            let Some(position) = boundary.side.crossing(boundary.screen.rect(), x, y, dx, dy)
            else {
                continue;
            };
            if !boundary.contains(position) {
                continue;
            }
            if boundary.id == 0
                || !allow
                || self.routes.policy.paused
                || self.routes.policy.protects(*boundary, position)
                || self.latch.is_some()
                || self.cooldown.is_some_and(|until| now < until)
            {
                self.dwell = None;
                return Some(Hit::Block);
            }
            if self.routes.policy.dwell_ms > 0 {
                // Dwell requires continued outward motion. Parking at an edge
                // does not arm a timer or count idle time toward a later push.
                let gap = Duration::from_millis(100);
                let dwell = self.dwell.get_or_insert(Dwell {
                    boundary: *boundary,
                    since: now,
                    last: now,
                });
                if dwell.boundary != *boundary || now.saturating_duration_since(dwell.last) > gap {
                    *dwell = Dwell {
                        boundary: *boundary,
                        since: now,
                        last: now,
                    };
                }
                dwell.last = now;
                if now.saturating_duration_since(dwell.since)
                    < Duration::from_millis(u64::from(self.routes.policy.dwell_ms))
                {
                    return Some(Hit::Block);
                }
            }
            self.dwell = None;
            self.latch = Some((*screen, boundary.side));
            return Some(Hit::Cross(Crossing {
                revision: self.routes.revision,
                activation: self.routes.activation,
                boundary: boundary.id,
                position,
            }));
        }
        self.cancel_dwell();
        None
    }
}

impl Side {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "top" => Ok(Self::Top),
            "bottom" => Ok(Self::Bottom),
            _ => bail!("edge side must be left, right, top, or bottom"),
        }
    }

    fn vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    pub fn crossing(self, rect: Rect, x: f64, y: f64, dx: f64, dy: f64) -> Option<u16> {
        if !rect.contains(x, y) {
            return None;
        }
        let t = match self {
            Self::Left if dx < 0.0 => (rect.x - x) / dx,
            Self::Right if dx > 0.0 => (rect.right() - 1.0 - x) / dx,
            Self::Top if dy < 0.0 => (rect.y - y) / dy,
            Self::Bottom if dy > 0.0 => (rect.bottom() - 1.0 - y) / dy,
            _ => return None,
        };
        if t > 1.0 {
            return None;
        }
        let t = t.max(0.0);
        let (offset, span) = if self.vertical() {
            (y + t * dy - rect.y, rect.height)
        } else {
            (x + t * dx - rect.x, rect.width)
        };
        if !(0.0..span).contains(&offset) {
            return None; // The movement left through a different side first.
        }
        Some(((offset / (span - 1.0).max(1.0)).min(1.0) * f64::from(u16::MAX)).round() as u16)
    }

    pub fn entry(self, r: Rect, fraction: u16) -> (f64, f64) {
        let f = f64::from(fraction) / f64::from(u16::MAX);
        let (x, y) = (r.x + f * (r.width - 1.0), r.y + f * (r.height - 1.0));
        let inset = 16.0_f64.min(if self.vertical() {
            r.width / 3.0
        } else {
            r.height / 3.0
        });
        match self {
            Self::Left => (r.x + inset, y),
            Self::Right => (r.right() - 1.0 - inset, y),
            Self::Top => (x, r.y + inset),
            Self::Bottom => (x, r.bottom() - 1.0 - inset),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn right(self) -> f64 {
        self.x + self.width
    }
    pub fn bottom(self) -> f64 {
        self.y + self.height
    }

    pub fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    #[cfg(target_os = "linux")]
    pub fn clamp(self, x: f64, y: f64) -> (f64, f64) {
        (
            x.clamp(self.x, self.right() - 1.0),
            y.clamp(self.y, self.bottom() - 1.0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native() -> NativeScreen {
        NativeScreen {
            native_id: "screen".into(),
            rect: ScreenRect {
                x: -100,
                y: 20,
                width: 100,
                height: 100,
            },
            epoch: 7,
        }
    }
    fn boundary(id: u64, side: Side, start: u16, end: u16) -> Boundary {
        Boundary {
            id,
            screen: native().rect,
            geometry: 7,
            side,
            start,
            end,
        }
    }
    fn routed(boundaries: Vec<Boundary>) -> Router {
        routed_policy(
            boundaries,
            Policy {
                dwell_ms: 0,
                protect_corners: false,
                ..Default::default()
            },
        )
    }
    fn routed_policy(boundaries: Vec<Boundary>, policy: Policy) -> Router {
        let mut router = Router::default();
        router
            .configure(
                Routes {
                    revision: 3,
                    activation: 5,
                    boundaries,
                    policy,
                },
                &[native()],
            )
            .unwrap();
        router
    }

    #[test]
    fn routes_try_every_side_and_only_the_contact_interval() {
        let mut router = routed(vec![
            boundary(1, Side::Left, 0, u16::MAX),
            boundary(2, Side::Right, 32768, u16::MAX),
            boundary(3, Side::Top, 0, u16::MAX),
        ]);
        assert!(router.would_cross(-2.0, 100.0, 5.0, 0.0));
        assert!(!router.would_cross(-2.0, 30.0, 5.0, 0.0));
        assert_eq!(router.crossing(-2.0, 30.0, 5.0, 0.0), None);
        assert!(matches!(
            router.crossing(-50.0, 21.0, 0.0, -5.0),
            Some(Hit::Cross(Crossing {
                boundary: 3,
                revision: 3,
                activation: 5,
                ..
            }))
        ));
        let mut router = routed(vec![
            boundary(1, Side::Left, 0, u16::MAX),
            boundary(2, Side::Right, 32768, u16::MAX),
        ]);
        assert!(matches!(
            router.crossing(-2.0, 100.0, 5.0, 0.0),
            Some(Hit::Cross(Crossing { boundary: 2, .. }))
        ));
    }

    #[test]
    fn held_inputs_block_without_consuming_a_future_crossing_and_latch_requires_inward_motion() {
        let mut router = routed(vec![boundary(1, Side::Right, 0, u16::MAX)]);
        assert_eq!(
            router.crossing_if(-2.0, 60.0, 5.0, 0.0, false),
            Some(Hit::Block)
        );
        assert!(matches!(
            router.crossing(-2.0, 60.0, 5.0, 0.0),
            Some(Hit::Cross(_))
        ));
        assert_eq!(router.crossing(-2.0, 60.0, 5.0, 0.0), Some(Hit::Block));
        assert_eq!(router.crossing(-30.0, 60.0, -5.0, 0.0), None);
        assert!(matches!(
            router.crossing(-2.0, 60.0, 5.0, 0.0),
            Some(Hit::Cross(_))
        ));
    }

    #[test]
    fn stale_or_ambiguous_geometry_disables_edges_and_blocks_never_emit_requests() {
        let routes = Routes {
            revision: 1,
            activation: 1,
            boundaries: vec![boundary(0, Side::Right, 0, u16::MAX)],
            ..Default::default()
        };
        let mut router = Router::default();
        router.configure(routes.clone(), &[native()]).unwrap();
        assert_eq!(router.crossing(-2.0, 60.0, 5.0, 0.0), Some(Hit::Block));
        let mut stale = native();
        stale.epoch += 1;
        router.configure(routes.clone(), &[stale]).unwrap();
        assert!(!router.enabled());
        router.configure(routes, &[native(), native()]).unwrap();
        assert!(!router.enabled());
        let entry = Entry {
            screen: native().rect,
            geometry: 7,
            side: Side::Bottom,
            position: u16::MAX / 2,
        };
        assert!(entry.resolve(&[native(), native()]).is_err());
    }

    #[test]
    fn selected_edge_uses_normalized_offset_across_scaled_and_offset_monitors() {
        let win = Rect {
            x: -1920.0,
            y: 100.0,
            width: 1920.0,
            height: 1080.0,
        };
        let linux = Rect {
            x: 2560.0,
            y: 0.0,
            width: 1280.0,
            height: 720.0,
        };
        let (x, y) = (-2.0, 640.0);
        let position = Side::Right.crossing(win, x, y, 6.0, 0.0).unwrap();
        assert!(Side::Left.crossing(win, x, y, 6.0, 0.0).is_none());
        assert!(Side::Right.crossing(win, 10.0, y, 6.0, 0.0).is_none());
        assert!(
            Side::Right
                .crossing(win, -30.0, 105.0, 35.0, -35.0)
                .is_none()
        );
        let (x, y) = Side::Left.entry(linux, position);
        assert_eq!(x, 2576.0);
        assert!((y - 359.8).abs() < 0.1);
        assert_eq!(Side::Left.crossing(linux, x, y, -20.0, 0.0), Some(position));
    }

    #[test]
    fn dwell_requires_continued_pressure_and_pullback_or_idle_time_cancels_it() {
        let mut router = routed_policy(
            vec![boundary(1, Side::Right, 0, u16::MAX)],
            Policy::default(),
        );
        let now = Instant::now();
        let push = |router: &mut Router, millis| {
            router.crossing_at(
                -2.0,
                60.0,
                5.0,
                0.0,
                true,
                now + Duration::from_millis(millis),
            )
        };
        assert_eq!(push(&mut router, 0), Some(Hit::Block));
        assert_eq!(push(&mut router, 80), Some(Hit::Block));
        assert_eq!(
            router.crossing_at(-2.0, 60.0, -5.0, 0.0, true, now + Duration::from_millis(90)),
            None
        );
        assert_eq!(push(&mut router, 150), Some(Hit::Block));
        assert_eq!(push(&mut router, 230), Some(Hit::Block));
        assert!(matches!(push(&mut router, 300), Some(Hit::Cross(_))));

        let mut router = routed_policy(
            vec![boundary(1, Side::Right, 0, u16::MAX)],
            Policy::default(),
        );
        assert_eq!(push(&mut router, 0), Some(Hit::Block));
        assert_eq!(push(&mut router, 2000), Some(Hit::Block));
        assert_eq!(push(&mut router, 2080), Some(Hit::Block));
        assert!(matches!(push(&mut router, 2150), Some(Hit::Cross(_))));
    }

    #[test]
    fn held_inputs_and_pausing_require_a_fresh_dwell_after_release_or_unlock() {
        let mut router = routed_policy(
            vec![boundary(1, Side::Right, 0, u16::MAX)],
            Policy::default(),
        );
        let now = Instant::now();
        let push = |router: &mut Router, millis, allow| {
            router.crossing_at(
                -2.0,
                60.0,
                5.0,
                0.0,
                allow,
                now + Duration::from_millis(millis),
            )
        };
        assert_eq!(push(&mut router, 0, true), Some(Hit::Block));
        assert_eq!(push(&mut router, 80, false), Some(Hit::Block));
        assert_eq!(push(&mut router, 150, true), Some(Hit::Block));
        router.cancel_dwell(); // A complete key press between pointer events.
        assert_eq!(push(&mut router, 230, true), Some(Hit::Block));
        let mut locked = router.routes().clone();
        locked.revision += 1;
        locked.policy.paused = true;
        router.configure(locked.clone(), &[native()]).unwrap();
        assert_eq!(push(&mut router, 300, true), Some(Hit::Block));
        locked.revision += 1;
        locked.policy.paused = false;
        router.configure(locked, &[native()]).unwrap();
        assert_eq!(push(&mut router, 380, true), Some(Hit::Block));
        assert_eq!(push(&mut router, 460, true), Some(Hit::Block));
        assert!(matches!(push(&mut router, 530, true), Some(Hit::Cross(_))));
    }

    #[test]
    fn moving_to_a_different_contact_or_generation_does_not_reuse_dwell() {
        let mut router = routed_policy(
            vec![
                boundary(1, Side::Right, 0, 32768),
                boundary(2, Side::Right, 32768, u16::MAX),
                boundary(3, Side::Top, 0, u16::MAX),
            ],
            Policy::default(),
        );
        let now = Instant::now();
        assert_eq!(
            router.crossing_at(-2., 40., 5., 0., true, now),
            Some(Hit::Block)
        );
        assert_eq!(
            router.crossing_at(-2., 100., 5., 0., true, now + Duration::from_millis(80)),
            Some(Hit::Block)
        );
        assert_eq!(
            router.crossing_at(-50., 21., 0., -5., true, now + Duration::from_millis(150)),
            Some(Hit::Block)
        );
        let mut changed = router.routes().clone();
        changed.activation += 1;
        router.configure(changed, &[native()]).unwrap();
        assert_eq!(
            router.crossing_at(-50., 21., 0., -5., true, now + Duration::from_millis(230)),
            Some(Hit::Block)
        );
        assert_eq!(
            router.crossing_at(-50., 21., 0., -5., true, now + Duration::from_millis(310)),
            Some(Hit::Block)
        );
        assert!(matches!(
            router.crossing_at(-50., 21., 0., -5., true, now + Duration::from_millis(380)),
            Some(Hit::Cross(Crossing {
                boundary: 3,
                activation: 6,
                ..
            }))
        ));
    }

    #[test]
    fn protected_corners_scale_with_the_screen_and_not_with_contact_endpoints() {
        let policy = Policy {
            dwell_ms: 0,
            ..Default::default()
        };
        for scale in [1, 2] {
            let screen = NativeScreen {
                native_id: "scaled".into(),
                rect: ScreenRect {
                    x: -1000 * scale,
                    y: 20 * scale,
                    width: 1000 * scale as u32,
                    height: 800 * scale as u32,
                },
                epoch: 7,
            };
            for side in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
                let rect = screen.rect.rect();
                let edge = Boundary {
                    id: 1,
                    screen: screen.rect,
                    geometry: 7,
                    side,
                    start: 0,
                    end: u16::MAX,
                };
                let mut router = Router::default();
                router
                    .configure(
                        Routes {
                            revision: 1,
                            activation: 1,
                            boundaries: vec![edge],
                            policy,
                        },
                        std::slice::from_ref(&screen),
                    )
                    .unwrap();
                let (x, y, dx, dy) = match side {
                    Side::Left => (rect.x, rect.y + 10. * f64::from(scale), -5., 0.),
                    Side::Right => (rect.right() - 1., rect.y + 10. * f64::from(scale), 5., 0.),
                    Side::Top => (rect.x + 10. * f64::from(scale), rect.y, 0., -5.),
                    Side::Bottom => (rect.x + 10. * f64::from(scale), rect.bottom() - 1., 0., 5.),
                };
                assert_eq!(router.crossing(x, y, dx, dy), Some(Hit::Block));
                let (x, y) = if side.vertical() {
                    (x, rect.y + 40. * f64::from(scale))
                } else {
                    (rect.x + 40. * f64::from(scale), y)
                };
                assert!(matches!(router.crossing(x, y, dx, dy), Some(Hit::Cross(_))));
            }
        }
        let mut router = routed_policy(vec![boundary(1, Side::Right, 32768, u16::MAX)], policy);
        assert!(matches!(
            router.crossing(-2., 70., 5., 0.),
            Some(Hit::Cross(_))
        ));
    }

    #[test]
    fn policy_defaults_are_protective_and_invalid_delays_are_rejected() {
        let routes: Routes =
            serde_json::from_str(r#"{"revision":1,"activation":1,"boundaries":[]}"#).unwrap();
        assert_eq!(routes.policy, Policy::default());
        let mut invalid = routes;
        invalid.policy.dwell_ms = 1001;
        assert!(invalid.validate().is_err());
    }
}
