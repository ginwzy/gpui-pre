use crate::{Bounds, Corners, ScaledPixels, point, size};
use smallvec::SmallVec;

/// Disjoint, device-pixel-aligned regions covering the visible rounded rectangle.
/// Only corners and fractional straight edges need an isolated blending surface.
pub struct ClipTiles {
    direct: SmallVec<[Bounds<ScaledPixels>; 5]>,
    isolated: SmallVec<[Bounds<ScaledPixels>; 8]>,
}

impl ClipTiles {
    pub(super) fn new(
        bounds: Bounds<ScaledPixels>,
        radii: Corners<ScaledPixels>,
        mask: Bounds<ScaledPixels>,
        viewport: Bounds<ScaledPixels>,
    ) -> Self {
        let mut tiles = Self {
            direct: SmallVec::new(),
            isolated: SmallVec::new(),
        };
        let visible = bounds.intersect(&mask).intersect(&viewport);
        if visible.is_empty() {
            return tiles;
        }
        let rect = |left: f32, top: f32, right: f32, bottom: f32| {
            Bounds::new(
                point(ScaledPixels(left), ScaledPixels(top)),
                size(ScaledPixels(right - left), ScaledPixels(bottom - top)),
            )
        };
        tiles.direct.push(rect(
            visible.left().0.floor(),
            visible.top().0.floor(),
            visible.right().0.ceil(),
            visible.bottom().0.ceil(),
        ));
        let l = bounds.left().0;
        let t = bounds.top().0;
        let r = bounds.right().0;
        let b = bounds.bottom().0;
        // Fractional straight edges also have partial coverage. Keep their one-pixel
        // strips isolated; the direct region must only contain full-coverage pixels.
        for edge in [
            rect(l.floor(), t.floor(), l.ceil(), b.ceil()),
            rect(r.floor(), t.floor(), r.ceil(), b.ceil()),
            rect(l.floor(), t.floor(), r.ceil(), t.ceil()),
            rect(l.floor(), b.floor(), r.ceil(), b.ceil()),
        ] {
            tiles.isolate(edge);
        }
        for (radius, left, top) in [
            (radii.top_left.0, true, true),
            (radii.top_right.0, false, true),
            (radii.bottom_left.0, true, false),
            (radii.bottom_right.0, false, false),
        ] {
            if radius <= 0. {
                continue;
            }
            let (x0, x1) = if left {
                (l.floor(), (l + radius).ceil())
            } else {
                ((r - radius).floor(), r.ceil())
            };
            let (y0, y1) = if top {
                (t.floor(), (t + radius).ceil())
            } else {
                ((b - radius).floor(), b.ceil())
            };
            tiles.isolate(rect(x0, y0, x1, y1));
        }
        tiles
    }

    /// Pixels with full mask coverage, drawn into the parent target in paint order.
    pub fn direct(&self) -> &[Bounds<ScaledPixels>] {
        &self.direct
    }

    /// Pixels needing a single rounded mask after their contents have blended.
    pub fn isolated(&self) -> &[Bounds<ScaledPixels>] {
        &self.isolated
    }

    fn isolate(&mut self, bounds: Bounds<ScaledPixels>) {
        if bounds.is_empty() {
            return;
        }
        let mut remaining = SmallVec::new();
        for region in self.direct.drain(..) {
            let cut = region.intersect(&bounds);
            if cut.is_empty() {
                remaining.push(region);
                continue;
            }
            self.isolated.push(cut);
            // Subtract only from regions still direct, so even tiny circles with
            // overlapping, outward-rounded corner boxes never blend a pixel twice.
            for (origin, extent) in [
                (
                    region.origin,
                    size(region.size.width, cut.top() - region.top()),
                ),
                (
                    point(region.left(), cut.bottom()),
                    size(region.size.width, region.bottom() - cut.bottom()),
                ),
                (
                    point(region.left(), cut.top()),
                    size(cut.left() - region.left(), cut.size.height),
                ),
                (
                    point(cut.right(), cut.top()),
                    size(region.right() - cut.right(), cut.size.height),
                ),
            ] {
                let piece = Bounds::new(origin, extent);
                if !piece.is_empty() {
                    remaining.push(piece);
                }
            }
        }
        self.direct = remaining;
    }
}
