// SPDX-License-Identifier: CC0-1.0

//! The SDF glyph protobuf (`.pbf`) `GetGlyphs` serves.
//!
//! The messages below are the `glyphs.proto` schema a MapLibre/Mapbox glyph
//! range uses, transcribed as `prost` messages: the protobuf wire codec is
//! `prost`'s, so a range this service serves decodes in any glyph reader.
//!
//! The bitmaps are signed distance fields this module computes. The emulator
//! has no font files, so the shape every glyph carries is the "missing glyph"
//! box a renderer draws for a codepoint its font does not cover — a real glyph
//! shape, measured with the requested font stack's stored metrics, rather than
//! bytes dressed up as a rasterized typeface.

use super::FontRecord;

/// `message glyph`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Glyph {
    #[prost(uint32, tag = "1")]
    pub id: u32,
    /// The SDF bitmap: `(width + 2 * buffer) * (height + 2 * buffer)` bytes.
    #[prost(bytes = "vec", optional, tag = "2")]
    pub bitmap: Option<Vec<u8>>,
    #[prost(uint32, tag = "3")]
    pub width: u32,
    #[prost(uint32, tag = "4")]
    pub height: u32,
    #[prost(sint32, tag = "5")]
    pub left: i32,
    #[prost(sint32, tag = "6")]
    pub top: i32,
    #[prost(uint32, tag = "7")]
    pub advance: u32,
}

/// `message fontstack`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Fontstack {
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(string, tag = "2")]
    pub range: String,
    #[prost(message, repeated, tag = "3")]
    pub glyphs: Vec<Glyph>,
}

/// `message glyphs`, the top-level message of a `.pbf` range.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Glyphs {
    #[prost(message, repeated, tag = "1")]
    pub stacks: Vec<Fontstack>,
}

/// The widest codepoint span one range may carry.
///
/// A MapLibre glyph range is 256 codepoints wide (`0-255`, `256-511`, ...), and
/// a style's `glyphs` template only ever asks for one of those. The modeled
/// `FontUnicodeRange` pattern (`^[0-9]+-[0-9]+\.pbf$`) places no bound on the
/// numbers, so the span is clamped here rather than letting one request ask for
/// every codepoint at once.
pub const MAX_RANGE_WIDTH: u32 = 256;

/// The highest Unicode codepoint a glyph range covers.
pub const MAX_CODEPOINT: u32 = 0xFFFF;

/// Build the glyph range `start..=end` for `stack`, using `font`'s metrics.
pub fn glyph_range(font: &FontRecord, stack: &str, start: u32, end: u32) -> Glyphs {
    let last = end
        .min(start.saturating_add(MAX_RANGE_WIDTH - 1))
        .min(MAX_CODEPOINT);
    let mut glyphs = Vec::new();
    for codepoint in start..=last.max(start) {
        if codepoint > MAX_CODEPOINT || !is_renderable(codepoint) {
            continue;
        }
        glyphs.push(glyph(font, codepoint));
    }
    Glyphs {
        stacks: vec![Fontstack {
            name: stack.to_string(),
            range: format!("{start}-{end}"),
            glyphs,
        }],
    }
}

/// Whether a codepoint gets a glyph: control characters and the surrogate
/// range are not renderable scalar values, so a range that spans them simply
/// carries no glyph for them — which is what a real glyph range does.
fn is_renderable(codepoint: u32) -> bool {
    codepoint >= 0x20 && !(0xD800..=0xDFFF).contains(&codepoint) && codepoint != 0x7F
}

/// The box one glyph of `font` occupies, in the font's own pixels.
///
/// The ONE place a glyph's measurements are decided. `GetGlyphs` reports them
/// as the glyph range's metrics, and the static-map renderer measures the
/// labels it draws with the same boxes (see [`super::labels`]), so a label on a
/// rendered image is as wide as a client laying the same text out from the
/// served glyph range would compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphBox {
    /// The em size the box is measured in, clamped from the font record.
    pub em: u32,
    pub width: u32,
    pub height: u32,
    /// How far the pen moves after drawing this glyph.
    pub advance: u32,
}

/// Measure one glyph of `font`.
pub fn glyph_box(font: &FontRecord, codepoint: u32) -> GlyphBox {
    // The catalog supplies these, but clamp them anyway: the bitmap is sized
    // from them, so a record with an absurd em or advance would size an
    // allocation. 64px is far above the 24px em an SDF glyph pipeline uses.
    let em = font.em.clamp(4, 64);
    let height = (em * 2 / 3).max(2);
    // Three widths per font, so a range carries narrow, normal and wide glyphs
    // the way a real stack does, all scaled by the stack's own advance.
    let width = (font.advance.clamp(4, 64) * (2 + codepoint % 3) / 3).max(2);
    GlyphBox {
        em,
        width,
        height,
        advance: width + 1,
    }
}

/// One glyph: its metrics, derived from the font record, and its SDF bitmap.
fn glyph(font: &FontRecord, codepoint: u32) -> Glyph {
    let metrics = glyph_box(font, codepoint);
    let buffer = font.buffer.min(8);
    Glyph {
        id: codepoint,
        bitmap: Some(sdf_box(metrics.width, metrics.height, buffer)),
        width: metrics.width,
        height: metrics.height,
        left: 0,
        top: -font.ascender,
        advance: metrics.advance,
    }
}

/// The SDF bitmap of a "missing glyph" box of `width` x `height`, with
/// `buffer` pixels of padding on every side.
///
/// The value mapping is the one MapLibre's TinySDF defines:
/// `alpha = 255 - 255 * (distance / 8 + 0.25)`, with `distance` the signed
/// distance in pixels (positive outside the shape), so the shape edge lands on
/// ~191 and a renderer's 0.75 alpha cutoff falls exactly on it.
fn sdf_box(width: u32, height: u32, buffer: u32) -> Vec<u8> {
    let stroke = 1.5_f64;
    let bitmap_width = width + buffer * 2;
    let bitmap_height = height + buffer * 2;
    let mut bitmap = Vec::with_capacity(bitmap_width as usize * bitmap_height as usize);
    for row in 0..bitmap_height {
        for column in 0..bitmap_width {
            // Pixel centre, in glyph-box coordinates.
            let px = f64::from(column) - f64::from(buffer) + 0.5;
            let py = f64::from(row) - f64::from(buffer) + 0.5;
            // Signed distance to the box, then to its outline of width
            // `stroke`: the outline is where the box distance is within half a
            // stroke of zero.
            let to_outline = rect_distance(px, py, f64::from(width), f64::from(height)).abs()
                - stroke / 2.0;
            let alpha = 255.0 - 255.0 * (to_outline / 8.0 + 0.25);
            bitmap.push(alpha.clamp(0.0, 255.0).round() as u8);
        }
    }
    bitmap
}

/// Signed distance from `(px, py)` to the axis-aligned rectangle
/// `[0, width] x [0, height]`: negative inside, positive outside.
fn rect_distance(px: f64, py: f64, width: f64, height: f64) -> f64 {
    let dx = (-px).max(px - width);
    let dy = (-py).max(py - height);
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    outside + dx.max(dy).min(0.0)
}
