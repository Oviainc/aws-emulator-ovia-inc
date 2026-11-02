// SPDX-License-Identifier: CC0-1.0

//! The SDF glyph protobuf (`.pbf`) `GetGlyphs` serves.
//!
//! The messages below are the `glyphs.proto` schema a MapLibre/Mapbox glyph
//! range uses, transcribed as `prost` messages: the protobuf wire codec is
//! `prost`'s, so a range this service serves decodes in any glyph reader.
//!
//! The SHAPES come from a typeface, not from a generator: `font8x8` is an
//! embedded public-domain 8x8 bitmap font (Basic Latin, Latin-1, Greek,
//! Hiragana, box and block drawing), and each glyph of a range is that
//! typeface's own cell for the codepoint, measured and rasterized at the
//! requested font stack's stored metrics. A codepoint the typeface does not
//! cover carries the missing-glyph box a renderer draws for an uncovered
//! codepoint, which is what a real glyph range does for a codepoint its face
//! has no outline for.
//!
//! The bitmaps are signed distance fields computed from that cell's coverage:
//! the distance is to the boundary of the inked region, so a reader's alpha
//! cutoff lands on the glyph's real edge.

use std::collections::BTreeMap;

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

/// The side of the embedded typeface's design cell, in design pixels.
pub const DESIGN_CELL: u32 = 8;

/// How many of a cell's rows sit above the baseline: the last row is the
/// descender row, so rows `0..=6` are the ascending part of the design.
const DESIGN_BASELINE_ROWS: u32 = DESIGN_CELL - 1;

/// Build the glyph range `start..=end` for `stack`, using `font`'s metrics.
///
/// Two codepoints the typeface draws with the same cell — every codepoint it
/// does not cover shares the missing-glyph box — have the same bitmap at the
/// same metrics, so a range rasterizes each distinct cell once.
pub fn glyph_range(font: &FontRecord, stack: &str, start: u32, end: u32) -> Glyphs {
    let last = end
        .min(start.saturating_add(MAX_RANGE_WIDTH - 1))
        .min(MAX_CODEPOINT);
    let mut glyphs = Vec::new();
    let mut rasterized: BTreeMap<[u8; DESIGN_CELL as usize], Vec<u8>> = BTreeMap::new();
    for codepoint in start..=last.max(start) {
        if codepoint > MAX_CODEPOINT || !is_renderable(codepoint) {
            continue;
        }
        glyphs.push(glyph(font, codepoint, &mut rasterized));
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

/// The inked part of a design cell: the first and last column and row that
/// carry ink. A cell with no ink at all (the space) has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InkBox {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

impl InkBox {
    fn columns(&self) -> u32 {
        self.x1 - self.x0 + 1
    }

    fn rows(&self) -> u32 {
        self.y1 - self.y0 + 1
    }
}

/// One glyph's shape on the typeface's own design grid.
///
/// The single source of this service's typeface: `GetGlyphs` rasterizes it into
/// an SDF bitmap and the static-map renderer draws its coverage directly (see
/// [`super::labels`]), so a label on a rendered image is drawn with the same
/// glyphs a client lays text out with from the served range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphShape {
    /// One byte per design row, bit 0 the leftmost pixel.
    rows: [u8; DESIGN_CELL as usize],
    ink: Option<InkBox>,
}

/// The missing-glyph box: the rectangle a renderer draws for a codepoint its
/// face has no outline for, sitting on the baseline (so the descender row is
/// clear) and one design pixel thick.
const MISSING_GLYPH_ROWS: [u8; DESIGN_CELL as usize] =
    [0xFF, 0x81, 0x81, 0x81, 0x81, 0x81, 0xFF, 0x00];

impl GlyphShape {
    fn new(rows: [u8; DESIGN_CELL as usize]) -> Self {
        let mut ink: Option<InkBox> = None;
        for (y, row) in rows.iter().enumerate() {
            for x in 0..DESIGN_CELL {
                if row & (1 << x) == 0 {
                    continue;
                }
                let (x, y) = (x, y as u32);
                ink = Some(match ink {
                    None => InkBox {
                        x0: x,
                        y0: y,
                        x1: x,
                        y1: y,
                    },
                    Some(box_) => InkBox {
                        x0: box_.x0.min(x),
                        y0: box_.y0.min(y),
                        x1: box_.x1.max(x),
                        y1: box_.y1.max(y),
                    },
                });
            }
        }
        Self { rows, ink }
    }

    /// Whether the design pixel `(x, y)` of the cell carries ink.
    fn lit(&self, x: i32, y: i32) -> bool {
        let cell = i32::try_from(DESIGN_CELL).unwrap_or(i32::MAX);
        if x < 0 || y < 0 || x >= cell || y >= cell {
            return false;
        }
        self.rows[y as usize] & (1 << x) != 0
    }

    /// Whether the pixel `(column, row)` of this glyph drawn into a
    /// `width` x `height` box carries ink.
    ///
    /// The box is the glyph's INK box — the rectangle `GetGlyphs` reports as the
    /// glyph's `width` and `height` — so the design cell's inked columns and
    /// rows are stretched across exactly that rectangle.
    pub fn inked_at(&self, column: u32, row: u32, width: u32, height: u32) -> bool {
        let Some(ink) = self.ink else {
            return false;
        };
        if width == 0 || height == 0 || column >= width || row >= height {
            return false;
        }
        let x = ink.x0 + (u64::from(column) * u64::from(ink.columns()) / u64::from(width)) as u32;
        let y = ink.y0 + (u64::from(row) * u64::from(ink.rows()) / u64::from(height)) as u32;
        self.lit(x as i32, y as i32)
    }
}

/// The typeface's shape for `codepoint`.
///
/// `font8x8` carries one cell per covered codepoint; a codepoint outside its
/// blocks gets the missing-glyph box, because this operation serves a glyph for
/// every renderable codepoint of the requested range.
pub fn glyph_shape(codepoint: u32) -> GlyphShape {
    let cell = char::from_u32(codepoint).and_then(typeface_cell);
    GlyphShape::new(cell.unwrap_or(MISSING_GLYPH_ROWS))
}

/// The embedded typeface's cell for `character`, or `None` where no block of it
/// covers the character.
///
/// `font8x8` splits its tables by Unicode block and each table knows its own
/// codepoints, so the blocks are consulted in turn rather than indexed by an
/// offset computed here.
fn typeface_cell(character: char) -> Option<[u8; DESIGN_CELL as usize]> {
    use font8x8::UnicodeFonts as _;

    font8x8::BASIC_FONTS
        .get(character)
        .or_else(|| font8x8::LATIN_FONTS.get(character))
        .or_else(|| font8x8::GREEK_FONTS.get(character))
        .or_else(|| font8x8::HIRAGANA_FONTS.get(character))
        .or_else(|| font8x8::BOX_FONTS.get(character))
        .or_else(|| font8x8::BLOCK_FONTS.get(character))
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
    /// The left side bearing: how far right of the pen the ink starts.
    pub left: i32,
    /// How far the top of the ink sits above the baseline, as the glyph
    /// protobuf writes it (negative above the baseline).
    pub top: i32,
    /// How far the pen moves after drawing this glyph.
    pub advance: u32,
}

/// Measure one glyph of `font`.
pub fn glyph_box(font: &FontRecord, codepoint: u32) -> GlyphBox {
    measure(font, &glyph_shape(codepoint))
}

/// A face's metrics as this module uses them: the catalog's values, clamped,
/// plus how many pixels one design pixel is worth horizontally (the em across
/// the cell's columns) and vertically (the ascender across the rows above the
/// baseline).
///
/// Clamping happens HERE and nowhere else: the bitmap is sized from these, so a
/// catalog record with an absurd em or ascender would size an allocation. 64px
/// is far above the 24px em an SDF glyph pipeline uses.
struct Face {
    em: u32,
    ascender: i32,
    advance: u32,
    horizontal: f64,
    vertical: f64,
}

fn face(font: &FontRecord) -> Face {
    let em = font.em.clamp(4, 64);
    let ascender = font.ascender.clamp(2, 64);
    Face {
        em,
        ascender,
        advance: font.advance.clamp(2, 64),
        horizontal: f64::from(em) / f64::from(DESIGN_CELL),
        vertical: f64::from(ascender) / f64::from(DESIGN_BASELINE_ROWS),
    }
}

/// Measure `shape` at `font`'s metrics.
fn measure(font: &FontRecord, shape: &GlyphShape) -> GlyphBox {
    let face = face(font);
    let Some(ink) = shape.ink else {
        // A cell with no ink is a blank glyph — the space — which carries no
        // bitmap rectangle at all and only moves the pen.
        return GlyphBox {
            em: face.em,
            width: 0,
            height: 0,
            left: 0,
            top: 0,
            advance: face.advance,
        };
    };
    let round = |value: f64| value.round().clamp(0.0, f64::from(u32::MAX)) as u32;
    GlyphBox {
        em: face.em,
        width: round(f64::from(ink.columns()) * face.horizontal).max(1),
        height: round(f64::from(ink.rows()) * face.vertical).max(1),
        left: round(f64::from(ink.x0) * face.horizontal) as i32,
        // The cell's top row sits the stack's ascender above the baseline, so a
        // glyph whose ink starts lower starts that much further down.
        top: round(f64::from(ink.y0) * face.vertical) as i32 - face.ascender,
        advance: face.advance,
    }
}

/// One glyph: its metrics, measured from the typeface's cell at the stack's own
/// scale, and the SDF bitmap of that cell's coverage.
fn glyph(
    font: &FontRecord,
    codepoint: u32,
    rasterized: &mut BTreeMap<[u8; DESIGN_CELL as usize], Vec<u8>>,
) -> Glyph {
    let shape = glyph_shape(codepoint);
    let metrics = measure(font, &shape);
    let buffer = font.buffer.min(8);
    let bitmap = rasterized
        .entry(shape.rows)
        .or_insert_with(|| sdf(&shape, &metrics, buffer))
        .clone();
    Glyph {
        id: codepoint,
        bitmap: Some(bitmap),
        width: metrics.width,
        height: metrics.height,
        left: metrics.left,
        top: metrics.top,
        advance: metrics.advance,
    }
}

/// How the signed distance at a pixel becomes that pixel's byte.
///
/// The value mapping is the one MapLibre's TinySDF defines:
/// `alpha = 255 - 255 * (distance / 8 + 0.25)`, with `distance` the signed
/// distance in pixels (positive outside the shape), so the shape edge lands on
/// ~191 and a renderer's 0.75 alpha cutoff falls exactly on it.
fn sdf_byte(distance: f64) -> u8 {
    let alpha = 255.0 - 255.0 * (distance / 8.0 + 0.25);
    alpha.clamp(0.0, 255.0).round() as u8
}

/// The glyph-pixel distance beyond which [`sdf_byte`] has already saturated to
/// 0, so nothing further away changes a byte of the bitmap.
///
/// `alpha` reaches 0 where `distance / 8 + 0.25` reaches 1.
const SDF_RANGE_PX: f64 = 6.0;

/// The SDF bitmap of one glyph: `(width + 2 * buffer) * (height + 2 * buffer)`
/// bytes, each the signed distance from that pixel's centre to the boundary of
/// the glyph's inked region.
fn sdf(shape: &GlyphShape, metrics: &GlyphBox, buffer: u32) -> Vec<u8> {
    let bitmap_width = metrics.width + buffer * 2;
    let bitmap_height = metrics.height + buffer * 2;
    let mut bitmap = Vec::with_capacity(bitmap_width as usize * bitmap_height as usize);
    let Some(ink) = shape.ink else {
        // No ink: every pixel is infinitely far outside the shape.
        let pixels = bitmap_width as usize * bitmap_height as usize;
        bitmap.resize(pixels, sdf_byte(f64::INFINITY));
        return bitmap;
    };
    // The glyph box holds the cell's inked columns and rows, so a point of the
    // box maps back onto the design grid by the same two scales the metrics
    // were measured with.
    let horizontal = f64::from(metrics.width) / f64::from(ink.columns());
    let vertical = f64::from(metrics.height) / f64::from(ink.rows());
    // Distances are reported in glyph pixels, the unit the SDF value mapping is
    // defined in; the design grid is not square once the face's ascender
    // differs from its em, so one design pixel is worth the smaller of the two
    // scales. Nothing further than the saturation distance is searched, and
    // never more than the whole cell plus its clear surround.
    let scale = horizontal.min(vertical).max(f64::MIN_POSITIVE);
    let radius = ((SDF_RANGE_PX / scale).ceil() as i32)
        .saturating_add(1)
        .clamp(1, DESIGN_CELL as i32 + 1);
    for row in 0..bitmap_height {
        for column in 0..bitmap_width {
            // The pixel's centre, in design pixels from the cell's origin.
            let x = (f64::from(column) - f64::from(buffer) + 0.5) / horizontal + f64::from(ink.x0);
            let y = (f64::from(row) - f64::from(buffer) + 0.5) / vertical + f64::from(ink.y0);
            bitmap.push(sdf_byte(design_distance(shape, x, y, radius) * scale));
        }
    }
    bitmap
}

/// The signed distance, in design pixels, from `(x, y)` to the boundary of the
/// region the cell inks: negative inside it, positive outside.
///
/// The inked region is the union of the unit squares of the lit design pixels.
/// Outside it, the distance to that union is the distance to the nearest lit
/// square; inside it, the distance to the boundary is the distance to the
/// nearest square that is NOT lit, which is why both are searched. Every lit
/// square is inside the cell and everything past the cell's one-square border
/// is clear and further away, so the search never leaves that border.
fn design_distance(shape: &GlyphShape, x: f64, y: f64, radius: i32) -> f64 {
    let mut nearest_lit = f64::INFINITY;
    let mut nearest_clear = f64::INFINITY;
    let mut inside = false;
    let (cx, cy) = (x.floor() as i32, y.floor() as i32);
    let border = DESIGN_CELL as i32;
    for grid_y in (cy - radius).max(-1)..=(cy + radius).min(border) {
        for grid_x in (cx - radius).max(-1)..=(cx + radius).min(border) {
            let distance = square_distance(x, y, grid_x, grid_y);
            if shape.lit(grid_x, grid_y) {
                inside |= distance == 0.0;
                nearest_lit = nearest_lit.min(distance);
            } else {
                nearest_clear = nearest_clear.min(distance);
            }
        }
    }
    if inside {
        -nearest_clear
    } else {
        nearest_lit
    }
}

/// The distance from `(x, y)` to the unit square whose top-left corner is the
/// design pixel `(grid_x, grid_y)`; zero when the point is inside it.
fn square_distance(x: f64, y: f64, grid_x: i32, grid_y: i32) -> f64 {
    let left = f64::from(grid_x);
    let top = f64::from(grid_y);
    let dx = (left - x).max(x - (left + 1.0)).max(0.0);
    let dy = (top - y).max(y - (top + 1.0)).max(0.0);
    (dx * dx + dy * dy).sqrt()
}
