// SPDX-License-Identifier: CC0-1.0

//! `GetGlyphs` serves the typeface's own glyph set for the requested font stack
//! and unicode range.
//!
//! The rule these cases pin, over a set of ranges and stacks rather than one
//! recorded request: a glyph range carries one glyph per renderable codepoint of
//! the requested range; each glyph is the embedded typeface's own cell for that
//! codepoint, rasterized as a signed distance field of that cell's coverage and
//! measured at the requested stack's stored metrics; and a codepoint the
//! typeface does not cover carries the missing-glyph box a renderer draws for an
//! uncovered codepoint. So the body is a function of the stack and of the range:
//! a range the typeface covers carries a distinct shape per distinct cell, and
//! one it does not cover carries the one box.
//!
//! The typeface is read here straight from `font8x8`, so what the served bitmap
//! is checked against is the font data and not the handler that drew it.

use std::collections::{BTreeMap, BTreeSet};

use font8x8::UnicodeFonts as _;
use prost::Message as _;
use service_com_amazonaws_geomaps_mapsservice::state::glyph_pbf::{Glyph, Glyphs, DESIGN_CELL};
use service_com_amazonaws_geomaps_mapsservice::state::FontRecord;

use super::map_assets::{assert_ok, Emulator};

/// The font stack the `Standard` style publishes.
const STACK: &str = "Amazon Ember Regular";

/// The SDF byte the shape's own edge lands on, which is the value a renderer
/// cuts icons and glyphs out at: a sample above it is inside the ink, one below
/// it is outside.
///
/// The mapping is MapLibre's TinySDF one, `255 - 255 * (distance / 8 + 0.25)`,
/// so zero distance is `255 * 0.75`.
const SDF_EDGE: u8 = 191;

/// The ranges the cases below span, each 256-aligned the way the modeled
/// `FontUnicodeRange` requires.
///
/// Rows: the Basic Latin and Latin-1 range (covered, with an uncovered pocket at
/// U+0080..U+009F), the range holding Greek (partly covered), the box and block
/// drawing range (covered), the range holding Hiragana (partly covered), and two
/// ranges the typeface does not cover at all.
const RANGES: [(u32, u32); 6] = [
    (0, 255),
    (768, 1023),
    (9472, 9727),
    (12288, 12543),
    (3072, 3327),
    (65280, 65535),
];

/// The embedded typeface's cell for `codepoint`, or `None` where no block of it
/// covers the codepoint.
fn cell(codepoint: u32) -> Option<[u8; 8]> {
    let character = char::from_u32(codepoint)?;
    font8x8::BASIC_FONTS
        .get(character)
        .or_else(|| font8x8::LATIN_FONTS.get(character))
        .or_else(|| font8x8::GREEK_FONTS.get(character))
        .or_else(|| font8x8::HIRAGANA_FONTS.get(character))
        .or_else(|| font8x8::BOX_FONTS.get(character))
        .or_else(|| font8x8::BLOCK_FONTS.get(character))
}

/// The first and last inked column and row of a cell, or `None` for a cell with
/// no ink at all (the space).
fn ink_box(rows: &[u8; 8]) -> Option<(u32, u32, u32, u32)> {
    let mut found: Option<(u32, u32, u32, u32)> = None;
    for (y, row) in rows.iter().enumerate() {
        for x in 0..DESIGN_CELL {
            if row & (1 << x) == 0 {
                continue;
            }
            let (x, y) = (x, y as u32);
            found = Some(match found {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
    }
    found
}

/// Whether a codepoint is a renderable scalar value: a control character and a
/// surrogate are not, so a range spanning them carries no glyph for them.
fn is_renderable(codepoint: u32) -> bool {
    codepoint >= 0x20 && codepoint != 0x7F && !(0xD800..=0xDFFF).contains(&codepoint)
}

/// The glyphs of one range, keyed by codepoint.
async fn glyphs_of(emulator: &Emulator, stack: &str, start: u32, end: u32) -> BTreeMap<u32, Glyph> {
    let case = format!("{stack} {start}-{end}");
    let response = emulator
        .get(&format!(
            "/v2/glyphs/{}/{start}-{end}.pbf",
            stack.replace(' ', "%20")
        ))
        .await;
    assert_ok(&case, &response);
    let decoded = Glyphs::decode(response.body().as_ref())
        .unwrap_or_else(|error| panic!("case {case:?}: must decode as a glyph range, got {error}"));
    assert_eq!(
        decoded.stacks.len(),
        1,
        "case {case:?}: a range carries exactly one font stack"
    );
    let fontstack = decoded.stacks.into_iter().next().expect("one font stack");
    assert_eq!(
        fontstack.name, stack,
        "case {case:?}: the stack must be the one requested"
    );
    fontstack
        .glyphs
        .into_iter()
        .map(|glyph| (glyph.id, glyph))
        .collect()
}

/// The buffer, in pixels, a stack's glyph bitmaps keep on every side, read from
/// the catalog rather than written out here.
fn buffer_of(emulator: &Emulator, stack: &str) -> u32 {
    let catalog = emulator.catalog();
    let catalog = catalog.lock();
    catalog
        .fonts
        .get(stack)
        .unwrap_or(&catalog.fallback_font)
        .buffer
}

/// One bitmap sample: the SDF byte at the centre of the glyph pixel that holds
/// the fraction `(fx, fy)` of the glyph's rectangle.
fn sample(glyph: &Glyph, buffer: u32, fx: f64, fy: f64) -> u8 {
    let bitmap = glyph.bitmap.as_ref().expect("a glyph carries a bitmap");
    let column = ((fx * f64::from(glyph.width)).floor() as u32).min(glyph.width.saturating_sub(1));
    let row = ((fy * f64::from(glyph.height)).floor() as u32).min(glyph.height.saturating_sub(1));
    let stride = glyph.width + buffer * 2;
    bitmap[((row + buffer) * stride + column + buffer) as usize]
}

/// Every glyph of a covered range is the typeface's own cell for its codepoint:
/// the SDF is inked exactly where that cell is, and the rectangle it is drawn in
/// is the cell's inked columns and rows.
#[tokio::test]
async fn a_served_glyph_is_the_typefaces_own_cell_for_its_codepoint() {
    let emulator = Emulator::new();
    let buffer = buffer_of(&emulator, STACK);
    let mut checked = 0_usize;

    for (start, end) in RANGES {
        let glyphs = glyphs_of(&emulator, STACK, start, end).await;
        for codepoint in start..=end {
            let case = format!("{STACK} {start}-{end} U+{codepoint:04X}");
            if !is_renderable(codepoint) {
                assert!(
                    !glyphs.contains_key(&codepoint),
                    "case {case:?}: a codepoint that is not a renderable scalar value \
                     must carry no glyph, got {:?}",
                    glyphs.get(&codepoint).map(|glyph| glyph.width)
                );
                continue;
            }
            let glyph = glyphs.get(&codepoint).unwrap_or_else(|| {
                panic!("case {case:?}: a renderable codepoint of the range must carry a glyph")
            });
            let bitmap = glyph
                .bitmap
                .as_ref()
                .unwrap_or_else(|| panic!("case {case:?}: a glyph must carry its bitmap"));
            assert_eq!(
                bitmap.len(),
                (glyph.width as usize + buffer as usize * 2)
                    * (glyph.height as usize + buffer as usize * 2),
                "case {case:?}: the bitmap must be the glyph's rectangle plus its buffer, \
                 got {} bytes for {}x{}",
                bitmap.len(),
                glyph.width,
                glyph.height
            );

            let Some(rows) = cell(codepoint) else {
                // An uncovered codepoint is the missing-glyph box, which the
                // next case pins.
                continue;
            };
            let Some((x0, y0, x1, y1)) = ink_box(&rows) else {
                assert_eq!(
                    (glyph.width, glyph.height),
                    (0, 0),
                    "case {case:?}: a cell with no ink is a blank glyph, got {}x{}",
                    glyph.width,
                    glyph.height
                );
                continue;
            };
            assert!(
                glyph.width > 0 && glyph.height > 0,
                "case {case:?}: a cell with ink must be drawn in a rectangle, got {}x{}",
                glyph.width,
                glyph.height
            );
            let (columns, rows_span) = (x1 - x0 + 1, y1 - y0 + 1);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let lit = rows[y as usize] & (1 << x) != 0;
                    let fx = (f64::from(x - x0) + 0.5) / f64::from(columns);
                    let fy = (f64::from(y - y0) + 0.5) / f64::from(rows_span);
                    let value = sample(glyph, buffer, fx, fy);
                    assert_eq!(
                        value > SDF_EDGE,
                        lit,
                        "case {case:?}: design pixel ({x},{y}) is {} in the typeface but the \
                         served field reads {value} there",
                        if lit { "inked" } else { "clear" }
                    );
                }
            }
            checked += 1;
        }
    }

    assert!(
        checked > 200,
        "case \"coverage\": the ranges must exercise many covered codepoints, checked {checked}"
    );
}

/// A codepoint the typeface does not cover carries the missing-glyph box: one
/// rectangle outline, the same for every uncovered codepoint, and not the shape
/// of any letter the typeface does carry.
#[tokio::test]
async fn an_uncovered_codepoint_carries_the_missing_glyph_box() {
    let emulator = Emulator::new();
    let buffer = buffer_of(&emulator, STACK);
    let mut boxes: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut uncovered = 0_usize;

    for (start, end) in RANGES {
        let glyphs = glyphs_of(&emulator, STACK, start, end).await;
        for (codepoint, glyph) in &glyphs {
            if cell(*codepoint).is_some() {
                continue;
            }
            let case = format!("{STACK} {start}-{end} U+{codepoint:04X}");
            // A rectangle outline: its corners are inked and its middle is not.
            for (fx, fy, corner) in [
                (0.0, 0.0, "top left"),
                (0.99, 0.0, "top right"),
                (0.0, 0.99, "bottom left"),
                (0.99, 0.99, "bottom right"),
            ] {
                let value = sample(glyph, buffer, fx, fy);
                assert!(
                    value > SDF_EDGE,
                    "case {case:?}: the missing-glyph box must be inked at its {corner} \
                     corner, the served field reads {value} there"
                );
            }
            let middle = sample(glyph, buffer, 0.5, 0.5);
            assert!(
                middle < SDF_EDGE,
                "case {case:?}: the missing-glyph box must be hollow, the served field \
                 reads {middle} at its centre"
            );
            boxes.insert(glyph.bitmap.clone().unwrap_or_default());
            uncovered += 1;
        }
    }

    assert!(
        uncovered > 200,
        "case \"coverage\": the ranges must exercise uncovered codepoints, saw {uncovered}"
    );
    assert_eq!(
        boxes.len(),
        1,
        "case \"one box\": every uncovered codepoint carries the same missing-glyph box, \
         got {} different ones",
        boxes.len()
    );

    // And the box is not what a covered letter looks like.
    let glyphs = glyphs_of(&emulator, STACK, 0, 255).await;
    let letter = glyphs[&u32::from('A')]
        .bitmap
        .clone()
        .expect("a glyph carries a bitmap");
    assert!(
        !boxes.contains(&letter),
        "case \"box vs letter\": a covered codepoint must not be drawn as the \
         missing-glyph box"
    );
}

/// A cell's ink, cropped to its ink box: the shape a glyph is rasterized from,
/// with where the ink sits inside the cell taken out — a glyph reports that
/// separately, as its `left` and `top`.
fn crop(rows: &[u8; 8]) -> (u32, u32, Vec<bool>) {
    let Some((x0, y0, x1, y1)) = ink_box(rows) else {
        return (0, 0, Vec::new());
    };
    let mut ink = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            ink.push(rows[y as usize] & (1 << x) != 0);
        }
    }
    (x1 - x0 + 1, y1 - y0 + 1, ink)
}

/// The shapes a range carries are decided by the typeface's coverage of that
/// range: one distinct field per distinct cell, so a covered range carries many
/// and an uncovered one carries exactly the box.
#[tokio::test]
async fn the_shapes_a_range_carries_follow_the_typefaces_coverage() {
    let emulator = Emulator::new();
    for (start, end) in RANGES {
        let case = format!("{STACK} {start}-{end}");
        let glyphs = glyphs_of(&emulator, STACK, start, end).await;
        // Over the codepoints the typeface covers: one distance field per
        // distinct cropped cell, which is exactly what the typeface itself
        // holds for the range.
        let served: BTreeSet<Vec<u8>> = glyphs
            .iter()
            .filter(|(codepoint, _)| cell(**codepoint).is_some())
            .map(|(_, glyph)| glyph.bitmap.clone().unwrap_or_default())
            .collect();
        let mut cells: BTreeSet<(u32, u32, Vec<bool>)> = BTreeSet::new();
        for codepoint in start..=end {
            if let Some(rows) = cell(codepoint).filter(|_| is_renderable(codepoint)) {
                cells.insert(crop(&rows));
            }
        }
        assert_eq!(
            served.len(),
            cells.len(),
            "case {case:?}: the range must carry one distance field per distinct cell \
             the typeface holds for it, got {} fields for {} cells",
            served.len(),
            cells.len()
        );
    }

    // The two ends of that rule, stated directly: a range the typeface covers
    // carries many shapes, a range it does not cover carries one.
    let covered = glyphs_of(&emulator, STACK, 0, 255).await;
    let distinct = |glyphs: &BTreeMap<u32, Glyph>| {
        glyphs
            .values()
            .map(|glyph| glyph.bitmap.clone().unwrap_or_default())
            .collect::<BTreeSet<_>>()
            .len()
    };
    assert!(
        distinct(&covered) > 80,
        "case \"0-255\": a covered range carries a shape per letter, got {}",
        distinct(&covered)
    );
    let unknown = glyphs_of(&emulator, STACK, 3072, 3327).await;
    assert_eq!(
        distinct(&unknown),
        1,
        "case \"3072-3327\": a range the typeface does not cover carries only the \
         missing-glyph box, got {}",
        distinct(&unknown)
    );
}

/// A glyph is measured at the requested stack's own metrics: the pen moves by
/// that stack's advance, the ink is as wide as its em and as tall as its
/// ascender, and the bitmap keeps its buffer.
#[tokio::test]
async fn glyph_metrics_come_from_the_requested_stacks_record() {
    let emulator = Emulator::new();
    // Rows: an em sweep, an ascender sweep and an advance sweep, so each metric
    // is varied on its own. The names are the stacks the requests ask for.
    let records: [(&str, u32, u32, i32, u32); 6] = [
        ("Metrics Small", 8, 12, 14, 2),
        ("Metrics Medium", 24, 18, 20, 3),
        ("Metrics Large", 48, 30, 42, 4),
        ("Metrics Tall", 24, 18, 42, 3),
        ("Metrics Wide Advance", 24, 30, 20, 3),
        ("Metrics Narrow Advance", 24, 6, 20, 3),
    ];
    {
        let catalog = emulator.catalog();
        let mut catalog = catalog.lock();
        for (name, em, advance, ascender, buffer) in records {
            catalog.fonts.insert(
                name.to_string(),
                FontRecord {
                    name: name.to_string(),
                    em,
                    buffer,
                    advance,
                    ascender,
                    cache_control: "max-age=86400".to_string(),
                },
            );
        }
    }

    // Codepoints from three blocks the typeface covers, each with a different
    // ink box: a capital, a descender, a digit, a Greek letter and a kana.
    let codepoints = ['A', 'g', '0', 'α', 'あ'].map(u32::from);
    let mut widths: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut heights: BTreeMap<u32, Vec<u32>> = BTreeMap::new();

    for (name, em, advance, ascender, buffer) in records {
        let glyphs = glyphs_of(&emulator, name, 0, 255).await;
        let greek = glyphs_of(&emulator, name, 768, 1023).await;
        let kana = glyphs_of(&emulator, name, 12288, 12543).await;
        for codepoint in codepoints {
            let case = format!("{name} U+{codepoint:04X}");
            let glyph = greek
                .get(&codepoint)
                .or_else(|| kana.get(&codepoint))
                .or_else(|| glyphs.get(&codepoint))
                .unwrap_or_else(|| panic!("case {case:?}: the range must carry the glyph"));
            assert_eq!(
                glyph.advance, advance,
                "case {case:?}: the pen must move by the stack's own advance, got {}",
                glyph.advance
            );
            let bitmap = glyph.bitmap.as_ref().expect("a glyph carries a bitmap");
            assert_eq!(
                bitmap.len(),
                (glyph.width as usize + buffer as usize * 2)
                    * (glyph.height as usize + buffer as usize * 2),
                "case {case:?}: the bitmap must keep the stack's buffer on every side"
            );
            assert!(
                glyph.width <= em && glyph.width > 0,
                "case {case:?}: the ink must fit the stack's em of {em}, got {}",
                glyph.width
            );
            assert!(
                glyph.height <= ascender as u32 && glyph.height > 0,
                "case {case:?}: the ink must fit the stack's ascender of {ascender}, got {}",
                glyph.height
            );
            assert!(
                glyph.top < 0 && glyph.top >= -ascender,
                "case {case:?}: the ink must start within the stack's ascender of \
                 {ascender} above the baseline, got {}",
                glyph.top
            );
            widths.entry(codepoint).or_default().push(glyph.width);
            heights.entry(codepoint).or_default().push(glyph.height);
        }
    }

    // The em sweep (rows 0..3) widens every glyph and the ascender sweep
    // (rows 1 and 3, same em) heightens it, so neither dimension is a constant.
    for codepoint in codepoints {
        let case = format!("U+{codepoint:04X}");
        let width = &widths[&codepoint];
        assert!(
            width[0] < width[1] && width[1] < width[2],
            "case {case:?}: a larger em must draw wider ink, got {width:?}"
        );
        let height = &heights[&codepoint];
        assert!(
            height[1] < height[3],
            "case {case:?}: a larger ascender must draw taller ink, got {height:?}"
        );
    }
}
