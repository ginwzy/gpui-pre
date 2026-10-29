use crate::{ClipContext, ContentMask, Window};
use std::{mem, sync::Arc};

use super::DrawPhase;

impl Window {
    pub(crate) fn with_rounded_clip<R>(
        &mut self,
        clip: Arc<ClipContext>,
        overlay_host: bool,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint_or_prepaint();
        let painting = self.invalidator.inner.borrow().draw_phase == DrawPhase::Paint;
        let composited = painting && clip.radii != crate::Corners::default();
        if composited {
            self.next_frame.scene.push_clip(
                clip.id.clone(),
                clip.bounds.scale(self.scale_factor()),
                clip.radii.scale(self.scale_factor()),
                ContentMask {
                    bounds: self.cover_bounds(clip.mask.bounds),
                },
            );
        }
        let previous = self.rounded_clip.replace(clip.clone());
        let previous_host = if overlay_host {
            mem::replace(&mut self.overlay_clip, Some(clip.clone()))
        } else {
            self.overlay_clip.clone()
        };
        let result = self.with_content_mask(Some(clip.mask), f);
        self.rounded_clip = previous;
        self.overlay_clip = previous_host;
        if composited {
            self.next_frame.scene.pop_clip();
        }
        result
    }

    /// Deferred content rejoins its host's composition surface. It escapes local
    /// scroll clips while retaining the host's geometry in both drawing phases.
    pub(crate) fn with_overlay_clip<R>(
        &mut self,
        clip: Option<Arc<ClipContext>>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        fn enter<R>(
            window: &mut Window,
            clips: &[Arc<ClipContext>],
            f: &mut dyn FnMut(&mut Window) -> R,
        ) -> R {
            if let Some((clip, rest)) = clips.split_first() {
                window.with_rounded_clip(clip.clone(), true, |window| enter(window, rest, f))
            } else {
                f(window)
            }
        }
        let mut clips = Vec::new();
        let mut current = clip;
        while let Some(clip) = current {
            current = clip.parent.clone();
            clips.push(clip);
        }
        clips.reverse();
        let mut f = Some(f);
        enter(self, &clips, &mut |window| {
            f.take().expect("overlay continuation runs once")(window)
        })
    }
}
