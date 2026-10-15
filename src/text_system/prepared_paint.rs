//! Background grayscale composition for display rows with excessive glyph counts.
//! Geometry and hit testing still use the complete shaped row. The image covers
//! only the current clip; it never determines a document's extent.
use crate::{
    Bounds, DecorationRun, DevicePixels, Hsla, Pixels, Point, PreparedLine, RenderGlyphParams,
    RenderImage, Result, TextAlign, TextSystem, Window, point, px, size,
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Immutable inputs for one clipped row image. The document owns scheduling.
#[derive(Clone)]
pub struct PreparedLinePaint {
    line: Arc<PreparedLine>,
    decorations: Arc<[DecorationRun]>,
    origin: Point<Pixels>,
    image_bounds: Bounds<Pixels>,
    height: Pixels,
    scale: f32,
}

/// A finished row image; attaching it does not visit the row's glyphs.
pub struct PreparedLineImage {
    bounds: Bounds<Pixels>,
    image: Arc<RenderImage>,
}

impl PreparedLineImage {
    /// Paint into the same window coordinates for which this image was prepared.
    pub fn paint(&self, window: &mut Window) -> Result<()> {
        window.paint_image(
            self.bounds,
            self.bounds,
            Default::default(),
            self.image.clone(),
            0,
            false,
        )
    }
}

impl PreparedLinePaint {
    pub(crate) fn new(
        line: Arc<PreparedLine>,
        decorations: &[DecorationRun],
        bounds: Bounds<Pixels>,
        indent: Pixels,
        height: Pixels,
        align: TextAlign,
        window: &Window,
    ) -> Self {
        let width = bounds.size.width - indent;
        let mut origin = bounds.origin + point(indent, px(0.));
        origin.x += match align {
            TextAlign::Left => px(0.),
            TextAlign::Center => (width - line.layout.width) / 2.,
            TextAlign::Right => width - line.layout.width,
        };
        let baseline =
            (height - line.layout.ascent - line.layout.descent) / 2. + line.layout.ascent;
        let ink = Bounds::new(
            origin
                + point(
                    line.ink_bounds.origin.x,
                    line.ink_bounds.origin.y + baseline,
                ),
            line.ink_bounds.size,
        );
        let decorations_bounds = Bounds::new(
            origin,
            size(line.layout.width.max(line.max_glyph_width), height * 2.),
        );
        let extent = ink
            .union(&decorations_bounds)
            .intersect(&window.content_mask().bounds);
        let scale = window.scale_factor();
        let left = (f32::from(extent.left()) * scale).floor() / scale;
        let top = (f32::from(extent.top()) * scale).floor() / scale;
        let right = (f32::from(extent.right()) * scale).ceil() / scale;
        let bottom = (f32::from(extent.bottom()) * scale).ceil() / scale;
        Self {
            line,
            decorations: decorations.into(),
            origin,
            image_bounds: Bounds::new(
                point(px(left), px(top)),
                size(px((right - left).max(0.)), px((bottom - top).max(0.))),
            ),
            height,
            scale,
        }
    }

    /// Whether a retained result answers these exact shaping, paint and clip inputs.
    pub fn same_request(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.line, &other.line)
            && self.decorations == other.decorations
            && self.origin == other.origin
            && self.image_bounds == other.image_bounds
            && self.height == other.height
            && self.scale == other.scale
    }

    /// Compose on a background executor, checking cancellation between glyph
    /// batches. This deliberately uses grayscale coverage instead of LCD masks.
    pub fn prepare(
        &self,
        text_system: &TextSystem,
        cancel: &AtomicBool,
    ) -> Result<Option<PreparedLineImage>> {
        let width = (f32::from(self.image_bounds.size.width) * self.scale).round() as usize;
        let height = (f32::from(self.image_bounds.size.height) * self.scale).round() as usize;
        if width == 0 || height == 0 || cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let mut canvas = Canvas {
            width,
            height,
            pixels: vec![[0.; 4]; width * height],
            origin: self.image_bounds.origin,
            scale: self.scale,
        };
        let platform = text_system
            .platform_text_system
            .independent_rasterizer()
            .unwrap_or_else(|| text_system.platform_text_system.clone());
        let baseline = (self.height - self.line.layout.ascent - self.line.layout.descent) / 2.
            + self.line.layout.ascent;
        // Resolve decorations once in the same glyph order as normal painting.
        // Partial-glyph style runs are skipped, including inside a huge cluster.
        let mut segments = Vec::new();
        let mut style_ix = 0;
        let mut run_end = 0;
        let mut active: Option<(Pixels, DecorationRun)> = None;
        for (ix, glyph) in self
            .line
            .layout
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .enumerate()
        {
            if ix % 512 == 0 && cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if glyph.index >= run_end {
                if let Some((start, style)) = active.take() {
                    segments.push((start, glyph.position.x, style));
                }
                while let Some(style) = self.decorations.get(style_ix) {
                    style_ix += 1;
                    run_end += style.len as usize;
                    if glyph.index < run_end {
                        active = Some((glyph.position.x, style.clone()));
                        break;
                    }
                }
            }
        }
        if let Some((start, style)) = active {
            segments.push((start, self.line.layout.width, style));
        }
        for (ix, (start, end, style)) in segments.iter().enumerate() {
            if ix % 128 == 0 && cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if let Some(color) = style.background_color {
                let start = if start == end {
                    *start - self.line.max_glyph_width / 2.
                } else {
                    *start
                };
                canvas.rect(
                    self.origin + point(start, px(0.)),
                    size(*end - start, self.height),
                    color,
                );
            }
        }

        let mut cache: HashMap<RenderGlyphParams, (Bounds<DevicePixels>, Vec<u8>)> = HashMap::new();
        let mut run_end = 0;
        let mut style_ix = 0;
        let mut color = crate::black();
        let mut last: Option<(RenderGlyphParams, Point<i32>, Hsla, bool)> = None;
        let mut count = 0;
        let mut previous_x = px(0.);
        let mut glyph_x = self.origin.x;
        for run in &self.line.layout.runs {
            for glyph in &run.glyphs {
                if count % 512 == 0 && cancel.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                count += 1;
                glyph_x += glyph.position.x - previous_x;
                previous_x = glyph.position.x;
                if glyph.index >= run_end {
                    while let Some(style) = self.decorations.get(style_ix) {
                        style_ix += 1;
                        run_end += style.len as usize;
                        if glyph.index < run_end {
                            color = style.color;
                            break;
                        }
                    }
                }
                let x = f32::from(glyph_x) * self.scale;
                let y = f32::from(self.origin.y + baseline + glyph.position.y) * self.scale;
                let quantize = |n: f32, variants: u8| {
                    let n = n * variants as f32;
                    n.signum() * (n.abs() - 0.5).ceil() / variants as f32
                };
                let (x, y) = if glyph.is_emoji {
                    (quantize(x, 1), quantize(y, 1))
                } else {
                    (
                        quantize(x, crate::SUBPIXEL_VARIANTS_X),
                        quantize(y, crate::SUBPIXEL_VARIANTS_Y),
                    )
                };
                let position = point(x.trunc() as i32, y.trunc() as i32);
                let params = RenderGlyphParams {
                    font_id: run.font_id,
                    glyph_id: glyph.id,
                    font_size: self.line.layout.font_size,
                    scale_factor: self.scale,
                    is_emoji: glyph.is_emoji,
                    subpixel_variant: if glyph.is_emoji {
                        Default::default()
                    } else {
                        point(
                            (x.fract() * crate::SUBPIXEL_VARIANTS_X as f32) as u8,
                            (y.fract() * crate::SUBPIXEL_VARIANTS_Y as f32) as u8,
                        )
                    },
                    subpixel_rendering: false,
                    dilation: if glyph.is_emoji {
                        0
                    } else {
                        platform.glyph_dilation_for_color(color)
                    },
                };
                // Once identical consecutive coverage no longer changes the
                // floating-point target, later copies are exactly redundant.
                if last.as_ref().is_some_and(|(p, at, c, changed)| {
                    p == &params && *at == position && *c == color && !changed
                }) {
                    continue;
                }
                if !cache.contains_key(&params) {
                    let bounds = platform.glyph_raster_bounds(&params)?;
                    let bytes = if bounds.size.width.0 > 0 && bounds.size.height.0 > 0 {
                        platform.rasterize_glyph(&params, bounds)?.1
                    } else {
                        Vec::new()
                    };
                    cache.insert(params.clone(), (bounds, bytes));
                }
                let (bounds, bytes) = &cache[&params];
                let left = position.x + bounds.origin.x.0
                    - (f32::from(canvas.origin.x) * self.scale).round() as i32;
                let top = position.y + bounds.origin.y.0
                    - (f32::from(canvas.origin.y) * self.scale).round() as i32;
                let rgb = color.to_rgb();
                let mut changed = false;
                if left >= canvas.width as i32
                    || top >= canvas.height as i32
                    || left + bounds.size.width.0 <= 0
                    || top + bounds.size.height.0 <= 0
                {
                    last = Some((params, position, color, false));
                    continue;
                }
                for gy in 0..bounds.size.height.0 {
                    let dy = top + gy;
                    if dy < 0 || dy as usize >= canvas.height {
                        continue;
                    }
                    for gx in 0..bounds.size.width.0 {
                        let dx = left + gx;
                        if dx < 0 || dx as usize >= canvas.width {
                            continue;
                        }
                        let ix = (gy * bounds.size.width.0 + gx) as usize;
                        let source = if glyph.is_emoji {
                            [
                                bytes[ix * 4 + 2] as f32 / 255.,
                                bytes[ix * 4 + 1] as f32 / 255.,
                                bytes[ix * 4] as f32 / 255.,
                                bytes[ix * 4 + 3] as f32 / 255.,
                            ]
                        } else {
                            [rgb.r, rgb.g, rgb.b, rgb.a * bytes[ix] as f32 / 255.]
                        };
                        changed |= canvas.over(dx as usize, dy as usize, source);
                    }
                }
                last = Some((params, position, color, changed));
            }
        }
        for (ix, (start, end, style)) in segments.into_iter().enumerate() {
            if ix % 128 == 0 && cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let start = if start == end {
                start - self.line.max_glyph_width / 2.
            } else {
                start
            };
            if let Some(underline) = style.underline {
                let y = baseline + self.line.layout.descent / 2.;
                canvas.underline(
                    self.origin + point(start, y),
                    end - start,
                    underline.thickness,
                    underline.color.unwrap_or(style.color),
                    underline.wavy,
                );
            }
            if let Some(strike) = style.strikethrough {
                let y = ((self.line.layout.ascent * 0.5) + baseline) * 0.5;
                canvas.rect(
                    self.origin + point(start, y),
                    size(end - start, strike.thickness),
                    strike.color.unwrap_or(style.color),
                );
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let mut bytes = Vec::with_capacity(width * height * 4);
        for pixel in canvas.pixels {
            let a = pixel[3];
            let component = |c: f32| {
                if a > 0. {
                    (c / a * 255.).round().clamp(0., 255.) as u8
                } else {
                    0
                }
            };
            bytes.extend([
                component(pixel[2]),
                component(pixel[1]),
                component(pixel[0]),
                (a * 255.).round().clamp(0., 255.) as u8,
            ]);
        }
        let image = image::RgbaImage::from_raw(width as u32, height as u32, bytes)
            .expect("raster dimensions");
        Ok(Some(PreparedLineImage {
            bounds: self.image_bounds,
            image: Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
                image
            )])),
        }))
    }
}

struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 4]>,
    origin: Point<Pixels>,
    scale: f32,
}

impl Canvas {
    fn over(&mut self, x: usize, y: usize, source: [f32; 4]) -> bool {
        let target = &mut self.pixels[y * self.width + x];
        let old = *target;
        for channel in 0..3 {
            target[channel] = source[channel] * source[3] + target[channel] * (1. - source[3]);
        }
        target[3] = source[3] + target[3] * (1. - source[3]);
        *target != old
    }
    fn rect(&mut self, origin: Point<Pixels>, extent: crate::Size<Pixels>, color: Hsla) {
        let left = f32::from(origin.x - self.origin.x) * self.scale;
        let top = f32::from(origin.y - self.origin.y) * self.scale;
        let right = left + f32::from(extent.width) * self.scale;
        let bottom = top + f32::from(extent.height) * self.scale;
        let color = color.to_rgb();
        for y in top.floor().max(0.) as usize..(bottom.ceil().max(0.) as usize).min(self.height) {
            for x in left.floor().max(0.) as usize..(right.ceil().max(0.) as usize).min(self.width)
            {
                let coverage = ((x + 1) as f32).min(right) - (x as f32).max(left);
                let coverage = coverage * (((y + 1) as f32).min(bottom) - (y as f32).max(top));
                self.over(
                    x,
                    y,
                    [color.r, color.g, color.b, color.a * coverage.clamp(0., 1.)],
                );
            }
        }
    }
    fn underline(
        &mut self,
        origin: Point<Pixels>,
        width: Pixels,
        thickness: Pixels,
        color: Hsla,
        wavy: bool,
    ) {
        if !wavy {
            self.rect(origin, size(width, thickness), color);
            return;
        }
        let period = (f32::from(thickness) * 6.).max(1.);
        let start = (f32::from(self.origin.x - origin.x) * self.scale)
            .floor()
            .max(0.) as usize;
        let end = (f32::from(self.origin.x - origin.x) * self.scale + self.width as f32)
            .ceil()
            .max(0.) as usize;
        let count = (f32::from(width) * self.scale).ceil().max(0.) as usize;
        for ix in start..end.min(count) {
            let x = ix as f32 / self.scale;
            let y = (x / period * std::f32::consts::TAU).sin() * f32::from(thickness);
            self.rect(
                origin + point(px(x), px(y)),
                size(px(1. / self.scale), thickness),
                color,
            );
        }
    }
}
