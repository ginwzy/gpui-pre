use super::{
    SvgRenderer, SvgSize, fix_generic_font_families, font_has_char, is_emoji_presentation,
    select_emoji_font,
};
use crate::{DevicePixels, Result, Size};
use std::sync::Arc;
use usvg::fontdb::{Database, Family, Query};

const IBM_PLEX_REGULAR: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fonts/IBMPlexSans-Regular.ttf"
));
const LILEX_REGULAR: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fonts/Lilex-Regular.ttf"
));

#[test]
fn renders_parsed_svg_at_requested_size() -> Result<()> {
    let renderer = SvgRenderer::new(Arc::new(()));
    let svg = renderer.parse_svg(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="24pt" height="12pt"></svg>"#,
    )?;
    assert_eq!(svg.intrinsic_size(), Size::new(32., 16.));
    let requested_size = Size::new(DevicePixels(24), DevicePixels(12));
    let image = renderer.render_parsed(&svg, SvgSize::ExactSize(requested_size))?;

    assert_eq!(image.size(0), requested_size);
    Ok(())
}

#[test]
fn preserves_aspect_ratio_for_width_constrained_size() -> Result<()> {
    let renderer = SvgRenderer::new(Arc::new(()));
    let svg = renderer.parse_svg(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="24pt" height="12pt"></svg>"#,
    )?;
    let image = renderer.render_parsed(
        &svg,
        SvgSize::Size(Size::new(DevicePixels(24), DevicePixels(24))),
    )?;

    assert_eq!(image.size(0), Size::new(DevicePixels(24), DevicePixels(12)));
    Ok(())
}

fn db_with_bundled_fonts() -> Database {
    let mut db = Database::new();
    db.load_font_data(IBM_PLEX_REGULAR.to_vec());
    db.load_font_data(LILEX_REGULAR.to_vec());
    db
}

#[test]
fn text_with_split_glyph_clusters_in_mixed_fonts_does_not_panic() {
    let mut db = Database::new();
    db.load_font_data(IBM_PLEX_REGULAR.to_vec());
    db.load_font_data(LILEX_REGULAR.to_vec());
    let options = usvg::Options {
        fontdb: std::sync::Arc::new(db),
        ..Default::default()
    };

    // A base letter followed by a stack of combining marks. Under HarfBuzz's
    // default cluster merging every mark glyph shares the base's byte index,
    // which is the "glyph splitting" condition that triggered the panic. The
    // chunk must use two different fonts so the buggy merge path runs.
    let zalgo = "e\u{0301}\u{0302}\u{0303}\u{0304}\u{0306}\u{0307}\u{0308}\u{030a}";
    let svg = format!(
        r#"<svg viewBox="0 0 200 200" xmlns="http://www.w3.org/2000/svg"><text font-family="Lilex" font-size="32">{zalgo}<tspan font-family="IBM Plex Sans">{zalgo}</tspan></text></svg>"#
    );

    // Before the fix this aborts via panic with a message like
    // "removal index (is 5) should be < len (is 5)".
    usvg::Tree::from_data(svg.as_bytes(), &options).expect("SVG with mixed-font text should parse");
}

#[test]
fn test_is_emoji_presentation() {
    let cases = [
        ("a", false),
        ("Z", false),
        ("1", false),
        ("#", false),
        ("*", false),
        ("漢", false),
        ("中", false),
        ("カ", false),
        ("©", false),
        ("♥", false),
        ("😀", true),
        ("✅", true),
        ("🇺🇸", true),
        // SVG fallback is not cluster-aware yet
        ("©️", false),
        ("♥️", false),
        ("1️⃣", false),
    ];
    for (s, expected) in cases {
        assert_eq!(
            is_emoji_presentation(s.chars().next().unwrap()),
            expected,
            "for char {:?}",
            s
        );
    }
}

#[test]
fn fix_generic_font_families_sets_all_families() {
    let mut db = db_with_bundled_fonts();
    fix_generic_font_families(&mut db);

    let families = [
        Family::SansSerif,
        Family::Serif,
        Family::Monospace,
        Family::Cursive,
        Family::Fantasy,
    ];

    for family in families {
        let query = Query {
            families: &[family],
            ..Default::default()
        };
        assert!(
            db.query(&query).is_some(),
            "Expected generic family {family:?} to resolve after fix_generic_font_families"
        );
    }
}

#[test]
fn test_select_emoji_font_skips_family_without_glyph() {
    let mut db = db_with_bundled_fonts();

    let ibm_plex_sans = db
        .query(&usvg::fontdb::Query {
            families: &[usvg::fontdb::Family::Name("IBM Plex Sans")],
            weight: usvg::fontdb::Weight(400),
            stretch: usvg::fontdb::Stretch::Normal,
            style: usvg::fontdb::Style::Normal,
        })
        .unwrap();
    let lilex = db
        .query(&usvg::fontdb::Query {
            families: &[usvg::fontdb::Family::Name("Lilex")],
            weight: usvg::fontdb::Weight(400),
            stretch: usvg::fontdb::Stretch::Normal,
            style: usvg::fontdb::Style::Normal,
        })
        .unwrap();
    let selected = select_emoji_font('│', &[], &db, &["IBM Plex Sans", "Lilex"]).unwrap();

    assert_eq!(selected, lilex);
    assert!(!font_has_char(&db, ibm_plex_sans, '│'));
    assert!(font_has_char(&db, selected, '│'));
}

#[test]
fn fix_generic_font_families_monospace_resolves_to_lilex() {
    let mut db = db_with_bundled_fonts();
    fix_generic_font_families(&mut db);

    let query = Query {
        families: &[Family::Monospace],
        ..Default::default()
    };
    let id = db.query(&query).expect("Monospace should resolve");
    let face = db.face(id).expect("Face should exist");
    assert!(
        face.families.iter().any(|(name, _)| name.contains("Lilex")),
        "Monospace should map to Lilex, got {:?}",
        face.families
    );
}

#[test]
fn fractional_intrinsic_sizes_preserve_the_requested_device_width() -> Result<()> {
    let renderer = SvgRenderer::new(Arc::new(()));
    let svg = renderer
        .parse_svg(br#"<svg xmlns="http://www.w3.org/2000/svg" width="0.3" height="0.15"/>"#)?;
    let image = renderer.render_parsed(
        &svg,
        SvgSize::Size(Size::new(DevicePixels(480), DevicePixels(1))),
    )?;
    assert_eq!(
        image.size(0),
        Size::new(DevicePixels(480), DevicePixels(240))
    );
    Ok(())
}

#[test]
fn width_requests_keep_the_existing_side_cap_and_aspect_ratio() -> Result<()> {
    let renderer = SvgRenderer::new(Arc::new(()));
    let svg = renderer
        .parse_svg(br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="1"/>"#)?;
    let image = renderer.render_parsed(
        &svg,
        SvgSize::Size(Size::new(DevicePixels(32768), DevicePixels(1))),
    )?;
    assert_eq!(
        image.size(0),
        Size::new(DevicePixels(8192), DevicePixels(81))
    );
    Ok(())
}

#[test]
fn explicit_font_assets_are_isolated_and_change_text_geometry() -> Result<()> {
    let base = SvgRenderer::new(Arc::new(()));
    let plex = base.with_fonts([Arc::new(IBM_PLEX_REGULAR.to_vec())], "IBM Plex Sans");
    let lilex = base.with_fonts([Arc::new(LILEX_REGULAR.to_vec())], "Lilex");
    assert_eq!(plex.usvg_options.fontdb.faces().count(), 1);
    assert_eq!(lilex.usvg_options.fontdb.faces().count(), 1);
    assert!(base.usvg_options.fontdb.is_empty());
    let source = br#"<svg xmlns="http://www.w3.org/2000/svg" width="160" height="40"><text y="30" font-size="24">iiiiWWW</text></svg>"#;
    let plex = plex.render_parsed(
        &plex.parse_svg(source)?,
        SvgSize::Size(Size::new(DevicePixels(160), DevicePixels(40))),
    )?;
    let lilex = lilex.render_parsed(
        &lilex.parse_svg(source)?,
        SvgSize::Size(Size::new(DevicePixels(160), DevicePixels(40))),
    )?;
    assert!(
        plex.as_bytes(0)
            .unwrap()
            .chunks_exact(4)
            .any(|pixel| pixel[3] != 0)
    );
    assert!(
        lilex
            .as_bytes(0)
            .unwrap()
            .chunks_exact(4)
            .any(|pixel| pixel[3] != 0)
    );
    assert_ne!(plex.as_bytes(0), lilex.as_bytes(0));
    Ok(())
}

#[test]
fn explicit_font_renderer_does_not_load_external_images() -> Result<()> {
    let path = std::env::temp_dir().join(format!("gpui-svg-resource-{}.svg", std::process::id()));
    std::fs::write(
        &path,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="red"/></svg>"#,
    )?;
    let source = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><image href="{}" width="8" height="8"/></svg>"#,
        path.display()
    );
    let base = SvgRenderer::new(Arc::new(()));
    let closed = base.with_fonts([Arc::new(IBM_PLEX_REGULAR.to_vec())], "IBM Plex Sans");
    let original = base.parse_svg(source.as_bytes())?;
    let isolated = closed.parse_svg(source.as_bytes())?;
    std::fs::remove_file(path)?;
    let dimensions = SvgSize::Size(Size::new(DevicePixels(8), DevicePixels(8)));
    let original = base.render_parsed(&original, dimensions)?;
    let isolated = closed.render_parsed(&isolated, dimensions)?;
    assert!(
        original
            .as_bytes(0)
            .unwrap()
            .chunks_exact(4)
            .any(|pixel| pixel[3] != 0)
    );
    assert!(
        isolated
            .as_bytes(0)
            .unwrap()
            .chunks_exact(4)
            .all(|pixel| pixel[3] == 0)
    );
    Ok(())
}
