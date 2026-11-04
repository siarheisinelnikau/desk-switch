use anyhow::{Result, bail};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side { Left, Right, Top, Bottom }

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

    pub fn crossed(self, r: Rect, x: f64, y: f64, dx: f64, dy: f64) -> bool {
        let (x1, y1) = (x + dx, y + dy);
        r.contains(x, y) && match self {
            Self::Left => dx < 0.0 && x1 <= r.x,
            Self::Right => dx > 0.0 && x1 >= r.right() - 1.0,
            Self::Top => dy < 0.0 && y1 <= r.y,
            Self::Bottom => dy > 0.0 && y1 >= r.bottom() - 1.0,
        }
    }

    pub fn fraction(self, rect: Rect, x: f64, y: f64) -> u16 {
        let fraction = if self.vertical() { (y - rect.y) / rect.height }
            else { (x - rect.x) / rect.width };
        (fraction.clamp(0.0, 1.0) * f64::from(u16::MAX)).round() as u16
    }

    pub fn entry(self, r: Rect, fraction: u16) -> (f64, f64) {
        let f = f64::from(fraction) / f64::from(u16::MAX);
        let (x, y) = (r.x + f * (r.width - 1.0), r.y + f * (r.height - 1.0));
        let inset = 16.0_f64.min(if self.vertical() { r.width / 3.0 } else { r.height / 3.0 });
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
    pub fn right(self) -> f64 { self.x + self.width }
    pub fn bottom(self) -> f64 { self.y + self.height }

    pub fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn clamp(self, x: f64, y: f64) -> (f64, f64) {
        (x.clamp(self.x, self.right() - 1.0), y.clamp(self.y, self.bottom() - 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_edge_uses_normalized_offset_across_scaled_and_offset_monitors() {
        let win = Rect { x: -1920.0, y: 100.0, width: 1920.0, height: 1080.0 };
        let linux = Rect { x: 2560.0, y: 0.0, width: 1280.0, height: 720.0 };
        let (x, y) = (-2.0, 640.0);
        assert!(Side::Right.crossed(win, x, y, 6.0, 0.0));
        assert!(!Side::Left.crossed(win, x, y, 6.0, 0.0));
        assert!(!Side::Right.crossed(win, 10.0, y, 6.0, 0.0));
        let (x, y) = Side::Left.entry(linux, Side::Right.fraction(win, x, y));
        assert_eq!(x, 2576.0);
        assert!((y - 359.5).abs() < 0.1);
        assert!(Side::Left.crossed(linux, x, y, -20.0, 0.0));
    }
}
