// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the RASTER side of Textract's synchronous data plane: a
//! document submitted as pixels rather than as characters.
//!
//! The rule under test, over a table of inputs rather than one recorded page:
//! a synchronous document operation given inline `Bytes` in a raster format
//! `Document$Bytes` carries — "PNG or JPEG format" — answers HTTP 200 with the text its
//! pixels really carry — a `PAGE` block, the `LINE` blocks of the lines of ink on the
//! page and the `WORD` blocks of the words in them — and `UnsupportedDocumentException`
//! answers every byte string no page can be read out of, whether it is in no format the
//! service reads, in a format this channel does not carry, or in an accepted one its
//! decoder cannot read. The rows differ along what the rule generalises over: the format,
//! the size the text is rendered at, where it sits on the page, the polarity and contrast
//! of the ink, the stroke weight, and whether the page carries any text at all.
//!
//! Every page here is rendered in this test from known text with the same public-domain
//! 8x8 font the recogniser holds templates for, so the recognised characters can be
//! compared with the characters that were drawn: the assertion is a ROUND TRIP through
//! real pixels (encode to PNG/JPEG/TIFF, decode, binarise, segment, recognise), not a
//! comparison against a recorded answer.

use std::io::Cursor;

use image::{GrayImage, ImageFormat, Luma};
use serde_json::{json, Value};
use service_com_amazonaws_textract_textract::state;

use crate::document_extraction::{
    assert_block_graph, block_type, blocks_of, call, error_code, harness, of_type,
};

/// The side of one glyph cell of the embedded font.
pub(crate) const CELL: usize = 8;

/// Baseline-to-baseline distance, in glyph cells: tall enough to leave blank rows
/// between two lines of ink, which is what makes them two lines.
pub(crate) const PITCH: usize = 12;

/// How a page is drawn. Each knob is one axis the recognition rule must hold along.
///
/// `pub(crate)` knobs: the geometry suite draws its own pages with these same axes, so
/// one renderer serves both rather than each suite growing its own.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Paper {
    /// Pixels per font pixel: the size the text is rendered at.
    pub(crate) scale: usize,
    /// Blank pixels around the text block.
    pub(crate) margin: usize,
    /// The page's background level.
    pub(crate) background: u8,
    /// The ink's level. Darker or lighter than the background, either way.
    pub(crate) ink: u8,
    /// Grow every stroke by one pixel downwards, for a heavier weight than the font's.
    pub(crate) thicken: bool,
    /// Draw a two-pixel rule across the page under the text: a border, not a line of
    /// text.
    pub(crate) rule: bool,
}

impl Paper {
    pub(crate) const fn plain() -> Self {
        Self {
            scale: 2,
            margin: 6,
            background: 255,
            ink: 0,
            thicken: false,
            rule: false,
        }
    }
}

/// Draw `lines` with the embedded 8x8 font.
///
/// `font8x8::legacy::BASIC_LEGACY` is the same public-domain font the recogniser's
/// templates come from: one byte per row, bit 0 the leftmost pixel.
pub(crate) fn render(lines: &[&str], paper: Paper) -> GrayImage {
    let columns = lines.iter().map(|line| line.chars().count()).max().unwrap_or(0);
    let width = (paper.margin * 2 + columns * CELL * paper.scale).max(1) as u32;
    let height = (paper.margin * 2 + lines.len() * PITCH * paper.scale).max(1) as u32;
    let mut page = GrayImage::from_pixel(width, height, Luma([paper.background]));
    for (row, line) in lines.iter().enumerate() {
        for (column, ch) in line.chars().enumerate() {
            let glyph = font8x8::legacy::BASIC_LEGACY[ch as usize];
            for (gy, bits) in glyph.iter().enumerate() {
                for gx in 0..CELL {
                    if bits & (1 << gx) == 0 {
                        continue;
                    }
                    let left = paper.margin + (column * CELL + gx) * paper.scale;
                    let top = paper.margin + (row * PITCH + gy) * paper.scale;
                    let depth = paper.scale + usize::from(paper.thicken);
                    for dy in 0..depth {
                        for dx in 0..paper.scale {
                            page.put_pixel(
                                (left + dx) as u32,
                                (top + dy) as u32,
                                Luma([paper.ink]),
                            );
                        }
                    }
                }
            }
        }
    }
    if paper.rule {
        let y = height - 3;
        for x in 0..width {
            for dy in 0..2 {
                page.put_pixel(x, y + dy, Luma([paper.ink]));
            }
        }
    }
    page
}

/// Encode a page in one of the raster formats Textract accepts.
///
/// `pub(crate)`: the S3-sourced suite (`crate::s3_sourced_documents`) stages these very
/// bytes as an object of the emulator's own Amazon S3, so both suites submit pages
/// rendered and encoded the one way.
pub(crate) fn encode(page: &GrayImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    page.write_to(&mut out, format)
        .expect("the page encodes in a format Textract accepts");
    out.into_inner()
}

pub(crate) fn raster_document(page: &GrayImage, format: ImageFormat) -> Value {
    json!({ "Bytes": aws_smithy_types::base64::encode(encode(page, format)) })
}

pub(crate) fn texts_of(blocks: &[Value], kind: &str) -> Vec<String> {
    of_type(blocks, kind)
        .iter()
        .map(|block| {
            block
                .get("Text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The confidence of the first block of an answer that reports one.
fn confidence_of(blocks: &[Value]) -> Option<f64> {
    blocks
        .iter()
        .find_map(|block| block.get("Confidence").and_then(Value::as_f64))
}

/// One row of the recognition table: a page, and the text it was drawn from.
struct RasterCase {
    name: &'static str,
    lines: Vec<&'static str>,
    format: ImageFormat,
    paper: Paper,
    /// Whether every character must come back exactly as drawn.
    ///
    /// A page drawn with the font's own strokes round-trips character for character. A
    /// page deliberately drawn with HEAVIER strokes than the font's does not, and should
    /// not be asserted to: closing a letter's aperture is exactly the kind of ink that
    /// makes a recogniser read one glyph as another, so such a row asserts the layout it
    /// read (the lines, and the length of every word in them) rather than the glyphs.
    exact: bool,
}

/// The lines every row that does not need its own text is drawn from. Mixed case,
/// digits, a labelled field and punctuation, so the recogniser is exercised over more
/// than one glyph class.
fn sample_lines() -> Vec<&'static str> {
    vec![
        "SIMPARITY LEDGER",
        "Account Number: 48213",
        "Total Due 1250.00",
    ]
}

/// One row of the table, read character for character.
fn exact(name: &'static str, lines: Vec<&'static str>, format: ImageFormat, paper: Paper) -> RasterCase {
    RasterCase {
        name,
        lines,
        format,
        paper,
        exact: true,
    }
}

fn recognition_cases() -> Vec<RasterCase> {
    vec![
        exact(
            "a PNG at the font's own size",
            sample_lines(),
            ImageFormat::Png,
            Paper {
                scale: 1,
                ..Paper::plain()
            },
        ),
        exact(
            "a PNG rendered three times larger",
            sample_lines(),
            ImageFormat::Png,
            Paper {
                scale: 3,
                ..Paper::plain()
            },
        ),
        exact(
            "a PNG rendered five times larger, far from the page's corner",
            sample_lines(),
            ImageFormat::Png,
            Paper {
                scale: 5,
                margin: 140,
                ..Paper::plain()
            },
        ),
        exact(
            "light ink on a dark page",
            sample_lines(),
            ImageFormat::Png,
            Paper {
                scale: 3,
                background: 12,
                ink: 243,
                ..Paper::plain()
            },
        ),
        exact(
            "low contrast, grey on grey",
            sample_lines(),
            ImageFormat::Png,
            Paper {
                scale: 3,
                background: 205,
                ink: 120,
                ..Paper::plain()
            },
        ),
        RasterCase {
            name: "a heavier stroke than the font's",
            lines: sample_lines(),
            format: ImageFormat::Png,
            paper: Paper {
                scale: 3,
                thicken: true,
                ..Paper::plain()
            },
            exact: false,
        },
        exact(
            "a rule across the page is a border, not a line of text",
            sample_lines(),
            ImageFormat::Png,
            Paper {
                scale: 3,
                rule: true,
                ..Paper::plain()
            },
        ),
        exact(
            "a JPEG, whose coding is lossy",
            sample_lines(),
            ImageFormat::Jpeg,
            Paper {
                scale: 6,
                ..Paper::plain()
            },
        ),
        exact(
            "a single word",
            vec!["SOLO"],
            ImageFormat::Png,
            Paper {
                scale: 4,
                ..Paper::plain()
            },
        ),
        exact(
            "many short lines",
            vec!["One", "Two", "Three", "Four", "Five", "Six"],
            ImageFormat::Png,
            Paper {
                scale: 3,
                ..Paper::plain()
            },
        ),
        exact(
            "digits and separators only",
            vec!["2026-10-22", "48213 / 7"],
            ImageFormat::Png,
            Paper {
                scale: 4,
                ..Paper::plain()
            },
        ),
    ]
}

#[tokio::test]
async fn detect_document_text_reads_the_text_a_raster_really_carries() {
    let router = harness();

    for case in recognition_cases() {
        let page = render(&case.lines, case.paper);
        let request = json!({ "Document": raster_document(&page, case.format) });
        let (status, body) = call(&router, "DetectDocumentText", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: a document in a format Textract accepts must be read, got {status} \
             with {body}",
            case.name
        );
        let blocks = blocks_of(&body);
        assert_block_graph(case.name, &body, &blocks, None);
        assert_eq!(
            body["DocumentMetadata"]["Pages"].as_u64(),
            Some(1),
            "case {:?}: one raster is one page, got {body}",
            case.name
        );
        assert_eq!(
            body.get("DetectDocumentTextModelVersion")
                .and_then(Value::as_str),
            Some("1.0"),
            "case {:?}: expected a model version, got {:?}",
            case.name,
            body.get("DetectDocumentTextModelVersion")
        );
        assert!(
            blocks.iter().all(|block| {
                matches!(block_type(block), "PAGE" | "LINE" | "WORD")
            }),
            "case {:?}: text detection returns PAGE/LINE/WORD only, got {:?}",
            case.name,
            blocks.iter().map(block_type).collect::<Vec<_>>()
        );

        // The round trip: the lines read off the pixels are the lines that were drawn.
        let read_lines = texts_of(&blocks, "LINE");
        let read_words = texts_of(&blocks, "WORD");
        let drawn_words: Vec<String> = case
            .lines
            .iter()
            .flat_map(|line| line.split_whitespace())
            .map(ToString::to_string)
            .collect();
        if case.exact {
            assert_eq!(
                read_lines,
                case.lines
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                "case {:?}: the LINE blocks must report the text the page was drawn from",
                case.name
            );
            assert_eq!(
                read_words, drawn_words,
                "case {:?}: the WORD blocks must report the words the page was drawn from",
                case.name
            );
        } else {
            assert_eq!(
                read_lines.len(),
                case.lines.len(),
                "case {:?}: expected one LINE per line of ink, got {read_lines:?}",
                case.name
            );
            assert_eq!(
                read_words.iter().map(|word| word.chars().count()).collect::<Vec<_>>(),
                drawn_words
                    .iter()
                    .map(|word| word.chars().count())
                    .collect::<Vec<_>>(),
                "case {:?}: expected one WORD per word drawn, of the same length, got \
                 {read_words:?}",
                case.name
            );
        }

        // The confidence is the recogniser's own measurement, inside Percent's range,
        // and not the constant a verbatim text document reports.
        let confidence = confidence_of(&blocks).unwrap_or_default();
        assert!(
            confidence > 0.0 && confidence <= 100.0,
            "case {:?}: a recognised page must report the confidence it was recognised \
             with, got {confidence}",
            case.name
        );

        // Same pixels, same answer: the blocks come from stored state, not from a roll.
        let (repeat_status, repeat) = call(&router, "DetectDocumentText", &request).await;
        assert_eq!(
            repeat_status, 200,
            "case {:?}: the repeat call must also succeed, got {repeat_status}",
            case.name
        );
        assert_eq!(
            blocks_of(&repeat),
            blocks,
            "case {:?}: the same document must answer with the same blocks",
            case.name
        );
    }
}

#[tokio::test]
async fn a_raster_with_no_ink_is_a_page_with_no_words() {
    let router = harness();

    // A page the recogniser finds no text on is still a page: AWS answers 200 with a
    // PAGE block, not a client error. A JPEG of a blank page is not byte-exactly blank
    // once coded, which is why the recogniser needs real contrast to call pixels ink.
    let cases: Vec<(&str, GrayImage, ImageFormat)> = vec![
        (
            "a blank white PNG",
            render(&[], Paper { margin: 200, ..Paper::plain() }),
            ImageFormat::Png,
        ),
        (
            "a blank white JPEG",
            render(&[], Paper { margin: 200, ..Paper::plain() }),
            ImageFormat::Jpeg,
        ),
        (
            "a blank black PNG",
            render(
                &[],
                Paper {
                    margin: 200,
                    background: 0,
                    ..Paper::plain()
                },
            ),
            ImageFormat::Png,
        ),
        (
            "a one-pixel PNG",
            render(&[], Paper { margin: 1, ..Paper::plain() }),
            ImageFormat::Png,
        ),
    ];

    for (name, page, format) in cases {
        for operation in ["DetectDocumentText", "AnalyzeExpense"] {
            let request = json!({ "Document": raster_document(&page, format) });
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 200,
                "case {name:?} {operation}: a blank page is read, not refused, got {status} \
                 with {body}"
            );
            assert_eq!(
                body["DocumentMetadata"]["Pages"].as_u64(),
                Some(1),
                "case {name:?} {operation}: a blank raster is still one page, got {body}"
            );
        }

        let request = json!({ "Document": raster_document(&page, format) });
        let (_, body) = call(&router, "DetectDocumentText", &request).await;
        let blocks = blocks_of(&body);
        assert_eq!(
            blocks.len(),
            1,
            "case {name:?}: a page with no ink holds exactly one PAGE block, got {blocks:?}"
        );
        assert_eq!(
            block_type(&blocks[0]),
            "PAGE",
            "case {name:?}: the one block must be the PAGE, got {:?}",
            blocks[0]
        );
        assert!(
            of_type(&blocks, "WORD").is_empty() && of_type(&blocks, "LINE").is_empty(),
            "case {name:?}: a page with no ink has no words, got {blocks:?}"
        );
    }
}

#[tokio::test]
async fn every_synchronous_operation_reads_the_same_raster() {
    let router = harness();

    // One page, read by all three synchronous operations: each must answer 200 with its
    // own output members over the SAME recognised text.
    let page = render(&sample_lines(), Paper { scale: 4, ..Paper::plain() });
    let document = raster_document(&page, ImageFormat::Png);

    let (status, detected) = call(
        &router,
        "DetectDocumentText",
        &json!({ "Document": document }),
    )
    .await;
    assert_eq!(status, 200, "DetectDocumentText must read the page: {detected}");
    let detected_blocks = blocks_of(&detected);

    for features in [
        vec!["FORMS"],
        vec!["TABLES"],
        vec!["TABLES", "FORMS"],
        vec!["FORMS", "FORMS"],
    ] {
        let case = format!("AnalyzeDocument {features:?}");
        let (status, body) = call(
            &router,
            "AnalyzeDocument",
            &json!({ "Document": document, "FeatureTypes": features }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {body}"
        );
        let blocks = blocks_of(&body);
        assert_block_graph(&case, &body, &blocks, None);
        assert_eq!(
            body.get("AnalyzeDocumentModelVersion")
                .and_then(Value::as_str),
            Some("1.0"),
            "case {case}: expected a model version, got {body}"
        );
        assert_eq!(
            body["DocumentMetadata"]["Pages"].as_u64(),
            Some(1),
            "case {case}: expected one page, got {body}"
        );
        // The analysis paths read the same document, so their PAGE/LINE/WORD blocks are
        // the ones text detection reports; the features only ADD blocks.
        assert_eq!(
            texts_of(&blocks, "LINE"),
            texts_of(&detected_blocks, "LINE"),
            "case {case}: the analysis path must read the same lines as text detection"
        );
        assert_eq!(
            texts_of(&blocks, "WORD"),
            texts_of(&detected_blocks, "WORD"),
            "case {case}: the analysis path must read the same words as text detection"
        );
        // 'Account Number: 48213' is a labelled line, so FORMS reports a KEY and a VALUE
        // for it off the recognised text.
        if features.contains(&"FORMS") {
            let keys = of_type(&blocks, "KEY_VALUE_SET");
            assert!(
                !keys.is_empty(),
                "case {case}: a labelled line must answer with KEY_VALUE_SET blocks, got \
                 {blocks:?}"
            );
        }
    }

    let (status, expense) = call(&router, "AnalyzeExpense", &json!({ "Document": document })).await;
    assert_eq!(status, 200, "AnalyzeExpense must read the page: {expense}");
    assert_eq!(
        expense["DocumentMetadata"]["Pages"].as_u64(),
        Some(1),
        "AnalyzeExpense: expected one page, got {expense}"
    );
    let documents = expense
        .get("ExpenseDocuments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        documents.len(),
        1,
        "AnalyzeExpense: expected one expense document per page, got {expense}"
    );
    let first = &documents[0];
    assert_eq!(
        first.get("ExpenseIndex").and_then(Value::as_u64),
        Some(1),
        "AnalyzeExpense: expense documents are indexed from 1, got {first}"
    );
    for member in ["SummaryFields", "LineItemGroups", "Blocks"] {
        assert!(
            first.get(member).and_then(Value::as_array).is_some(),
            "AnalyzeExpense: an expense document must carry {member}, got {first}"
        );
    }
    assert_eq!(
        first
            .get("Blocks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        detected_blocks,
        "AnalyzeExpense: an expense document's Blocks are the blocks DetectDocumentText \
         reports for that page"
    );
    let groups = first
        .get("LineItemGroups")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        groups.first().and_then(|group| group
            .get("LineItemGroupIndex")
            .and_then(Value::as_u64)),
        Some(1),
        "AnalyzeExpense: the first line item group is indexed 1, got {groups:?}"
    );
}

#[tokio::test]
async fn two_different_pages_are_read_as_two_different_documents() {
    let router = harness();

    // Nothing about an answer is keyed to the request's bytes rather than to what they
    // DRAW: two pages of different text answer with different text, and the same text
    // rendered at two sizes answers with the same text.
    let paper = Paper { scale: 3, ..Paper::plain() };
    let mut seen: Vec<(&str, Vec<String>)> = Vec::new();
    for (name, lines, scale) in [
        ("one invoice", vec!["Invoice 1001", "Total 10.00"], 3usize),
        ("another invoice", vec!["Invoice 2002", "Total 20.00"], 3),
        ("the first invoice, larger", vec!["Invoice 1001", "Total 10.00"], 5),
    ] {
        let page = render(&lines, Paper { scale, ..paper });
        let (status, body) = call(
            &router,
            "DetectDocumentText",
            &json!({ "Document": raster_document(&page, ImageFormat::Png) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with {body}"
        );
        let read = texts_of(&blocks_of(&body), "LINE");
        assert_eq!(
            read,
            lines.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {name:?}: the lines read must be the lines drawn"
        );
        seen.push((name, read));
    }
    assert_ne!(
        seen[0].1, seen[1].1,
        "two pages of different text must be read as different text"
    );
    assert_eq!(
        seen[0].1, seen[2].1,
        "one text rendered at two sizes must be read as the same text"
    );
}

#[tokio::test]
async fn bytes_in_no_format_this_service_reads_are_refused_as_unsupported() {
    let router = harness();

    // A document no page can be read out of is answered UnsupportedDocumentException
    // whether its bytes are in a format Textract accepts and cannot be decoded, or in no
    // format the service reads at all: the service draws no distinction between them.
    // Each row names the status and the code, which are the requirement; the message is
    // asserted in `crate::refusal_messages`.
    let good = encode(
        &render(&sample_lines(), Paper { scale: 3, ..Paper::plain() }),
        ImageFormat::Png,
    );
    let truncated = good[..good.len() / 2].to_vec();
    let mut corrupt_body = good.clone();
    for byte in corrupt_body.iter_mut().skip(60) {
        *byte ^= 0x5A;
    }

    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "a PNG cut in half",
            truncated,
            "UnsupportedDocumentException",
        ),
        (
            // `Document$Bytes` carries a document "in PNG or JPEG format", so a TIFF —
            // a format Textract reads out of an Amazon S3 object — is not a document
            // this channel takes. `crate::s3_sourced_documents` reads one from an object.
            "a TIFF, which inline Bytes do not carry",
            encode(
                &render(&sample_lines(), Paper { scale: 3, ..Paper::plain() }),
                ImageFormat::Tiff,
            ),
            "UnsupportedDocumentException",
        ),
        (
            // Characters are not one of the document formats Textract processes.
            "a text document",
            b"Invoice Number: INV-1\nTotal Due: 42.50\n".to_vec(),
            "UnsupportedDocumentException",
        ),
        (
            // A GIF is a real raster format, and not one Textract accepts.
            "a GIF, which Textract does not accept",
            b"GIF89a\x01\x00\x01\x00\x00\x00\x00;".to_vec(),
            "UnsupportedDocumentException",
        ),
        (
            // Textract itself accepts PDF; this emulator renders no PDF page and parses
            // no PDF content stream, so it reads nothing out of one. The status and the
            // code are the requirement here too, not the sentence.
            "a PDF, which this emulator does not decode",
            b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n%%EOF\n".to_vec(),
            "UnsupportedDocumentException",
        ),
        (
            "bytes in no format at all",
            vec![0x00, 0x01, 0x02, 0xFF, 0xFE, 0x7F],
            "UnsupportedDocumentException",
        ),
    ];

    for (name, bytes, expected) in cases {
        for operation in ["DetectDocumentText", "AnalyzeExpense"] {
            let (status, body) = call(
                &router,
                operation,
                &json!({ "Document": { "Bytes": aws_smithy_types::base64::encode(&bytes) } }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {name:?} {operation}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                expected,
                "case {name:?} {operation}: expected {expected}, got {body}"
            );
        }
    }

    // A corrupt body may decode to garbage pixels or not decode at all; either way the
    // service answers, and never with text it did not read.
    let (status, body) = call(
        &router,
        "DetectDocumentText",
        &json!({ "Document": { "Bytes": aws_smithy_types::base64::encode(&corrupt_body) } }),
    )
    .await;
    if status == 200 {
        // It decoded to garbage pixels. Whatever the recogniser made of them, the answer
        // must be a well-formed block graph, and it must NOT be the text of the page the
        // bytes were corrupted from: that text is no longer in the pixels.
        let blocks = blocks_of(&body);
        assert_block_graph("a corrupt PNG body", &body, &blocks, None);
        let read = texts_of(&blocks, "LINE");
        assert_ne!(
            read,
            sample_lines()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            "case corrupt PNG body: the answer must report what the corrupted pixels hold, \
             not the text the page was drawn from"
        );
    } else {
        assert_eq!(
            error_code(&body),
            "UnsupportedDocumentException",
            "case corrupt PNG body: a document no page can be read out of is an \
             unsupported document format, got {status} with {body}"
        );
    }
}

/// Draw `lines` with strokes that are NOT the recogniser's own templates: each glyph row
/// is slid sideways, so the page is typeset in a slanted face no template matches
/// exactly.
///
/// The recognition rows above are drawn with the same public-domain font the recogniser
/// holds templates for, which makes that round trip partly self-referential. This
/// renderer exists to exercise the other path: ink whose shape is not a template's.
fn render_slanted(lines: &[&str], paper: Paper) -> GrayImage {
    let columns = lines.iter().map(|line| line.chars().count()).max().unwrap_or(0);
    // The lean pushes ink to the right of its own cell, so the sheet is widened by the
    // largest offset any row takes: `put_pixel` panics outside the image.
    let lean = CELL * paper.scale / 3;
    let width = (paper.margin * 2 + columns * CELL * paper.scale + lean).max(1) as u32;
    let height = (paper.margin * 2 + lines.len() * PITCH * paper.scale).max(1) as u32;
    let mut page = GrayImage::from_pixel(width, height, Luma([paper.background]));
    for (row, line) in lines.iter().enumerate() {
        for (column, ch) in line.chars().enumerate() {
            let glyph = font8x8::legacy::BASIC_LEGACY[ch as usize];
            for (gy, bits) in glyph.iter().enumerate() {
                // Higher rows lean further right: a slant, which changes every glyph's
                // shape without changing the text that was drawn.
                let slide = (CELL - 1 - gy) * paper.scale / 3;
                for gx in 0..CELL {
                    if bits & (1 << gx) == 0 {
                        continue;
                    }
                    let left = paper.margin + (column * CELL + gx) * paper.scale + slide;
                    let top = paper.margin + (row * PITCH + gy) * paper.scale;
                    for dy in 0..paper.scale {
                        for dx in 0..paper.scale {
                            page.put_pixel((left + dx) as u32, (top + dy) as u32, Luma([paper.ink]));
                        }
                    }
                }
            }
        }
    }
    page
}

#[tokio::test]
async fn a_page_in_a_typeface_the_recogniser_has_no_template_for_invents_nothing() {
    let router = harness();

    // The generalisation this exercises: the answer for a raster is whatever the
    // recogniser MEASURED off its pixels, whatever typeface the page is set in. A page
    // whose strokes are not the recogniser's own templates is still read — status,
    // metadata and the block graph hold — but its glyph identities are NOT asserted
    // here, because ink that matches no template is reported as nothing at all rather
    // than guessed: that is the honest answer, and it is why this row asserts the shape
    // of the answer and its determinism instead of a transcript. A page whose ink
    // matches no glyph therefore answers like a page with no ink (200 with a PAGE
    // block), which `a_raster_with_no_ink_is_a_page_with_no_words` pins.
    let cases: Vec<(&str, Vec<&str>, Paper)> = vec![
        (
            "a slanted face, three times the font's size",
            sample_lines(),
            Paper {
                scale: 3,
                ..Paper::plain()
            },
        ),
        (
            "a slanted face, six times the font's size, light on dark",
            sample_lines(),
            Paper {
                scale: 6,
                background: 20,
                ink: 235,
                ..Paper::plain()
            },
        ),
        (
            "a slanted face with heavier strokes",
            vec!["SOLO LEDGER"],
            Paper {
                scale: 4,
                thicken: true,
                ..Paper::plain()
            },
        ),
    ];

    for (name, lines, paper) in cases {
        let page = render_slanted(&lines, paper);
        let request = json!({ "Document": raster_document(&page, ImageFormat::Png) });
        let (status, body) = call(&router, "DetectDocumentText", &request).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a readable raster is read whatever typeface it is set in, got \
             {status} with {body}"
        );
        assert_eq!(
            body["DocumentMetadata"]["Pages"].as_u64(),
            Some(1),
            "case {name:?}: one raster is one page, got {body}"
        );
        let blocks = blocks_of(&body);
        assert_block_graph(name, &body, &blocks, None);
        let read_lines = texts_of(&blocks, "LINE");
        assert!(
            read_lines.len() <= lines.len(),
            "case {name:?}: a page cannot hold more lines of text than were drawn on it, \
             got {read_lines:?}"
        );
        for word in texts_of(&blocks, "WORD") {
            assert!(
                !word.is_empty()
                    && word
                        .chars()
                        .all(|ch| ch.is_ascii_graphic() && !ch.is_whitespace()),
                "case {name:?}: a WORD block must report printable characters the \
                 recogniser measured, got {word:?}"
            );
        }
        if let Some(confidence) = confidence_of(&blocks) {
            assert!(
                confidence > 0.0 && confidence <= 100.0,
                "case {name:?}: a recognised page reports the confidence it was \
                 recognised with, got {confidence}"
            );
        }
        // Nothing here is a roll of the dice: the same pixels answer the same way.
        let (repeat_status, repeat) = call(&router, "DetectDocumentText", &request).await;
        assert_eq!(
            repeat_status, 200,
            "case {name:?}: the repeat call must also succeed, got {repeat_status}"
        );
        assert_eq!(
            blocks_of(&repeat),
            blocks,
            "case {name:?}: the same document must answer with the same blocks"
        );
    }
}

#[test]
fn a_raster_over_the_pixel_ceiling_is_refused_as_too_large() {
    // The ceilings are this emulator's, not AWS's, so the refusal is asserted against
    // the constant that documents the ceiling rather than against a size written out
    // here.
    let side = state::ocr::MAX_RASTER_SIDE + 1;
    let page = GrayImage::from_pixel(side, 1, Luma([255u8]));
    let bytes = encode(&page, ImageFormat::Png);
    let document = service_com_amazonaws_textract_textract::ops::types::Document {
        bytes: Some(bytes.clone()),
        s3_object: None,
    };
    let reference = state::reference_for_document(&document).expect("a readable document");
    let refusal = state::read_content(&reference, Some(&bytes)).expect_err("over the ceiling");
    assert!(
        matches!(refusal, state::Unreadable::TooLarge(_)),
        "a raster wider than the ceiling must be refused as too large, got {refusal:?}"
    );
}
