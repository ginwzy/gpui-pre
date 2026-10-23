//! Whole-line advances only propose breaks. Each published row is shaped in
//! its final context, so a ligature can never carry ink across a soft wrap.
use super::{LineIndent, LineLayout, ShapedRow};
use crate::{LineWrapper, Pixels, px};
use smallvec::SmallVec;
use std::{ops::Range, sync::Arc};
use unicode_segmentation::UnicodeSegmentation as _;

struct Cluster {
    range: Range<usize>,
    advance: Pixels,
    preferred: bool,
}

pub(super) fn shape(
    text: &str,
    unwrapped: Arc<LineLayout>,
    width: Option<Pixels>,
    max_lines: Option<usize>,
    indent: LineIndent,
    mut shape_row: impl FnMut(Range<usize>) -> Arc<LineLayout>,
) -> SmallVec<[ShapedRow; 1]> {
    let mut rows = SmallVec::new();
    let Some(width) = width
        .filter(|width| !text.is_empty() && unwrapped.width > (*width - indent.first).max(px(0.)))
    else {
        rows.push(ShapedRow {
            range: 0..text.len(),
            layout: unwrapped,
        });
        return rows;
    };

    // Glyphs are in visual order. Accumulate their advance contributions in
    // source order to propose breaks even when a run contains RTL text. These
    // are estimates only: shaping a row can change advances and glyph count.
    let mut glyphs = unwrapped.runs.iter().flat_map(|run| &run.glyphs).peekable();
    let mut advances = Vec::new();
    while let Some(glyph) = glyphs.next() {
        let next_x = glyphs
            .peek()
            .map_or(unwrapped.width, |next| next.position.x);
        advances.push((glyph.index, next_x - glyph.position.x));
    }
    advances.sort_by_key(|(index, _)| *index);
    let mut advances = advances.into_iter().peekable();
    let mut previous = '\0';
    let mut indented = false;
    let clusters = text
        .grapheme_indices(true)
        .map(|(start, cluster)| {
            let end = start + cluster.len();
            let first = cluster.chars().next().unwrap();
            let preferred = indented && LineWrapper::can_break_before(previous, first);
            previous = cluster.chars().next_back().unwrap();
            indented |= first != ' ';
            let mut advance = px(0.);
            while advances.peek().is_some_and(|(index, _)| *index < end) {
                advance += advances.next().unwrap().1;
            }
            Cluster {
                range: start..end,
                advance,
                preferred,
            }
        })
        .collect::<Vec<_>>();

    let mut start = 0;
    while start < clusters.len() {
        let limit = (width - indent.for_row(rows.len())).max(px(0.));
        let clamped = max_lines.is_some_and(|max| rows.len() >= max.saturating_sub(1));
        let mut end = clusters.len();
        if !clamped {
            let mut advance = px(0.);
            let mut preferred = None;
            for (ix, cluster) in clusters.iter().enumerate().skip(start) {
                if ix > start && cluster.preferred {
                    preferred = Some(ix);
                }
                advance += cluster.advance;
                if advance > limit && ix > start {
                    end = preferred.unwrap_or(ix);
                    break;
                }
            }
        }

        loop {
            let range = clusters[start].range.start..clusters[end - 1].range.end;
            let layout = if range == (0..text.len()) {
                unwrapped.clone()
            } else {
                shape_row(range.clone())
            };
            // An oversized grapheme remains whole. A clamped final row keeps
            // the caller's remaining text; truncation/ellipsis is handled above.
            if layout.width <= limit || end == start + 1 || clamped {
                rows.push(ShapedRow { range, layout });
                start = end;
                break;
            }

            // Revalidate after retreating: splitting a contextual glyph can
            // change its width again. Always remove at least one whole cluster.
            let fit = range.start + layout.index_for_x(limit).unwrap_or(layout.len);
            let count = clusters[start..end]
                .partition_point(|cluster| cluster.range.end <= fit)
                .clamp(1, end - start - 1);
            end = start + count;
            if let Some(preferred) = (start + 1..=end).rev().find(|&ix| clusters[ix].preferred) {
                end = preferred;
            }
        }
    }
    rows
}
