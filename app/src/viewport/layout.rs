//! Quad viewport layout with splitters and a maximise toggle (spec 2.2, golden 14).

use eframe::egui::{pos2, vec2, Rect};

/// Tile index order: Top, Front, Left, Perspective (row-major).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuadLayout {
    /// Vertical and horizontal splitter positions as fractions 0.1..0.9.
    pub split: [f32; 2],
    /// Gap between tiles in points.
    pub gutter: f32,
    pub maximized: bool,
    /// The active tile.
    pub active: usize,
}

impl Default for QuadLayout {
    fn default() -> Self {
        Self {
            split: [0.5, 0.5],
            gutter: 4.0,
            maximized: false,
            active: 3,
        }
    }
}

impl QuadLayout {
    /// Tile rectangles; hidden tiles are `None` while one is maximised.
    pub fn tiles(&self, area: Rect) -> [Option<Rect>; 4] {
        if self.maximized {
            let mut out = [None; 4];
            out[self.active.min(3)] = Some(area);
            return out;
        }
        let g = self.gutter;
        let w = (area.width() - g).max(0.0);
        let h = (area.height() - g).max(0.0);
        let lw = (w * self.split[0]).round();
        let th = (h * self.split[1]).round();
        let x0 = area.min.x;
        let y0 = area.min.y;
        let x1 = x0 + lw + g;
        let y1 = y0 + th + g;
        [
            Some(Rect::from_min_size(pos2(x0, y0), vec2(lw, th))),
            Some(Rect::from_min_size(pos2(x1, y0), vec2(w - lw, th))),
            Some(Rect::from_min_size(pos2(x0, y1), vec2(lw, h - th))),
            Some(Rect::from_min_size(pos2(x1, y1), vec2(w - lw, h - th))),
        ]
    }

    /// The vertical and horizontal splitter bars (empty while maximised).
    pub fn splitters(&self, area: Rect) -> [Rect; 2] {
        if self.maximized {
            return [Rect::NOTHING, Rect::NOTHING];
        }
        let g = self.gutter;
        let lw = ((area.width() - g).max(0.0) * self.split[0]).round();
        let th = ((area.height() - g).max(0.0) * self.split[1]).round();
        [
            Rect::from_min_size(pos2(area.min.x + lw, area.min.y), vec2(g, area.height())),
            Rect::from_min_size(pos2(area.min.x, area.min.y + th), vec2(area.width(), g)),
        ]
    }

    /// Drag splitter `axis` (0 vertical, 1 horizontal) to a pointer position.
    pub fn drag_splitter(&mut self, axis: usize, pointer: f32, area: Rect) {
        let (lo, len) = if axis == 0 {
            (area.min.x, area.width() - self.gutter)
        } else {
            (area.min.y, area.height() - self.gutter)
        };
        if len > 1.0 {
            self.split[axis] = ((pointer - lo - self.gutter * 0.5) / len).clamp(0.1, 0.9);
        }
    }

    pub fn toggle_maximize(&mut self) {
        self.maximized = !self.maximized;
    }

    pub fn reset(&mut self) {
        *self = Self {
            active: self.active,
            ..Self::default()
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(1360.0, 760.0))
    }

    #[test]
    fn golden_tiles_cover_area() {
        let mut l = QuadLayout {
            gutter: 2.0,
            ..Default::default()
        };
        let tiles = l.tiles(area());
        let sum: f32 = tiles.iter().flatten().map(|r| r.area()).sum();
        assert!((sum - 1358.0 * 758.0).abs() < 1e-3);
        // No overlaps.
        for i in 0..4 {
            for j in i + 1..4 {
                assert!(!tiles[i].unwrap().intersects(tiles[j].unwrap()));
            }
        }
        l.active = 1;
        l.toggle_maximize();
        let max = l.tiles(area());
        assert_eq!(max[1], Some(area()));
        assert_eq!(max.iter().flatten().count(), 1);
        l.toggle_maximize();
        assert_eq!(l.tiles(area()), tiles);
    }

    #[test]
    fn splitter_drag_clamps() {
        let mut l = QuadLayout::default();
        l.drag_splitter(0, -50.0, area());
        assert_eq!(l.split[0], 0.1);
        l.drag_splitter(1, 380.0 + 2.0, area());
        assert!((l.split[1] - 0.5).abs() < 0.01);
        l.reset();
        assert_eq!(l.split, [0.5, 0.5]);
    }
}
