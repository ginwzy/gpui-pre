use std::sync::Arc;

use crate::{
    DecorationRun, FontRun, LineIndent, LineLayout, Pixels, SharedString, TextRun, TextSystem,
    WrappedLine, WrappedLineLayout,
};

/// Immutable shaping of one unwrapped display row, owned by its document.
/// Preparation runs on a background executor; attachment shares the glyphs.
#[derive(Debug)]
pub struct PreparedLine {
    pub(crate) layout: Arc<LineLayout>,
    text: SharedString,
    positions: Vec<(usize, Pixels)>,
    containing: Vec<(Pixels, usize)>,
    carets: Vec<(Pixels, usize, usize)>,
    pub(crate) ink_bounds: crate::Bounds<Pixels>,
    pub(crate) max_glyph_width: Pixels,
    pub(crate) glyph_count: usize,
    pub(crate) preview: Arc<LineLayout>,
}

impl PreparedLine {
    fn new(text: SharedString, layout: LineLayout, text_system: &TextSystem) -> Self {
        let mut positions: Vec<(usize, Pixels)> = Vec::new();
        let mut carets = Vec::new();
        let mut maximum = None;
        let mut ink_bounds: Option<crate::Bounds<Pixels>> = None;
        let mut max_glyph_width = crate::px(0.);
        let mut glyph_count = 0;
        for run in &layout.runs {
            let bbox = text_system.bounding_box(run.font_id, layout.font_size);
            let horizontal = bbox.left().abs().max(bbox.right().abs()) + crate::px(1.);
            let vertical = bbox.top().abs().max(bbox.bottom().abs()) + crate::px(1.);
            max_glyph_width = max_glyph_width.max(bbox.size.width);
            glyph_count += run.glyphs.len();
            for glyph in &run.glyphs {
                let bounds = crate::Bounds::new(
                    glyph.position - crate::point(horizontal, vertical),
                    crate::size(horizontal * 2., vertical * 2.),
                );
                ink_bounds = Some(ink_bounds.map_or(bounds, |old| old.union(&bounds)));
            }
        }
        for (order, glyph) in layout.runs.iter().flat_map(|run| &run.glyphs).enumerate() {
            if maximum.is_none_or(|max| glyph.index > max) {
                maximum = Some(glyph.index);
                if let Some(last) = positions
                    .last_mut()
                    .filter(|last| last.1 == glyph.position.x)
                {
                    last.0 = glyph.index;
                } else {
                    positions.push((glyph.index, glyph.position.x));
                }
            }
            if glyph.index < layout.len {
                carets.push((glyph.position.x, glyph.index, order));
            }
        }
        carets.sort_unstable_by(|a, b| {
            f32::from(a.0)
                .total_cmp(&f32::from(b.0))
                .then(a.2.cmp(&b.2))
        });
        carets.dedup_by(|a, b| a.0 == b.0);
        carets.shrink_to_fit();
        let mut containing: Vec<(Pixels, usize)> = Vec::new();
        for glyph in layout
            .runs
            .iter()
            .rev()
            .flat_map(|run| run.glyphs.iter().rev())
        {
            if containing
                .last()
                .is_none_or(|last| glyph.position.x < last.0)
            {
                containing.push((glyph.position.x, glyph.index));
            }
        }
        let mut remaining = 256;
        let runs = layout
            .runs
            .iter()
            .filter_map(|run| {
                let count = remaining.min(run.glyphs.len());
                remaining -= count;
                (count > 0).then(|| crate::ShapedRun {
                    font_id: run.font_id,
                    glyphs: run.glyphs[..count].to_vec(),
                })
            })
            .collect();
        let preview = Arc::new(LineLayout {
            font_size: layout.font_size,
            width: layout.width,
            ascent: layout.ascent,
            descent: layout.descent,
            len: layout.len,
            runs,
        });
        Self {
            text,
            layout: Arc::new(layout),
            positions,
            containing,
            carets,
            preview,
            ink_bounds: ink_bounds.unwrap_or_default(),
            max_glyph_width,
            glyph_count,
        }
    }

    pub(crate) fn x_for_index(&self, index: usize) -> Pixels {
        self.positions
            .get(self.positions.partition_point(|(end, _)| *end < index))
            .map_or(self.layout.width, |(_, x)| *x)
    }

    pub(crate) fn index_for_x(&self, x: Pixels) -> usize {
        self.containing
            .get(
                self.containing
                    .partition_point(|(position, _)| *position > x),
            )
            .map_or(0, |(_, index)| *index)
    }

    pub(crate) fn closest_caret(&self, x: Pixels) -> usize {
        let next = self
            .carets
            .partition_point(|(position, _, _)| *position < x);
        let mut closest = (0, x.abs(), 0);
        for ix in [next.checked_sub(1), Some(next)].into_iter().flatten() {
            if let Some((position, index, order)) = self.carets.get(ix) {
                let distance = (*position - x).abs();
                if distance < closest.1 || (distance == closest.1 && order + 1 < closest.2) {
                    closest = (*index, distance, order + 1);
                }
            }
        }
        if (self.layout.width - x).abs() < closest.1 {
            self.layout.len
        } else {
            closest.0
        }
    }
    /// The exact text used for shaping, shared without copying its bytes.
    pub fn text(&self) -> &SharedString {
        &self.text
    }

    pub(crate) fn decorated(&self, runs: &[TextRun], indent: LineIndent) -> WrappedLine {
        WrappedLine {
            layout: Arc::new(WrappedLineLayout {
                unwrapped_layout: self.layout.clone(),
                wrap_boundaries: Default::default(),
                wrap_width: None,
                indent,
            }),
            text: self.text.clone(),
            decoration_runs: runs
                .iter()
                .map(|run| DecorationRun {
                    len: run.len as u32,
                    color: run.color,
                    background_color: run.background_color,
                    underline: run.underline,
                    strikethrough: run.strikethrough,
                })
                .collect(),
        }
    }
}

impl TextSystem {
    /// Prepare one unwrapped row on a background executor. The caller owns
    /// request identity and must reject results for changed text or fonts.
    pub fn prepare_line(&self, text: SharedString, size: Pixels, runs: &[TextRun]) -> PreparedLine {
        let runs: Vec<_> = runs
            .iter()
            .map(|run| FontRun {
                len: run.len,
                font_id: self.resolve_font(&run.font),
            })
            .collect();
        let layout = self
            .platform_text_system
            .layout_line_in_background(&text, size, &runs);
        PreparedLine::new(text, layout, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontId, GlyphId, NoopTextSystem, ShapedGlyph, ShapedRun, point, px};

    #[test]
    fn prepared_indices_match_complete_glyph_walks() {
        let system = TextSystem::new(Arc::new(NoopTextSystem::new()));
        let text: SharedString = "a中e\u{301}👩\u{200d}💻\t".into();
        let boundaries: Vec<_> = text.char_indices().map(|(index, _)| index).collect();
        let mut seed = 17_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as usize
        };
        for _ in 0..128 {
            let glyphs: Vec<_> = (0..128)
                .map(|_| ShapedGlyph {
                    id: GlyphId(1),
                    index: boundaries[next() % boundaries.len()],
                    position: point(px((next() % 24) as f32 * 0.5 - 1.), px(0.)),
                    is_emoji: false,
                })
                .collect();
            let line = PreparedLine::new(
                text.clone(),
                LineLayout {
                    len: text.len(),
                    font_size: px(14.),
                    width: px(16.),
                    ascent: px(12.),
                    descent: px(3.),
                    runs: vec![
                        ShapedRun {
                            font_id: FontId(0),
                            glyphs: glyphs[..64].to_vec(),
                        },
                        ShapedRun {
                            font_id: FontId(1),
                            glyphs: glyphs[64..].to_vec(),
                        },
                    ],
                },
                &system,
            );
            for index in 0..=text.len() {
                assert_eq!(line.x_for_index(index), line.layout.x_for_index(index));
            }
            for n in -8..=72 {
                let x = px(n as f32 / 4.);
                if x < line.layout.width {
                    assert_eq!(line.index_for_x(x), line.layout.index_for_x(x).unwrap());
                }
                let mut closest = 0;
                let mut distance = x.abs();
                for glyph in &glyphs {
                    if glyph.index < text.len() && (glyph.position.x - x).abs() < distance {
                        closest = glyph.index;
                        distance = (glyph.position.x - x).abs();
                    }
                }
                if (line.layout.width - x).abs() < distance {
                    closest = text.len();
                }
                assert_eq!(line.closest_caret(x), closest);
            }
        }
    }
}
