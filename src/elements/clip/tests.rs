use super::rounded_contains;
use crate::{
    App, AppContext, Bounds, Context, Corners, Entity, GlobalElementId, InputEvent,
    InteractiveElement, IntoElement, MouseDownEvent, MouseUpEvent, ParentElement, Pixels, Render,
    Scene, StyleRefinement, Styled, TestAppContext, Window, anchored, deferred, div, point, px,
    red, rounded_clip, size,
};
use std::{cell::Cell, rc::Rc};

#[test]
fn asymmetric_rounding_keeps_original_corner_centers() {
    let bounds = Bounds::new(point(px(10.), px(20.)), size(px(100.), px(80.)));
    let radii = Corners {
        top_left: px(20.),
        top_right: px(0.),
        bottom_right: px(10.),
        bottom_left: px(5.),
    };
    assert!(!rounded_contains(bounds, radii, point(px(11.), px(21.))));
    assert!(rounded_contains(bounds, radii, point(px(30.), px(20.))));
    assert!(rounded_contains(bounds, radii, point(px(109.), px(21.))));
    assert!(!rounded_contains(bounds, radii, point(px(109.), px(99.))));
    assert!(!rounded_contains(bounds, radii, point(px(9.), px(60.))));
}

struct Content {
    clicks: Rc<Cell<usize>>,
    renders: Rc<Cell<usize>>,
}

impl Render for Content {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let clicks = self.clicks.clone();
        div().size_full().bg(red()).child(
            // The popup escapes this tiny scroll clip, but remains within its host.
            div().size(px(5.)).overflow_hidden().child(deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("popup")
                        .size(px(100.))
                        .bg(crate::blue())
                        .on_mouse_down(crate::MouseButton::Left, move |_, _, _| {
                            clicks.set(clicks.get() + 1)
                        }),
                ),
            )),
        )
    }
}

struct Host {
    radius: Pixels,
    content: Entity<Content>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        rounded_clip(
            "host",
            self.radius,
            self.content
                .clone()
                .cached(StyleRefinement::default().size_full()),
        )
        .overlay_host(true)
    }
}

#[crate::test]
fn rounded_clip_hosts_deferred_content_and_invalidates_cached_geometry(cx: &mut TestAppContext) {
    let clicks = Rc::new(Cell::new(0));
    let renders = Rc::new(Cell::new(0));
    let content = cx.new(|_| Content {
        clicks: clicks.clone(),
        renders: renders.clone(),
    });
    let window = cx.open_window(size(px(100.), px(100.)), |_, _| Host {
        radius: px(30.),
        content,
    });
    cx.run_until_parked();

    window
        .update(cx, |_, window, cx| {
            click(window, point(px(1.), px(1.)), cx);
            assert_eq!(
                clicks.get(),
                0,
                "clipped popup corner must not receive input"
            );
            click(window, point(px(50.), px(50.)), cx);
            assert_eq!(
                clicks.get(),
                1,
                "popup must escape its tiny scroll ancestor"
            );
            let scene = &window.rendered_frame.scene;
            assert_eq!(
                scene.composites.len(),
                1,
                "host and popup share one final mask"
            );
            assert_eq!(scene.composites[0].scene().quads.len(), 2);
        })
        .unwrap();
    cx.run_until_parked();

    let before = renders.get();
    for _ in 0..2 {
        window.update(cx, |_, _, cx| cx.notify()).unwrap();
        cx.run_until_parked();
        window
            .update(cx, |_, window, _| {
                let scene = &window.rendered_frame.scene;
                assert_eq!(scene.composites.len(), 1);
                assert!(scene.composites[0].scene().composites.is_empty());
                assert_eq!(scene.composites[0].scene().quads.len(), 2);
            })
            .unwrap();
    }
    assert_eq!(
        renders.get(),
        before,
        "unchanged clipped content must reuse its cache"
    );

    window
        .update(cx, |host, _, cx| {
            host.radius = px(0.);
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        renders.get() > before,
        "radius changes invalidate descendant hitboxes and paint"
    );
    window
        .update(cx, |_, window, cx| {
            assert!(
                window.rendered_frame.scene.composites.is_empty(),
                "square clips keep the direct path"
            );
            click(window, point(px(1.), px(1.)), cx);
            assert_eq!(clicks.get(), 2);
        })
        .unwrap();
}

#[test]
fn scene_replay_preserves_nested_surfaces_and_deferred_reentry() {
    use crate::{ContentMask, Quad, ScaledPixels};
    let bounds = Bounds::new(
        point(ScaledPixels(0.), ScaledPixels(0.)),
        size(ScaledPixels(100.), ScaledPixels(100.)),
    );
    let mask = ContentMask { bounds };
    let outer = GlobalElementId(vec![crate::ElementId::Name("outer".into())].into());
    let inner = GlobalElementId(vec![crate::ElementId::Name("inner".into())].into());
    let quad = Quad {
        bounds,
        content_mask: mask,
        ..Default::default()
    };
    let mut scene = Scene::default();
    scene.push_clip(outer.clone(), bounds, Corners::all(ScaledPixels(20.)), mask);
    scene.insert_primitive(quad);
    scene.push_clip(inner, bounds, Corners::all(ScaledPixels(10.)), mask);
    scene.insert_primitive(quad);
    scene.pop_clip();
    scene.pop_clip();
    scene.push_clip(outer, bounds, Corners::all(ScaledPixels(20.)), mask);
    scene.insert_primitive(quad);
    scene.pop_clip();
    scene.finish();
    let mut replay = Scene::default();
    replay.replay(0..scene.len(), &scene);
    replay.finish();
    assert_eq!(replay.composites.len(), 1);
    let root = replay.composites[0].scene();
    assert_eq!(root.quads.len(), 2);
    assert_eq!(root.composites.len(), 1);
    assert_eq!(root.composites[0].scene().quads.len(), 1);
    let labels: Vec<_> = root.batches().map(|batch| batch.label()).collect();
    assert_eq!(labels, ["quads (1)", "composites (1)", "quads (1)"]);
}

fn click(window: &mut Window, position: crate::Point<Pixels>, cx: &mut App) {
    window.dispatch_event(
        MouseDownEvent {
            position,
            button: crate::MouseButton::Left,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        }
        .to_platform_input(),
        cx,
    );
    window.dispatch_event(
        MouseUpEvent {
            position,
            button: crate::MouseButton::Left,
            modifiers: Default::default(),
            click_count: 1,
        }
        .to_platform_input(),
        cx,
    );
}

#[test]
fn isolation_splits_an_explicit_paint_layer() {
    use crate::{ContentMask, Quad, ScaledPixels};
    let bounds = Bounds::new(
        point(ScaledPixels(0.), ScaledPixels(0.)),
        size(ScaledPixels(100.), ScaledPixels(100.)),
    );
    let mask = ContentMask { bounds };
    let id = GlobalElementId(vec![crate::ElementId::Name("isolated".into())].into());
    let quad = Quad {
        bounds,
        content_mask: mask,
        ..Default::default()
    };
    let mut scene = Scene::default();
    scene.push_layer(bounds);
    scene.insert_primitive(quad);
    scene.push_clip(id, bounds, Corners::all(ScaledPixels(10.)), mask);
    scene.insert_primitive(quad);
    scene.pop_clip();
    scene.insert_primitive(quad);
    scene.pop_layer();
    scene.finish();
    let labels: Vec<_> = scene.batches().map(|batch| batch.label()).collect();
    assert_eq!(labels, ["quads (1)", "composites (1)", "quads (1)"]);
    let mut replay = Scene::default();
    replay.replay(0..scene.len(), &scene);
    replay.finish();
    assert_eq!(
        replay
            .batches()
            .map(|batch| batch.label())
            .collect::<Vec<_>>(),
        labels
    );
}
