use anyhow::{Result, bail};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
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
}
