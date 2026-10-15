//! Display bound for pathological grapheme clusters.
//!
//! Unicode's Stream-Safe Text Format (UAX #15) limits a run of combining
//! characters to 30. Display applies a bound of the same order to every
//! grapheme cluster: only its first [`MAX_DISPLAY_CLUSTER_CHARS`] characters
//! are shaped and drawn. The text itself is unchanged, so editing, selection
//! and copying still see every character, and glyph indices always refer to
//! the original text. Shaping cost is then bounded by the glyphs a line can
//! show, not by the bytes a single cluster holds.
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation as _;

use crate::{FontRun, LineLayout, Pixels, PlatformTextSystem};

/// The most characters of one grapheme cluster that display shapes.
pub const MAX_DISPLAY_CLUSTER_CHARS: usize = 32;

/// The part of one grapheme cluster that display shapes.
pub fn displayed_cluster(cluster: &str) -> &str {
    cluster
        .char_indices()
        .nth(MAX_DISPLAY_CLUSTER_CHARS)
        .map_or(cluster, |(end, _)| &cluster[..end])
}

/// Byte ranges of `text` that display omits, ascending; empty for ordinary text.
fn omitted_ranges(text: &str) -> Vec<Range<usize>> {
    // Apart from CR LF, a cluster holds at most one ASCII character, so a
    // cluster over the bound needs half the bound of consecutive non-ASCII
    // characters. Text without such a run never pays for segmentation.
    let mut run = 0;
    let long_run = text.bytes().any(|byte| {
        if byte < 0x80 {
            run = 0;
        } else if byte >= 0xC0 {
            run += 1;
        }
        run >= MAX_DISPLAY_CLUSTER_CHARS / 2
    });
    if !long_run {
        return Vec::new();
    }
    text.grapheme_indices(true)
        .filter_map(|(start, cluster)| {
            let shown = displayed_cluster(cluster).len();
            (shown < cluster.len()).then(|| start + shown..start + cluster.len())
        })
        .collect()
}

/// Shape `text` as one line, each grapheme cluster bounded for display.
pub(crate) fn layout_line_bounded(
    platform: &dyn PlatformTextSystem,
    text: &str,
    font_size: Pixels,
    runs: &[FontRun],
) -> LineLayout {
    let omitted = omitted_ranges(text);
    if omitted.is_empty() {
        return platform.layout_line(text, font_size, runs);
    }

    let mut shaped = String::with_capacity(text.len().min(omitted.len() * 128 + 1024));
    let mut from = 0;
    for range in &omitted {
        shaped.push_str(&text[from..range.start]);
        from = range.end;
    }
    shaped.push_str(&text[from..]);

    // Each run keeps its font and loses the omitted bytes it covered. A run
    // boundary may fall inside an omitted range; both sides lose their part.
    let mut shaped_runs = Vec::with_capacity(runs.len());
    let mut next = 0;
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        let mut removed = 0;
        while let Some(range) = omitted.get(next) {
            if range.start >= end {
                break;
            }
            removed += range.end.min(end) - range.start.max(start);
            if range.end > end {
                break;
            }
            next += 1;
        }
        if run.len > removed {
            shaped_runs.push(FontRun {
                len: run.len - removed,
                font_id: run.font_id,
            });
        }
        start = end;
    }

    let mut layout = platform.layout_line(&shaped, font_size, &shaped_runs);

    // Where each omission begins in the shaped text, and the bytes omitted up
    // to and including it. The character shaped there follows the omission.
    let mut total = 0;
    let shifts: Vec<(usize, usize)> = omitted
        .iter()
        .map(|range| {
            let at = range.start - total;
            total += range.len();
            (at, total)
        })
        .collect();
    for glyph in layout.runs.iter_mut().flat_map(|run| &mut run.glyphs) {
        let ix = shifts.partition_point(|(at, _)| *at <= glyph.index);
        if let Some((_, shift)) = ix.checked_sub(1).map(|ix| shifts[ix]) {
            glyph.index += shift;
        }
    }
    layout.len = text.len();
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FontId, NoopTextSystem};

    fn shape(text: &str, runs: &[FontRun]) -> LineLayout {
        layout_line_bounded(&NoopTextSystem::new(), text, crate::px(14.), runs)
    }

    #[test]
    fn ordinary_text_shapes_unchanged() {
        let text = "中文段落内容，没有超长字素。".repeat(40) + "e\u{301}👩\u{200d}💻\t";
        let layout = shape(
            &text,
            &[FontRun {
                len: text.len(),
                font_id: FontId(0),
            }],
        );
        let indices: Vec<_> = layout.runs[0].glyphs.iter().map(|g| g.index).collect();
        let expected: Vec<_> = text.char_indices().map(|(ix, _)| ix).collect();
        assert_eq!(indices, expected);
        assert_eq!(layout.len, text.len());
    }

    #[test]
    fn long_clusters_shape_their_bounded_prefix_with_original_indices() {
        let marks = "\u{301}".repeat(100_000);
        let text = format!("a{marks}b{marks}c");
        let second = 1 + marks.len();
        let runs = [
            // A font boundary inside the first omitted tail.
            FontRun {
                len: 1 + 2 * 1_000,
                font_id: FontId(0),
            },
            FontRun {
                len: text.len() - 1 - 2 * 1_000,
                font_id: FontId(1),
            },
        ];
        let layout = shape(&text, &runs);
        let glyphs: Vec<_> = layout
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .map(|g| g.index)
            .collect();
        // Each bounded cluster keeps its base and 31 marks.
        assert_eq!(glyphs.len(), 2 * MAX_DISPLAY_CLUSTER_CHARS + 1);
        assert_eq!(glyphs[0], 0);
        assert_eq!(glyphs[MAX_DISPLAY_CLUSTER_CHARS - 1], 1 + 2 * 30);
        assert_eq!(glyphs[MAX_DISPLAY_CLUSTER_CHARS], second);
        assert_eq!(glyphs[2 * MAX_DISPLAY_CLUSTER_CHARS], text.len() - 1);
        assert_eq!(layout.len, text.len());
        assert_eq!(
            displayed_cluster(&text[second..text.len() - 1]).len(),
            1 + 2 * 31
        );
    }
}
