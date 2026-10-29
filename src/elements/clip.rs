//! A composited rounded rectangle. Geometry is shared by painting and hit testing.

use crate::{
    AnyElement, App, Bounds, ContentMask, Corners, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Point, Window, px,
};
use std::sync::Arc;

/// Clips a complete element, including its background, to a rounded rectangle.
///
/// The child keeps its layout. Its painted contents are composited before the
/// rounded edge is applied, so overlapping backgrounds do not thicken the edge.
/// Shadows that should extend outside the clip belong on a surrounding element.
pub fn rounded_clip(
    id: impl Into<ElementId>,
    radii: impl Into<Corners<Pixels>>,
    child: impl IntoElement,
) -> RoundedClip {
    RoundedClip {
        id: id.into(),
        radii: radii.into(),
        child: child.into_any_element(),
        overlay_host: false,
    }
}

/// A rounded clipping element with no retained application state.
pub struct RoundedClip {
    id: ElementId,
    radii: Corners<Pixels>,
    child: AnyElement,
    overlay_host: bool,
}

impl RoundedClip {
    /// Make this scope the clipping host for deferred overlays inside it.
    ///
    /// Popups can escape intervening scroll containers, but remain within this
    /// host. Deferred content is composited into the same surface as its host.
    pub fn overlay_host(mut self, enabled: bool) -> Self {
        self.overlay_host = enabled;
        self
    }
}

/// Immutable geometry retained with hitboxes, cached views and deferred draws.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipContext {
    pub(crate) id: GlobalElementId,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) radii: Corners<Pixels>,
    pub(crate) mask: ContentMask<Pixels>,
    pub(crate) parent: Option<Arc<ClipContext>>,
}

impl ClipContext {
    pub(crate) fn contains(&self, position: Point<Pixels>) -> bool {
        self.mask.bounds.contains(&position)
            && rounded_contains(self.bounds, self.radii, position)
            && self
                .parent
                .as_ref()
                .is_none_or(|parent| parent.contains(position))
    }
}

pub(crate) fn rounded_contains(
    bounds: Bounds<Pixels>,
    radii: Corners<Pixels>,
    position: Point<Pixels>,
) -> bool {
    if !bounds.contains(&position) {
        return false;
    }
    let center = bounds.center();
    let (radius, x, y) = match (position.x < center.x, position.y < center.y) {
        (true, true) => (
            radii.top_left,
            bounds.left() + radii.top_left,
            bounds.top() + radii.top_left,
        ),
        (false, true) => (
            radii.top_right,
            bounds.right() - radii.top_right,
            bounds.top() + radii.top_right,
        ),
        (false, false) => (
            radii.bottom_right,
            bounds.right() - radii.bottom_right,
            bounds.bottom() - radii.bottom_right,
        ),
        (true, false) => (
            radii.bottom_left,
            bounds.left() + radii.bottom_left,
            bounds.bottom() - radii.bottom_left,
        ),
    };
    let dx = (position.x - center.x).abs() - (x - center.x).abs();
    let dy = (position.y - center.y).abs() - (y - center.y).abs();
    let dx = f32::from(dx.max(px(0.)));
    let dy = f32::from(dy.max(px(0.)));
    dx * dx + dy * dy <= f32::from(radius).powi(2)
}

impl Element for RoundedClip {
    type RequestLayoutState = ();
    type PrepaintState = Arc<ClipContext>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let context = Arc::new(ClipContext {
            id: id.expect("rounded clips have an element id").clone(),
            bounds,
            radii: self
                .radii
                .map(|r| (*r).max(px(0.)))
                .clamp_radii_for_quad_size(bounds.size),
            mask: ContentMask { bounds }.intersect(&window.content_mask()),
            parent: window.rounded_clip.clone(),
        });
        window.with_rounded_clip(context.clone(), self.overlay_host, |window| {
            self.child.prepaint(window, cx);
        });
        context
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _layout: &mut (),
        context: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rounded_clip(context.clone(), self.overlay_host, |window| {
            self.child.paint(window, cx);
        });
    }
}

impl IntoElement for RoundedClip {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

#[cfg(test)]
mod tests;
