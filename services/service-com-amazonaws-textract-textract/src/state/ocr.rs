// SPDX-License-Identifier: CC0-1.0

//! Reading the text out of a RASTER document: the pixel side of
//! [`super::read_content`].
//!
//! Textract accepts a document "in PNG, JPEG, PDF, or TIFF format"; a raster in one of
//! those formats carries its text as ink rather than as characters, so it has to be
//! recognised. Nothing here is echoed from the request, keyed to a digest or
//! randomised: every line, word and character this module reports was measured off the
//! submitted pixels, and ink it cannot match to a glyph is reported as nothing at all.
//!
//! ## The pipeline
//!
//! 1. **Decode** — the `image` crate decodes the submitted bytes for the format its
//!    magic bytes name, bounded by [`MAX_RASTER_SIDE`], [`MAX_RASTER_PIXELS`] and
//!    [`MAX_RASTER_ALLOC`] so one request cannot turn a small compressed file into
//!    unbounded work.
//! 2. **Binarise** — Otsu's method (N. Otsu, 1979: the threshold that maximises the
//!    between-class variance of the grey histogram) splits the page into ink and
//!    background. Ink is the SMALLER of the two classes, so a light page with dark text
//!    and a dark page with light text are both read; two classes closer together than
//!    [`MIN_INK_CONTRAST`] are sensor or compression noise on a blank page, not ink.
//! 3. **Segment** — a horizontal projection profile of the ink gives the page's text
//!    lines; a vertical profile within one line gives its glyph candidates, and the
//!    gaps between them, clustered against the glyphs' own median width, give the word
//!    and column breaks ([`breaks`]).
//! 4. **Recognise** — each candidate is resampled onto an 8x8 grid and scored against
//!    every printable ASCII glyph of the embedded public-domain 8x8 font (`font8x8`) by
//!    shape overlap (the Sørensen-Dice coefficient) plus the glyph's height, width and
//!    position inside its line. The best match above [`MIN_GLYPH_SCORE`] is the
//!    character; the mean winning score is the `Confidence` the blocks report, because
//!    it is the only confidence anything here actually measured — per word, per line and
//!    per page, each over the characters that block was read from.
//! 5. **Measure** — every line and every word also reports WHERE its ink was found, as
//!    the fraction of the decoded page it covers ([`MeasuredLine`], [`MeasuredWord`]), so
//!    the `Geometry` the blocks carry is the position and the size the recogniser
//!    measured rather than a typographic grid.
//!
//! No maintained Rust crate performs offline OCR: the ones that exist either bind to a
//! C library or load neural-network weights from a file this emulator may not fetch, so
//! the recogniser itself is in-crate while the decoding (a format job) is the `image`
//! crate's.
//!
//! ## What it does not read
//!
//! One raster is ONE page. A multi-frame TIFF's later frames are not reported, because
//! the decoder hands this module a single image and a page nothing decoded is not a page
//! this service can describe; `DocumentMetadata.Pages` then says 1, which is what was
//! read. A PDF is not a raster and is not decoded here at all.
//!
//! ## Where it runs
//!
//! Every step here is bounded, but all of it is CPU-bound over a page whose size the
//! caller chose, so a request path reaches it through [`super::read_document`], which
//! runs it on tokio's BLOCKING pool: it occupies no async worker, and the
//! `spawn_blocking(..).await` that drives it is a real await point a timeout, a
//! cancellation or a shutdown can land on.

use std::io::Cursor;
use std::sync::OnceLock;

use image::{ImageFormat, Limits};

use super::Unreadable;

/// The largest side, in pixels, of a raster this service decodes. NOT an AWS limit: an
/// emulator-side bound so one request cannot allocate without end.
pub const MAX_RASTER_SIDE: u32 = 10_000;

/// The most pixels one raster page may hold. NOT an AWS limit: see [`MAX_RASTER_SIDE`].
/// The scan is linear in the pixel count, so this bounds the per-request CPU too.
pub const MAX_RASTER_PIXELS: u64 = 8_000_000;

/// The most bytes the decoder may allocate for one raster. NOT an AWS limit: it is what
/// stops a small compressed file from expanding into a large allocation.
pub const MAX_RASTER_ALLOC: u64 = 96 * 1024 * 1024;

/// The most glyph candidates one page may hold. NOT an AWS limit: recognition costs one
/// comparison per candidate per template, so this bounds the work a single request can
/// cause.
pub const MAX_GLYPHS_PER_PAGE: usize = 10_000;

/// How far apart the two Otsu classes must be, in grey levels, for the darker one to be
/// ink. Below this the page holds one tone plus noise, which is a blank page.
const MIN_INK_CONTRAST: f32 = 24.0;

/// The score a glyph candidate must reach for its best-matching character to be
/// reported. Ink below it matched nothing, so it contributes no text.
const MIN_GLYPH_SCORE: f32 = 0.40;

/// How much of a candidate's ink must be SHARED with the template that matched it, as
/// the Sørensen-Dice coefficient of the two shapes.
///
/// A floor on the shape term alone, not just on the weighted total: the size and
/// position terms of [`score`] can carry up to 0.30 between them, so without this a mark
/// whose shape is nothing like any glyph could still clear [`MIN_GLYPH_SCORE`] and be
/// reported as a character. With it, ink that matches no glyph's shape contributes no
/// text — which is what this module promises.
const MIN_SHAPE_OVERLAP: f32 = 0.45;

/// Rows a run of inked rows must span to be a line of text rather than speckle.
const MIN_LINE_HEIGHT: usize = 2;

/// A run no taller than this that inks more than half the page's width is a rule or a
/// border, not a line of text.
const RULE_MAX_HEIGHT: usize = 3;

/// The most indent characters a line's left margin is reported as, so a page-wide
/// margin cannot stretch one line into a long string.
const MAX_INDENT: usize = 100;

/// A raster page's recognised text.
#[derive(Debug, Clone, PartialEq)]
pub struct Recognised {
    /// The page's text, one line per line of ink the recogniser read.
    pub text: String,
    /// The mean score of the characters recognised, as a percentage inside `Percent`'s
    /// `0..=100` range. Zero when the page carries no recognised text.
    pub confidence: f32,
    /// WHERE the recognised text sits on the page, one entry per line of `text` and in
    /// the same order, each with the words of that line in reading order.
    ///
    /// Every rectangle here was MEASURED off the submitted pixels — a word is the extent
    /// of the glyph run the line's text separates with a space or a tab, and a line the
    /// box enclosing its words — so the geometry the blocks report describes the document
    /// rather than a page template.
    pub lines: Vec<MeasuredLine>,
}

/// Where something sits on a page, in the page-relative fractions `BoundingBox` and
/// `Point` are expressed in: `0.0` is the page's left or top edge and `1.0` its right or
/// bottom edge.
#[derive(Debug, Clone, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    /// The whole page: what was examined when there is no ink to bound.
    pub const fn whole_page() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            width: 1.0,
            height: 1.0,
        }
    }
}

/// One word of a recognised line: where its ink sits, and the mean score its own
/// characters were recognised with.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredWord {
    pub rect: Rect,
    pub confidence: f32,
}

/// One recognised line: the mean score its characters were recognised with, and its
/// words — each with its own ink — in reading order. The line's own extent is the box
/// enclosing its words, which is where the block layer takes it from.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredLine {
    pub confidence: f32,
    pub words: Vec<MeasuredWord>,
}

/// A mean score as the percentage `Confidence` reports, inside `Percent`'s `0..=100`
/// range. Zero when nothing was recognised, because nothing was measured.
fn percentage(score_total: f32, score_count: usize) -> f32 {
    if score_count == 0 {
        0.0
    } else {
        (score_total / score_count as f32 * 100.0).clamp(0.0, 100.0)
    }
}

/// `Some(format)` when these bytes are a raster document format Textract reads on the
/// channel they were submitted on.
///
/// The format comes from the bytes' own magic numbers, never from a declared name. Which
/// formats a channel accepts is AWS's own split (see [`super::DocumentChannel`]): PNG and
/// JPEG are read whichever way the document arrived, while a TIFF is a document only an
/// Amazon S3 object may hold — `Document$Bytes` takes PNG and JPEG alone.
pub fn raster_format(bytes: &[u8], channel: super::DocumentChannel) -> Option<ImageFormat> {
    match image::guess_format(bytes) {
        Ok(format @ (ImageFormat::Png | ImageFormat::Jpeg)) => Some(format),
        Ok(ImageFormat::Tiff) if channel == super::DocumentChannel::S3Object => {
            Some(ImageFormat::Tiff)
        }
        _ => None,
    }
}

/// The name of the raster format these bytes DECLARE, whatever it is: `image`'s own
/// sniffing of the magic number at their head.
///
/// `None` says the bytes declare no raster format the sniffer knows — not that they are a
/// document. It is what lets [`super::read_content`] tell a document in a raster format
/// the service does not process from bytes that are not a raster at all.
pub fn declared_raster_format(bytes: &[u8]) -> Option<&'static str> {
    image::guess_format(bytes).ok().map(format_name)
}

/// Read the text of one raster document.
///
/// `Err` says why the document could not be read, with the variant the handlers answer
/// with the matching modelled error.
pub fn read_raster(bytes: &[u8], format: ImageFormat) -> Result<Recognised, Unreadable> {
    let name = format_name(format);
    // The header is read with the decoder's own size limits OFF, so a page over this
    // emulator's ceiling is refused as too large (what it is) rather than as unreadable.
    let (width, height) = reader(bytes, format, false)
        .into_dimensions()
        .map_err(|error| undecodable(name, &error))?;
    let pixels = u64::from(width) * u64::from(height);
    if width > MAX_RASTER_SIDE || height > MAX_RASTER_SIDE || pixels > MAX_RASTER_PIXELS {
        return Err(Unreadable::TooLarge(format!(
            "the document is {width}x{height} pixels; this emulator reads at most \
             {MAX_RASTER_SIDE} pixels on a side and {MAX_RASTER_PIXELS} pixels in all"
        )));
    }

    let decoded = reader(bytes, format, true)
        .decode()
        .map_err(|error| undecodable(name, &error))?;
    let gray = decoded.into_luma8();
    // The ceilings again, on the frame that was actually decoded: the scan, the
    // segmentation and the ink mask are all sized from THIS image, so the budget must not
    // depend on the header having agreed with it.
    if gray.width() > MAX_RASTER_SIDE
        || gray.height() > MAX_RASTER_SIDE
        || u64::from(gray.width()) * u64::from(gray.height()) > MAX_RASTER_PIXELS
    {
        return Err(Unreadable::TooLarge(format!(
            "the document decodes to {}x{} pixels; this emulator reads at most \
             {MAX_RASTER_SIDE} pixels on a side and {MAX_RASTER_PIXELS} pixels in all",
            gray.width(),
            gray.height()
        )));
    }
    let ink = Ink::of(&gray);
    let lines = ink.text_lines()?;

    let mut text = String::new();
    let mut measured: Vec<MeasuredLine> = Vec::new();
    let mut score_total = 0.0f32;
    let mut score_count = 0usize;
    for line in &lines {
        let Some(rendered) = line.render(&ink) else {
            continue;
        };
        text.push_str(&rendered.text);
        text.push('\n');
        score_total += rendered.score_total;
        score_count += rendered.score_count;
        // Measured in the frame of the image that was DECODED, which is the page the ink
        // was read off, so one entry per line of `text` in the same order.
        measured.push(rendered.measured(ink.width, ink.height));
    }

    let confidence = percentage(score_total, score_count);
    Ok(Recognised {
        text,
        confidence,
        lines: measured,
    })
}

/// The refusal for a document in an accepted container whose bytes the decoder cannot
/// read (a truncated or otherwise undecodable file).
///
/// Such a document is one no page can be read out of, which is what
/// [`Unreadable::UnsupportedFormat`] says, and that refusal carries no reason of its own:
/// what the decoder tripped over is a detail of this emulator's reader, so it is logged
/// here instead of being answered to the caller.
fn undecodable(name: &str, error: &image::ImageError) -> Unreadable {
    ::tracing::debug!(
        format = name,
        error = %error,
        "the document's bytes could not be decoded"
    );
    Unreadable::UnsupportedFormat
}

fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Tiff => "TIFF",
        // Reached through `declared_raster_format`, which names whatever format the
        // sniffer recognised so a refusal can be logged with it.
        other => other.extensions_str().first().copied().unwrap_or("raster"),
    }
}

/// A decoder for `bytes`, bounded so a small file cannot decode into a large one.
///
/// `bound_dimensions` turns the decoder's own width/height ceilings on, for the decode
/// itself; the header read leaves them off so this module can tell a page that is too
/// large from one that cannot be read at all.
fn reader(
    bytes: &[u8],
    format: ImageFormat,
    bound_dimensions: bool,
) -> image::ImageReader<Cursor<&[u8]>> {
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    // `Limits::no_limits()` rather than `default()`: the bounds below are this
    // emulator's, and starting from the crate's defaults would silently inherit them.
    let mut limits = Limits::no_limits();
    if bound_dimensions {
        limits.max_image_width = Some(MAX_RASTER_SIDE);
        limits.max_image_height = Some(MAX_RASTER_SIDE);
    }
    limits.max_alloc = Some(MAX_RASTER_ALLOC);
    reader.limits(limits);
    reader
}

// ---------------------------------------------------------------------------
// Binarisation
// ---------------------------------------------------------------------------

/// A page's ink: one bit per pixel, true where the recogniser sees a mark.
struct Ink {
    width: usize,
    height: usize,
    mask: Vec<bool>,
}

impl Ink {
    /// Binarise a grey page with Otsu's method, taking the smaller class as ink.
    fn of(gray: &image::GrayImage) -> Self {
        let width = gray.width() as usize;
        let height = gray.height() as usize;
        let mut histogram = [0u64; 256];
        for pixel in gray.pixels() {
            histogram[pixel.0[0] as usize] += 1;
        }
        let total: u64 = histogram.iter().sum();
        let mask = match otsu(&histogram, total) {
            // One tone plus noise: a blank page carries no ink at all.
            None => vec![false; width * height],
            Some(split) => {
                let dark: u64 = histogram[..=split.threshold as usize].iter().sum();
                let ink_is_dark = dark * 2 <= total;
                gray.pixels()
                    .map(|pixel| {
                        let value = pixel.0[0];
                        if ink_is_dark {
                            value <= split.threshold
                        } else {
                            value > split.threshold
                        }
                    })
                    .collect()
            }
        };
        Self {
            width,
            height,
            mask,
        }
    }

    fn at(&self, x: usize, y: usize) -> bool {
        self.mask.get(y * self.width + x).copied().unwrap_or(false)
    }

    fn row_inked(&self, y: usize) -> bool {
        (0..self.width).any(|x| self.at(x, y))
    }

    /// The page's lines of text, top to bottom, with the glyph candidates of each.
    fn text_lines(&self) -> Result<Vec<InkLine>, Unreadable> {
        let mut lines = Vec::new();
        let mut glyph_budget = MAX_GLYPHS_PER_PAGE;
        let mut y = 0usize;
        while y < self.height {
            if !self.row_inked(y) {
                y += 1;
                continue;
            }
            let top = y;
            while y < self.height && self.row_inked(y) {
                y += 1;
            }
            let bottom = y - 1;
            if bottom + 1 - top < MIN_LINE_HEIGHT {
                continue;
            }
            let glyphs = self.glyphs_between(top, bottom);
            if glyphs.is_empty() {
                continue;
            }
            // A short, page-wide run of ink is a rule or a border, not text.
            let inked_width = glyphs.last().map_or(0, |g| g.x1) + 1 - glyphs[0].x0;
            if bottom + 1 - top <= RULE_MAX_HEIGHT && inked_width * 2 > self.width {
                continue;
            }
            if glyphs.len() > glyph_budget {
                return Err(Unreadable::TooLarge(format!(
                    "the document holds more than {MAX_GLYPHS_PER_PAGE} marks to recognise; \
                     this emulator reads at most that many on one page"
                )));
            }
            glyph_budget -= glyphs.len();
            lines.push(InkLine {
                top,
                bottom,
                glyphs,
            });
        }
        Ok(lines)
    }

    /// The glyph candidates of the band `top..=bottom`: the maximal runs of inked
    /// columns, left to right, each with its own ink extent.
    fn glyphs_between(&self, top: usize, bottom: usize) -> Vec<GlyphBox> {
        let inked = |x: usize| (top..=bottom).any(|y| self.at(x, y));
        let mut out = Vec::new();
        let mut x = 0usize;
        while x < self.width {
            if !inked(x) {
                x += 1;
                continue;
            }
            let x0 = x;
            while x < self.width && inked(x) {
                x += 1;
            }
            let x1 = x - 1;
            let mut y0 = bottom;
            let mut y1 = top;
            for y in top..=bottom {
                if (x0..=x1).any(|cx| self.at(cx, y)) {
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
            out.push(GlyphBox { x0, x1, y0, y1 });
        }
        out
    }
}

/// The threshold Otsu's method picks, and how far apart the classes it splits are.
struct Split {
    threshold: u8,
    separation: f32,
}

/// Otsu's threshold for a grey histogram, or `None` when the two classes it would split
/// are closer together than [`MIN_INK_CONTRAST`] — one tone plus noise, so no ink.
fn otsu(histogram: &[u64; 256], total: u64) -> Option<Split> {
    if total == 0 {
        return None;
    }
    let total = total as f64;
    let sum: f64 = histogram
        .iter()
        .enumerate()
        .map(|(value, count)| value as f64 * *count as f64)
        .sum();
    let mut below_weight = 0f64;
    let mut below_sum = 0f64;
    let mut best: Option<Split> = None;
    let mut best_variance = -1f64;
    for (value, count) in histogram.iter().enumerate().take(255) {
        below_weight += *count as f64;
        below_sum += value as f64 * *count as f64;
        let above_weight = total - below_weight;
        if below_weight == 0.0 || above_weight == 0.0 {
            continue;
        }
        let below_mean = below_sum / below_weight;
        let above_mean = (sum - below_sum) / above_weight;
        let delta = above_mean - below_mean;
        let variance = below_weight * above_weight * delta * delta;
        if variance > best_variance {
            best_variance = variance;
            best = Some(Split {
                threshold: u8::try_from(value).unwrap_or(u8::MAX),
                separation: delta as f32,
            });
        }
    }
    best.filter(|split| split.separation >= MIN_INK_CONTRAST)
}

// ---------------------------------------------------------------------------
// Segmentation
// ---------------------------------------------------------------------------

/// One candidate mark: the rectangle its ink occupies.
struct GlyphBox {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
}

impl GlyphBox {
    fn width(&self) -> usize {
        self.x1 + 1 - self.x0
    }
}

/// A line of ink on the page.
struct InkLine {
    top: usize,
    bottom: usize,
    glyphs: Vec<GlyphBox>,
}

/// One line's recognised text, and the scores it was recognised with.
struct RenderedLine {
    text: String,
    score_total: f32,
    score_count: usize,
    /// The line's words, in reading order: one per run of glyphs the text does NOT
    /// separate with a space or a tab, so `words[k]` is the k-th whitespace-separated
    /// word of `text`.
    words: Vec<WordRun>,
}

/// One word's measured ink, in page pixels, and the scores its characters matched with.
struct WordRun {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    score_total: f32,
    score_count: usize,
}

impl WordRun {
    /// A word starting at `glyph`.
    fn starting_at(glyph: &GlyphBox, score: f32) -> Self {
        Self {
            x0: glyph.x0,
            x1: glyph.x1,
            y0: glyph.y0,
            y1: glyph.y1,
            score_total: score,
            score_count: 1,
        }
    }

    /// Take `glyph` into this word.
    fn extend(&mut self, glyph: &GlyphBox, score: f32) {
        self.x0 = self.x0.min(glyph.x0);
        self.x1 = self.x1.max(glyph.x1);
        self.y0 = self.y0.min(glyph.y0);
        self.y1 = self.y1.max(glyph.y1);
        self.score_total += score;
        self.score_count += 1;
    }

    /// This word's ink as a fraction of a `width` x `height` page.
    fn rect(&self, width: usize, height: usize) -> Rect {
        let page_width = width.max(1) as f32;
        let page_height = height.max(1) as f32;
        Rect {
            left: (self.x0 as f32 / page_width).clamp(0.0, 1.0),
            top: (self.y0 as f32 / page_height).clamp(0.0, 1.0),
            width: ((self.x1 + 1 - self.x0) as f32 / page_width).clamp(0.0, 1.0),
            height: ((self.y1 + 1 - self.y0) as f32 / page_height).clamp(0.0, 1.0),
        }
    }
}

impl RenderedLine {
    /// This line's measurements, as fractions of a `width` x `height` page: the extent of
    /// each word's own ink, and the score each was recognised with.
    fn measured(&self, width: usize, height: usize) -> MeasuredLine {
        MeasuredLine {
            confidence: percentage(self.score_total, self.score_count),
            words: self
                .words
                .iter()
                .map(|run| MeasuredWord {
                    rect: run.rect(width, height),
                    confidence: percentage(run.score_total, run.score_count),
                })
                .collect(),
        }
    }
}

impl InkLine {
    fn band(&self) -> usize {
        self.bottom + 1 - self.top
    }

    /// Recognise this line: its characters, and the word and column breaks between them.
    ///
    /// `None` when nothing in the line matched a glyph, so the line carries no text.
    fn render(&self, ink: &Ink) -> Option<RenderedLine> {
        let band = self.band();
        let median_width = median(self.glyphs.iter().map(GlyphBox::width)).unwrap_or(1.0);
        let gaps: Vec<usize> = self
            .glyphs
            .windows(2)
            .map(|pair| pair[1].x0.saturating_sub(pair[0].x1 + 1))
            .collect();
        // Which gaps are word breaks, and which are COLUMN breaks, is a property of THIS
        // line's own marks: its gaps fall into clusters — the ones inside a word, the ones
        // between words and, on a table row, the ones between columns — and each boundary
        // between two clusters is a jump in the sorted gaps. See [`breaks`].
        let (word_gap, column_gap) = breaks(&gaps, median_width);
        // The character cell the line's left margin is reported in, so a line that
        // starts half way across the page is laid out half way across its text.
        let cell = median(
            self.glyphs
                .windows(2)
                .map(|pair| pair[1].x0.saturating_sub(pair[0].x0))
                .filter(|advance| (*advance as f32) <= median_width + word_gap),
        )
        .unwrap_or(median_width + word_gap)
        .max(1.0);

        let mut text = String::new();
        let mut score_total = 0.0f32;
        let mut score_count = 0usize;
        let mut words: Vec<WordRun> = Vec::new();
        let mut previous: Option<&GlyphBox> = None;
        for glyph in &self.glyphs {
            let Some((ch, score)) = recognise(ink, glyph, self.top, band) else {
                continue;
            };
            // Whether this glyph starts a new word is decided by the same gaps the text
            // is separated on, so the measured words and the words of the text are one
            // and the same list.
            let starts_word = match previous {
                None => {
                    let indent = ((glyph.x0 as f32 / cell).round() as usize).min(MAX_INDENT);
                    text.push_str(&" ".repeat(indent));
                    true
                }
                Some(before) => {
                    let gap = glyph.x0.saturating_sub(before.x1 + 1) as f32;
                    if gap > column_gap {
                        text.push('\t');
                        true
                    } else if gap > word_gap {
                        text.push(' ');
                        true
                    } else {
                        false
                    }
                }
            };
            text.push(ch);
            match words.last_mut() {
                Some(word) if !starts_word => word.extend(glyph, score),
                _ => words.push(WordRun::starting_at(glyph, score)),
            }
            score_total += score;
            score_count += 1;
            previous = Some(glyph);
        }
        if score_count == 0 {
            return None;
        }
        Some(RenderedLine {
            text,
            score_total,
            score_count,
            words,
        })
    }
}

/// The median of `values`, or `None` when there are none.
fn median(values: impl Iterator<Item = usize>) -> Option<f32> {
    let mut values: Vec<usize> = values.collect();
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[values.len() / 2] as f32)
}

/// The gap widths that separate one line's marks into WORDS and into COLUMNS.
///
/// The gaps of one line fall into clusters — the ones inside a word, the ones between
/// words and, on a table row, the ones between columns — and a boundary between two
/// clusters is a jump between consecutive sorted gaps that is wide in absolute terms,
/// against the size of the text, and a doubling of the gaps below it.
///
/// The LOWEST such boundary is what separates marks into words, so a row whose columns
/// stand far apart still has its words split on its own word gaps rather than on the
/// column gaps. The NEXT boundary above it separates those words into columns. A line
/// that shows no second boundary has no column structure of its own, and only a gap
/// several times its word gap is read as one; a line with no boundary at all is one word,
/// where only a gap wider than the glyphs themselves breaks it.
fn breaks(gaps: &[usize], median_width: f32) -> (f32, f32) {
    let mut sorted: Vec<usize> = gaps.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let boundaries: Vec<f32> = sorted
        .windows(2)
        .filter(|pair| {
            let (low, high) = (pair[0], pair[1]);
            (high - low) as f32 >= (0.6 * median_width).max(2.0) && high >= 2 * low.max(1)
        })
        .map(|pair| (pair[0] + pair[1]) as f32 / 2.0)
        .collect();
    let word_gap = boundaries
        .first()
        .copied()
        .unwrap_or(0.9 * median_width)
        .max(2.0);
    let column_gap = boundaries.get(1).copied().unwrap_or(3.0 * word_gap);
    (word_gap, column_gap)
}

// ---------------------------------------------------------------------------
// Recognition
// ---------------------------------------------------------------------------

/// The side of the grid a glyph and a template are compared on: the embedded font's own
/// cell size.
const CELL: usize = 8;

/// One candidate's measurements, in the frame of its line.
struct Candidate {
    /// Its ink, resampled over its own bounding box onto a `CELL` x `CELL` grid.
    shape: [f32; CELL * CELL],
    /// How much ink that grid holds, for the Dice denominator.
    ink: f32,
    /// Its height, width and top offset as fractions of the line's band, which is what
    /// tells 'o' from 'O' and '.' from '-'.
    height: f32,
    width: f32,
    top: f32,
}

/// One recognition template: a printable ASCII glyph of the embedded 8x8 font, measured
/// exactly as a candidate is.
struct Template {
    ch: char,
    shape: [f32; CELL * CELL],
    ink: f32,
    height: f32,
    width: f32,
    top: f32,
}

/// Every printable ASCII glyph of the embedded font, as a template.
///
/// `font8x8::legacy::BASIC_LEGACY` is the public-domain 8x8 bitmap font: one row per
/// byte, bit 0 the leftmost pixel.
fn templates() -> &'static [Template] {
    static TEMPLATES: OnceLock<Vec<Template>> = OnceLock::new();
    TEMPLATES.get_or_init(|| (b'!'..=b'~').filter_map(|code| template(code as char)).collect())
}

fn template(ch: char) -> Option<Template> {
    // `get`, not an index: the table holds 128 entries and this function's signature
    // accepts any `char`, so a non-ASCII one is no template rather than a panic inside
    // the `OnceLock` initialiser.
    let rows = *font8x8::legacy::BASIC_LEGACY.get(ch as usize)?;
    let lit = |x: usize, y: usize| rows[y] & (1 << x) != 0;
    let mut x0 = CELL;
    let mut x1 = 0usize;
    let mut y0 = CELL;
    let mut y1 = 0usize;
    for y in 0..CELL {
        for x in 0..CELL {
            if lit(x, y) {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 || y0 > y1 {
        // A glyph the font leaves blank is no template at all.
        return None;
    }
    let shape = resample(lit, x0, x1, y0, y1);
    Some(Template {
        ch,
        ink: shape.iter().sum(),
        shape,
        height: (y1 + 1 - y0) as f32 / CELL as f32,
        width: (x1 + 1 - x0) as f32 / CELL as f32,
        top: y0 as f32 / CELL as f32,
    })
}

/// The best-matching character for one candidate, and the score it matched with, or
/// `None` when nothing matched it well enough to report.
fn recognise(ink: &Ink, glyph: &GlyphBox, band_top: usize, band: usize) -> Option<(char, f32)> {
    let mask = |x: usize, y: usize| ink.at(x, y);
    let shape = resample(mask, glyph.x0, glyph.x1, glyph.y0, glyph.y1);
    let band = band as f32;
    let candidate = Candidate {
        ink: shape.iter().sum(),
        shape,
        height: ((glyph.y1 + 1 - glyph.y0) as f32 / band).clamp(0.0, 1.0),
        width: (glyph.width() as f32 / band).clamp(0.0, 1.0),
        top: (glyph.y0.saturating_sub(band_top) as f32 / band).clamp(0.0, 1.0),
    };
    let mut best: Option<Match> = None;
    for template in templates() {
        let scored = score(&candidate, template);
        if best.is_none_or(|best: Match| scored.total > best.total) {
            best = Some(Match {
                ch: template.ch,
                ..scored
            });
        }
    }
    // Both floors: the mark's shape must really be this glyph's shape, and the whole
    // measurement must hold up. Ink that clears neither matched nothing.
    best.filter(|found| found.shape_overlap >= MIN_SHAPE_OVERLAP && found.total >= MIN_GLYPH_SCORE)
        .map(|found| (found.ch, found.total))
}

/// How well one candidate matched one template.
#[derive(Debug, Clone, Copy)]
struct Match {
    ch: char,
    /// The weighted measurement, which is what the `Confidence` reports.
    total: f32,
    /// The shape term on its own, floored by [`MIN_SHAPE_OVERLAP`].
    shape_overlap: f32,
}

/// How well a candidate matches a template: mostly the overlap of the two shapes (the
/// Sørensen-Dice coefficient, which is 1 for identical ink and 0 for disjoint ink), plus
/// how closely the glyph's size and position inside its line agree with the template's
/// inside its cell.
fn score(candidate: &Candidate, template: &Template) -> Match {
    let overlap: f32 = candidate
        .shape
        .iter()
        .zip(template.shape.iter())
        .map(|(c, t)| c.min(*t))
        .sum();
    let denominator = candidate.ink + template.ink;
    let dice = if denominator > 0.0 {
        2.0 * overlap / denominator
    } else {
        0.0
    };
    let agree = |a: f32, b: f32| 1.0 - (a - b).abs().min(1.0);
    Match {
        ch: template.ch,
        total: 0.70 * dice
            + 0.12 * agree(candidate.height, template.height)
            + 0.10 * agree(candidate.width, template.width)
            + 0.08 * agree(candidate.top, template.top),
        shape_overlap: dice,
    }
}

/// Resample the ink of `x0..=x1` by `y0..=y1` onto a `CELL` x `CELL` grid of densities.
///
/// Each cell averages the source rectangle it covers, and never fewer than one pixel, so
/// the same routine both shrinks a large glyph and stretches a one-pixel-wide one.
///
/// Generic over the mask rather than taking `&dyn Fn`, so the closure inlines: this runs
/// `CELL * CELL` sampled rectangles per candidate, for up to [`MAX_GLYPHS_PER_PAGE`]
/// candidates on a page.
fn resample(
    mask: impl Fn(usize, usize) -> bool,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
) -> [f32; CELL * CELL] {
    let width = x1 + 1 - x0;
    let height = y1 + 1 - y0;
    let mut out = [0f32; CELL * CELL];
    for row in 0..CELL {
        let sy0 = y0 + row * height / CELL;
        let sy1 = (y0 + (row + 1) * height / CELL).max(sy0 + 1).min(y1 + 1);
        for column in 0..CELL {
            let sx0 = x0 + column * width / CELL;
            let sx1 = (x0 + (column + 1) * width / CELL).max(sx0 + 1).min(x1 + 1);
            let mut inked = 0usize;
            let mut seen = 0usize;
            for y in sy0..sy1 {
                for x in sx0..sx1 {
                    seen += 1;
                    if mask(x, y) {
                        inked += 1;
                    }
                }
            }
            if seen > 0 {
                out[row * CELL + column] = inked as f32 / seen as f32;
            }
        }
    }
    out
}
