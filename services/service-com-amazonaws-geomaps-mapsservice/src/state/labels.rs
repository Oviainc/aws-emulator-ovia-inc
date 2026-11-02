// SPDX-License-Identifier: CC0-1.0

//! The label text a rendered map image carries, and the language it is written
//! in.
//!
//! Two things live here, and nothing else:
//!
//! * **Which language a label is written in.** `Language` is a BCP 47 tag
//!   "limited to ISO 639-1 two-letter language codes", and its member
//!   documentation lists the codes the service has label data for
//!   (`smithy_query(query="doc", shape="GetStaticMapRequest", member="Language")`).
//!   A tag outside that set falls back to the catalog's primary label language,
//!   which is what the same documentation says happens ("If the specified
//!   language data isn't available for the map image, the labels will default
//!   to the regional primary language").
//! * **How a label is drawn.** The emulator ships no licensed map typeface —
//!   see [`super::glyph_pbf`], whose glyph ranges are rasterized from the
//!   embedded public-domain 8x8 typeface, with the missing-glyph box standing
//!   in for a codepoint that typeface does not cover. Those cells ARE this
//!   service's typeface, so a label on a rendered image is drawn with the same
//!   per-codepoint glyphs [`super::glyph_pbf::glyph_shape`] reports to a client
//!   through `GetGlyphs`, at the stack's own advances. A label's ink therefore
//!   depends on its text, which is what makes two languages two images.
//!
//! The text itself is derived, not stored: the emulator has no gazetteer, so a
//! place's name comes from its position the way its elevation does
//! ([`super::assets::elevation_at`]), mixed with the language code and written
//! with the letters of that language's own script. Deriving it keeps the two
//! properties the rest of this module tree depends on — a name is a pure
//! function of (position, language), so a repeat request is byte-identical and
//! carries the same `ETag`.

use super::assets::Position;
use super::canvas::Canvas;
use super::glyph_pbf::{glyph_box, glyph_shape, GlyphShape};
use super::{FontRecord, Rgba};

/// Every language code `GetStaticMapRequest$Language` documents as supported.
///
/// Source: that member's own documentation ("Supported codes: ar as az be bg bn
/// bs ca cs cy da de el en es et eu fi fo fr ga gl gn gu he hi hr hu hy id is
/// it ja ka kk km kn ko ky lt lv mk ml mr ms mt my nl no or pa pl pt ro ru sk
/// sl sq sr sv ta te th tr uk uz vi zh"), read with
/// `smithy_query(query="doc", shape="GetStaticMapRequest", member="Language")`.
/// The catalog seeds its label languages from this list, so the set a request is
/// resolved against is the modeled one.
pub const SUPPORTED_LANGUAGES: &[&str] = &[
    "ar", "as", "az", "be", "bg", "bn", "bs", "ca", "cs", "cy", "da", "de", "el", "en", "es", "et",
    "eu", "fi", "fo", "fr", "ga", "gl", "gn", "gu", "he", "hi", "hr", "hu", "hy", "id", "is", "it",
    "ja", "ka", "kk", "km", "kn", "ko", "ky", "lt", "lv", "mk", "ml", "mr", "ms", "mt", "my", "nl",
    "no", "or", "pa", "pl", "pt", "ro", "ru", "sk", "sl", "sq", "sr", "sv", "ta", "te", "th", "tr",
    "uk", "uz", "vi", "zh",
];

/// The language labels are written in when the request names none, or names one
/// the catalog has no data for: the "regional primary language" the `Language`
/// documentation falls back to. This catalog publishes one primary language for
/// the whole world.
pub const PRIMARY_LANGUAGE: &str = "en";

/// The size, in style pixels, a place label is set at.
///
/// The same size the style document this service serves declares for its
/// `places` symbol layer (`"text-size": 12` in
/// [`super::assets::style_descriptor`]), so a static map's labels are the size
/// a client rendering the same style would draw them.
pub const LABEL_TEXT_SIZE: u32 = 12;

/// The shortest and longest derived place name, in characters.
pub const MIN_NAME_LETTERS: usize = 4;
pub const MAX_NAME_LETTERS: usize = 8;

/// The primary language subtag of a BCP 47 tag, lowercased.
///
/// `Language` takes a BCP 47 tag ("limited to ISO 639-1 two-letter language
/// codes"), so `en-GB` and `EN` both select the label data for `en`: a tag's
/// script, region and variant subtags narrow the same language.
pub fn primary_subtag(tag: &str) -> String {
    tag.split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_lowercase()
}

/// A script's letter repertoire: the span of codepoints this service writes a
/// label with when the label's language is written in that script.
///
/// Each span is a contiguous run of letters of the named Unicode block, as the
/// Unicode Character Database assigns them. Writing a name out of the script's
/// own letters is what makes a `ja` label visibly not an `en` one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Script {
    pub name: &'static str,
    /// First and last codepoint of the letter span, inclusive.
    pub first: u32,
    pub last: u32,
}

/// Latin: Basic Latin's lowercase letters, `a`-`z`.
pub const LATIN: Script = Script {
    name: "Latin",
    first: 0x0061,
    last: 0x007A,
};
/// Cyrillic: the lowercase letters `а`-`я`.
pub const CYRILLIC: Script = Script {
    name: "Cyrillic",
    first: 0x0430,
    last: 0x044F,
};
/// Greek: the lowercase letters `α`-`ω`.
pub const GREEK: Script = Script {
    name: "Greek",
    first: 0x03B1,
    last: 0x03C9,
};
/// Arabic: the letters `ء`-`غ`.
pub const ARABIC: Script = Script {
    name: "Arabic",
    first: 0x0621,
    last: 0x063A,
};
/// Hebrew: the letters `א`-`ת`.
pub const HEBREW: Script = Script {
    name: "Hebrew",
    first: 0x05D0,
    last: 0x05EA,
};
/// Armenian: the lowercase letters `ա`-`ֆ`.
pub const ARMENIAN: Script = Script {
    name: "Armenian",
    first: 0x0561,
    last: 0x0586,
};
/// Georgian: the Mkhedruli letters `ა`-`ჰ`.
pub const GEORGIAN: Script = Script {
    name: "Georgian",
    first: 0x10D0,
    last: 0x10F0,
};
/// Devanagari: the consonants `क`-`ह`.
pub const DEVANAGARI: Script = Script {
    name: "Devanagari",
    first: 0x0915,
    last: 0x0939,
};
/// Bengali (Bengali-Assamese): the consonants `ক`-`ম`.
pub const BENGALI: Script = Script {
    name: "Bengali",
    first: 0x0995,
    last: 0x09AE,
};
/// Gurmukhi: the consonants `ਕ`-`ਮ`.
pub const GURMUKHI: Script = Script {
    name: "Gurmukhi",
    first: 0x0A15,
    last: 0x0A2E,
};
/// Gujarati: the consonants `ક`-`મ`.
pub const GUJARATI: Script = Script {
    name: "Gujarati",
    first: 0x0A95,
    last: 0x0AAE,
};
/// Oriya: the consonants `କ`-`ମ`.
pub const ORIYA: Script = Script {
    name: "Oriya",
    first: 0x0B15,
    last: 0x0B2E,
};
/// Tamil: the consonants `ம`-`ஹ`.
pub const TAMIL: Script = Script {
    name: "Tamil",
    first: 0x0BAE,
    last: 0x0BB9,
};
/// Telugu: the consonants `క`-`న`.
pub const TELUGU: Script = Script {
    name: "Telugu",
    first: 0x0C15,
    last: 0x0C28,
};
/// Kannada: the consonants `ಕ`-`ನ`.
pub const KANNADA: Script = Script {
    name: "Kannada",
    first: 0x0C95,
    last: 0x0CA8,
};
/// Malayalam: the consonants `ക`-`ന`.
pub const MALAYALAM: Script = Script {
    name: "Malayalam",
    first: 0x0D15,
    last: 0x0D28,
};
/// Thai: the consonants `ก`-`ฮ`.
pub const THAI: Script = Script {
    name: "Thai",
    first: 0x0E01,
    last: 0x0E2E,
};
/// Myanmar: the consonants `က`-`အ`.
pub const MYANMAR: Script = Script {
    name: "Myanmar",
    first: 0x1000,
    last: 0x1021,
};
/// Khmer: the consonants `ក`-`អ`.
pub const KHMER: Script = Script {
    name: "Khmer",
    first: 0x1780,
    last: 0x17A2,
};
/// Hiragana: the syllables `ぁ`-`ゖ`.
pub const HIRAGANA: Script = Script {
    name: "Hiragana",
    first: 0x3041,
    last: 0x3096,
};
/// Hangul: the precomposed syllables `가`-`힣`.
pub const HANGUL: Script = Script {
    name: "Hangul",
    first: 0xAC00,
    last: 0xD7A3,
};
/// Han: the CJK Unified Ideographs block, `一`-`龥`.
pub const HAN: Script = Script {
    name: "Han",
    first: 0x4E00,
    last: 0x9FA5,
};

/// The script a language's labels are written in.
///
/// One entry per supported code whose writing system is not Latin; every other
/// code in [`SUPPORTED_LANGUAGES`] is written in Latin script. The pairing is
/// the language's own writing system (Japanese in kana, Greek in Greek letters,
/// Hindi and Marathi in Devanagari, Serbian in Cyrillic, and so on), not a
/// property of the request.
pub fn script_of(language: &str) -> Script {
    match language {
        "ar" => ARABIC,
        "as" | "bn" => BENGALI,
        "be" | "bg" | "kk" | "ky" | "mk" | "ru" | "sr" | "uk" => CYRILLIC,
        "el" => GREEK,
        "gu" => GUJARATI,
        "he" => HEBREW,
        "hi" | "mr" => DEVANAGARI,
        "hy" => ARMENIAN,
        "ja" => HIRAGANA,
        "ka" => GEORGIAN,
        "km" => KHMER,
        "kn" => KANNADA,
        "ko" => HANGUL,
        "ml" => MALAYALAM,
        "my" => MYANMAR,
        "or" => ORIYA,
        "pa" => GURMUKHI,
        "ta" => TAMIL,
        "te" => TELUGU,
        "th" => THAI,
        "zh" => HAN,
        _ => LATIN,
    }
}

/// The name the place at `position` carries in `language`.
///
/// Derived, because the emulator stores no gazetteer: the position decides
/// WHICH name (so the same place keeps its name across requests and across
/// framings) and the language decides the name's letters and its script (so
/// asking for another language asks for another label). A pure function of its
/// two arguments, with no clock and no randomness in it.
pub fn place_name(position: Position, language: &str) -> String {
    let script = script_of(language);
    let span = u64::from(script.last - script.first) + 1;
    let mut state = seed(position, language);
    let letters = MIN_NAME_LETTERS + (state as usize) % (MAX_NAME_LETTERS - MIN_NAME_LETTERS + 1);
    let mut name = String::with_capacity(letters * 4);
    for _ in 0..letters {
        state = next(state);
        // The high bits of an LCG state are the well-mixed ones.
        let offset = (state >> 33) % span;
        if let Some(letter) = char::from_u32(script.first + offset as u32) {
            name.push(letter);
        }
    }
    name
}

/// The pixel extent `text` occupies when set at `size` pixels with `font`'s
/// metrics: `(width, height)`, the box a label needs on the image.
pub fn label_extent(font: &FontRecord, text: &str, size: u32) -> (u32, u32) {
    let mut width = 0_u32;
    let mut height = 0_u32;
    for character in text.chars() {
        let glyph = scaled_box(font, character, size);
        width = width.saturating_add(glyph.advance);
        height = height.max(glyph.height);
    }
    (width, height)
}

/// Draw `text` on `canvas`, the pen starting on the baseline at `anchor`.
///
/// One glyph of this service's typeface per character — the same shapes its
/// glyph ranges carry — inked at `font`'s stored metrics scaled to `size`
/// pixels. The canvas clips, so a label that runs off the image is drawn as far
/// as the image goes; leaving such a label out entirely is the caller's
/// decision (`CropLabels`).
pub fn draw_label(
    canvas: &mut Canvas,
    font: &FontRecord,
    text: &str,
    size: u32,
    anchor: (i64, i64),
    color: Rgba,
) {
    let mut pen = anchor.0;
    for character in text.chars() {
        let glyph = scaled_box(font, character, size);
        draw_glyph(canvas, glyph_shape(character as u32), pen, anchor.1, &glyph, color);
        pen = pen.saturating_add(i64::from(glyph.advance));
    }
}

/// One glyph's box, scaled from the font's em to a label set at `size` pixels.
struct ScaledBox {
    width: u32,
    height: u32,
    advance: u32,
    /// The left side bearing: how far right of the pen the ink starts.
    left: i64,
    /// How far the ink's top sits above the baseline, in label pixels.
    above_baseline: i64,
}

/// Measure one character at label size: the glyph box `GetGlyphs` reports for
/// that codepoint, scaled from the font's em down to `size`.
fn scaled_box(font: &FontRecord, character: char, size: u32) -> ScaledBox {
    let metrics = glyph_box(font, character as u32);
    let em = metrics.em.max(1);
    let scale = |value: u32| (value.saturating_mul(size) / em).max(1);
    let bearing = |value: i32| i64::from(value).saturating_mul(i64::from(size)) / i64::from(em);
    ScaledBox {
        width: scale(metrics.width),
        height: scale(metrics.height),
        advance: scale(metrics.advance),
        left: bearing(metrics.left),
        // `top` is negative above the baseline, the way the glyph protobuf
        // writes it.
        above_baseline: bearing(metrics.top).saturating_neg(),
    }
}

/// Draw one glyph's ink: the cell's coverage, stretched across the rectangle
/// its metrics report, with the top of that rectangle its own height above the
/// baseline at `baseline`.
fn draw_glyph(
    canvas: &mut Canvas,
    shape: GlyphShape,
    x: i64,
    baseline: i64,
    glyph: &ScaledBox,
    color: Rgba,
) {
    // The ink starts at the pen plus the glyph's own side bearing, the way the
    // metrics `GetGlyphs` reports place it.
    let left = x.saturating_add(glyph.left);
    let top = baseline.saturating_sub(glyph.above_baseline);
    // Only the part of the rectangle that lands on the image is walked: the
    // canvas clips every write anyway, so a glyph set far larger than the image
    // would otherwise be iterated with nothing to show for it.
    let columns = on_canvas(left, glyph.width, canvas.width());
    let rows = on_canvas(top, glyph.height, canvas.height());
    for row in rows {
        for column in columns.clone() {
            if shape.inked_at(column, row, glyph.width, glyph.height) {
                canvas.put(left + i64::from(column), top + i64::from(row), color);
            }
        }
    }
}

/// The columns (or rows) of a glyph's rectangle that land inside an image
/// `limit` pixels across, when the rectangle starts at `start` and is `extent`
/// pixels wide.
fn on_canvas(start: i64, extent: u32, limit: u32) -> std::ops::Range<u32> {
    let extent = i64::from(extent);
    let first = start.saturating_neg().clamp(0, extent) as u32;
    let last = i64::from(limit).saturating_sub(start).clamp(0, extent) as u32;
    first..last.max(first)
}

/// The seed a place's name is drawn from: its position, quantized to the
/// micro-degree, mixed with the language code (FNV-1a).
fn seed(position: Position, language: &str) -> u64 {
    let hash = fnv(FNV_OFFSET_BASIS, &quantize(position.0).to_le_bytes());
    let hash = fnv(hash, &quantize(position.1).to_le_bytes());
    fnv(hash, language.as_bytes())
}

/// A coordinate as the integer number of micro-degrees it is drawn at: two
/// positions closer together than a micro-degree are the same place.
fn quantize(value: f64) -> i64 {
    if value.is_nan() {
        return 0;
    }
    (value * 1e6).round().clamp(-1e15, 1e15) as i64
}

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash = (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
    }
    hash
}

/// The next state of the name generator: a 64-bit linear congruential step
/// (the multiplier and increment Knuth's MMIX uses).
fn next(state: u64) -> u64 {
    state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
}
