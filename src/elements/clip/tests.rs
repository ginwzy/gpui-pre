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

#[test]
fn clip_tiles_cover_each_pixel_once_and_only_isolate_edges() {
    use crate::{ContentMask, ScaledPixels};
    let rect = |x, y, w, h| {
        Bounds::new(
            point(ScaledPixels(x), ScaledPixels(y)),
            size(ScaledPixels(w), ScaledPixels(h)),
        )
    };
    for scale in [1., 1.25, 1.5, 2.] {
        for (width, height) in [(100., 80.), (3., 3.), (1., 1.)] {
            for offset in [0., 0.2, 0.5, 0.9] {
                for corners in [[16., 16., 16., 16.], [0., 10., 5., 20.], [0.; 4]] {
                    let bounds = rect(
                        10. * scale + offset,
                        20. * scale + offset,
                        width * scale,
                        height * scale,
                    );
                    let radii = Corners {
                        top_left: ScaledPixels(corners[0] * scale),
                        top_right: ScaledPixels(corners[1] * scale),
                        bottom_left: ScaledPixels(corners[2] * scale),
                        bottom_right: ScaledPixels(corners[3] * scale),
                    }
                    .clamp_radii_for_quad_size(bounds.size);
                    for mask in [rect(0., 0., 256., 256.), rect(20., 25., 100., 90.)] {
                        let mut scene = Scene::default();
                        scene.push_clip(
                            GlobalElementId::default(),
                            bounds,
                            radii,
                            ContentMask { bounds: mask },
                        );
                        scene.pop_clip();
                        let viewport = rect(0., 0., 200., 170.);
                        let tiles = scene.composites[0].tiles(viewport);
                        let visible = bounds.intersect(&mask).intersect(&viewport);
                        if visible.is_empty() {
                            assert!(tiles.direct().is_empty() && tiles.isolated().is_empty());
                            continue;
                        }
                        let contains = |r: &Bounds<ScaledPixels>, x: f32, y: f32| {
                            x >= r.left().0 && x < r.right().0 && y >= r.top().0 && y < r.bottom().0
                        };
                        let regions: Vec<_> =
                            tiles.direct().iter().chain(tiles.isolated()).collect();
                        for r in &regions {
                            assert!(!r.is_empty());
                            assert_eq!(r.origin.x.0.fract(), 0.);
                            assert_eq!(r.origin.y.0.fract(), 0.);
                            assert_eq!(r.size.width.0.fract(), 0.);
                            assert_eq!(r.size.height.0.fract(), 0.);
                        }
                        for y in
                            visible.top().0.floor() as i32 - 1..=visible.bottom().0.ceil() as i32
                        {
                            for x in visible.left().0.floor() as i32 - 1
                                ..=visible.right().0.ceil() as i32
                            {
                                let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
                                let inside = x >= visible.left().0.floor()
                                    && x < visible.right().0.ceil()
                                    && y >= visible.top().0.floor()
                                    && y < visible.bottom().0.ceil();
                                assert_eq!(
                                    regions.iter().filter(|r| contains(r, x, y)).count(),
                                    usize::from(inside),
                                    "overlap or gap at {x},{y}"
                                );
                                if tiles.direct().iter().any(|r| contains(r, x, y)) {
                                    // Evaluate the shader's rounded-rectangle distance at the pixel center.
                                    let dx = x - (bounds.left().0 + bounds.size.width.0 / 2.);
                                    let dy = y - (bounds.top().0 + bounds.size.height.0 / 2.);
                                    let radius = match (dx < 0., dy < 0.) {
                                        (true, true) => radii.top_left.0,
                                        (false, true) => radii.top_right.0,
                                        (true, false) => radii.bottom_left.0,
                                        (false, false) => radii.bottom_right.0,
                                    };
                                    let qx = dx.abs() - bounds.size.width.0 / 2. + radius;
                                    let qy = dy.abs() - bounds.size.height.0 / 2. + radius;
                                    let distance =
                                        qx.max(0.).hypot(qy.max(0.)) + qx.max(qy).min(0.) - radius;
                                    assert!(
                                        distance <= -0.5 + 0.0001,
                                        "partial coverage leaked into direct tile: {distance}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn clip_tiles_keep_4k_isolation_bounded_by_corner_size() {
    use crate::{ContentMask, ScaledPixels};
    let bounds = Bounds::new(
        point(ScaledPixels(0.), ScaledPixels(0.)),
        size(ScaledPixels(3840.), ScaledPixels(2160.)),
    );
    let mut scene = Scene::default();
    scene.push_clip(
        GlobalElementId::default(),
        bounds,
        Corners::all(ScaledPixels(16.)),
        ContentMask { bounds },
    );
    scene.pop_clip();
    let tiles = scene.composites[0].tiles(bounds);
    assert_eq!(tiles.isolated().len(), 4);
    assert_eq!(tiles.direct().len(), 3);
    let area: f32 = tiles
        .isolated()
        .iter()
        .map(|r| r.size.width.0 * r.size.height.0)
        .sum();
    assert_eq!(area, 4. * 16. * 16.);
}

#[test]
fn visible_range_keeps_shadows_drawn_beyond_their_bounds() {
    use crate::{ContentMask, Hsla, PrimitiveBatch, ScaledPixels, Shadow};
    let rect = |x, y, w, h| {
        Bounds::new(
            point(ScaledPixels(x), ScaledPixels(y)),
            size(ScaledPixels(w), ScaledPixels(h)),
        )
    };
    let mask = ContentMask {
        bounds: rect(0., 0., 400., 400.),
    };
    let shadow = |bounds, element_bounds, blur_radius, inset| Shadow {
        order: 0,
        blur_radius: ScaledPixels(blur_radius),
        bounds,
        corner_radii: Corners::default(),
        content_mask: mask,
        color: Hsla::default(),
        element_bounds,
        element_corner_radii: Corners::default(),
        inset,
        pad: 0,
    };
    // A region (such as the band between two corners) next to each shadow's bounds.
    let region = rect(0., 0., 400., 16.);
    for (shadow, visible) in [
        // The gaussian tail reaches 3 * 4px above the drop shadow's bounds.
        (shadow(rect(50., 20., 100., 100.), rect(50., 20., 100., 100.), 4., 0), true),
        (shadow(rect(50., 40., 100., 100.), rect(50., 40., 100., 100.), 4., 0), false),
        // An inset shadow is drawn over its element, though its hole is offset away.
        (shadow(rect(50., 30., 100., 100.), rect(50., 10., 100., 100.), 0., 1), true),
    ] {
        let mut scene = Scene::default();
        scene.insert_primitive(shadow);
        scene.finish();
        let batch = PrimitiveBatch::Shadows(0..1);
        assert_eq!(!scene.visible_range(&batch, region).is_empty(), visible);
    }
}
