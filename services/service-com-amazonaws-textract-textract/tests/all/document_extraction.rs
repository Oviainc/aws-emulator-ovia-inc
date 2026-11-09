// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Textract's document-extraction loop over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: Textract.<Operation>`).
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded shape:
//!
//! * a document's blocks describe the text that was really read out of it, so the same
//!   document always answers with the same block graph and two documents never share
//!   one; a document whose text cannot be read is REFUSED with the modelled error for
//!   why, never answered with invented text;
//! * `PAGE`/`LINE`/`WORD` is the whole result of the text-detection paths, and the
//!   analysis paths add exactly the blocks the requested `FeatureTypes` name;
//! * a `Start*` persists a job whose matching `Get*` reports its clock-derived status,
//!   refuses a `JobId` nothing started, and refuses a cursor it never issued;
//! * a cursor pages the stored block list exactly, and every caller-supplied cursor is
//!   rejected rather than parsed into a panic.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_textract_textract::ops::types::{
    BLOCK_TYPE_VALUES, ENTITY_TYPE_VALUES, RELATIONSHIP_TYPE_VALUES,
};
use service_com_amazonaws_textract_textract::state;
use service_com_amazonaws_textract_textract::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Comfortably past `state::JOB_COMPLETION_DELAY`, so a job's terminal transition is
/// due. `tokio`'s `time` feature is not enabled for generated crates, so this blocks
/// the test thread rather than the runtime's timer.
const PAST_JOB_COMPLETION: Duration = Duration::from_millis(900);

/// The message Amazon Textract states for an S3 object it could not obtain: the
/// `InvalidS3ObjectException` sentence, which is also the `StatusMessage` of a job that
/// failed for that reason.
const UNREADABLE_OBJECT_MESSAGE: &str =
    "Unable to get object metadata from S3. Check object key, region and/or access permissions.";

/// The emulator, with this one service mounted on a shared engine.
///
/// `pub(crate)` here and below: the other suites of this test target share this one
/// harness rather than each standing up its own.
pub(crate) fn harness() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Textract.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn blocks_of(body: &Value) -> Vec<Value> {
    body.get("Blocks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Message` of an error answer, in either casing the envelopes render.
///
/// `pub(crate)`: every suite of this target that asserts the WORDING of a refusal reads
/// it through this one accessor.
pub(crate) fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn block_type(block: &Value) -> &str {
    block
        .get("BlockType")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn block_text(block: &Value) -> &str {
    block.get("Text").and_then(Value::as_str).unwrap_or_default()
}

pub(crate) fn block_id(block: &Value) -> &str {
    block.get("Id").and_then(Value::as_str).unwrap_or_default()
}

pub(crate) fn of_type<'a>(blocks: &'a [Value], wanted: &str) -> Vec<&'a Value> {
    blocks
        .iter()
        .filter(|block| block_type(block) == wanted)
        .collect()
}

fn child_ids(block: &Value) -> Vec<String> {
    related_ids(block, "CHILD")
}

fn related_ids(block: &Value, kind: &str) -> Vec<String> {
    block
        .get("Relationships")
        .and_then(Value::as_array)
        .map(|rels| {
            rels.iter()
                .filter(|rel| rel.get("Type").and_then(Value::as_str) == Some(kind))
                .flat_map(|rel| {
                    rel.get("Ids")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|id| id.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// How many character cells a tab is drawn as.
///
/// A column break is a gap distinctly wider than the line's own word gaps
/// (`crate::state::ocr`'s clustering), so a tab is drawn wide enough to stand apart from
/// the single-cell gap a space is drawn as.
pub(crate) const TAB_CELLS: usize = 6;

/// A `Document` of inline `Bytes` this service reads: `text`'s own lines DRAWN onto a
/// page, encoded as a PNG.
///
/// `Document$Bytes` carries a document "in PNG or JPEG format", so a suite that wants a
/// document holding known text submits that text as PIXELS and the service reads it back
/// off them. The page is drawn with the same public-domain 8x8 font the recogniser holds
/// templates for (`crate::raster_documents::render`), so the characters that come back
/// are the characters that went in.
pub(crate) fn text_document(text: &str) -> Value {
    crate::raster_documents::raster_document(&text_page(text), image::ImageFormat::Png)
}

/// The page [`text_document`] draws, for a suite that needs the pixels or another format.
pub(crate) fn text_page(text: &str) -> image::GrayImage {
    let lines = drawn_lines(text);
    crate::raster_documents::render(
        &lines.iter().map(String::as_str).collect::<Vec<_>>(),
        crate::raster_documents::Paper {
            scale: 3,
            ..crate::raster_documents::Paper::plain()
        },
    )
}

/// `text`'s lines as they are DRAWN: one line of the document per line of ink, with each
/// tab widened into the gap that makes its neighbours two columns rather than two words.
pub(crate) fn drawn_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.replace('\t', &" ".repeat(TAB_CELLS)))
        .collect()
}

fn find<'a>(blocks: &'a [Value], id: &str) -> Option<&'a Value> {
    blocks.iter().find(|block| block_id(block) == id)
}

/// Collapse runs of whitespace, so a LINE's text can be compared with its words.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every invariant a Textract block graph holds, whatever the document or the
/// requested features.
///
/// `expected_confidence` is the one value every block of this answer must report, where a
/// caller knows it. A page read off its pixels is passed `None`: each of its blocks
/// reports the confidence measured over the characters IT was read from, so the values
/// differ between blocks and only `Percent`'s range is asserted here
/// (`confidence_varies_with_the_text_it_was_measured_from` is where the spread itself is
/// asserted).
pub(crate) fn assert_block_graph(
    case: &str,
    body: &Value,
    blocks: &[Value],
    expected_confidence: Option<f32>,
) {
    let pages = body
        .get("DocumentMetadata")
        .and_then(|m| m.get("Pages"))
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize;
    assert!(
        pages >= 1,
        "case {case:?}: DocumentMetadata.Pages must be at least 1, got {pages:?}"
    );

    let mut ids: Vec<&str> = blocks.iter().map(block_id).collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        total,
        "case {case:?}: every block Id must be unique, got {total} blocks and {} distinct ids",
        ids.len()
    );
    assert!(
        ids.iter().all(|id| !id.is_empty()),
        "case {case:?}: every block must carry an Id"
    );

    let page_blocks = of_type(blocks, "PAGE");
    assert_eq!(
        page_blocks.len(),
        pages,
        "case {case:?}: expected one PAGE block per page ({pages}), got {}",
        page_blocks.len()
    );

    for block in blocks {
        // `BLOCK_TYPE_VALUES` / `RELATIONSHIP_TYPE_VALUES` / `ENTITY_TYPE_VALUES` are
        // the BlockType, RelationshipType and EntityType enums of the Smithy model.
        assert!(
            BLOCK_TYPE_VALUES.contains(&block_type(block)),
            "case {case:?}: {:?} is not a modelled BlockType",
            block_type(block)
        );
        let page = block.get("Page").and_then(Value::as_u64).unwrap_or_default() as usize;
        assert!(
            page >= 1 && page <= pages,
            "case {case:?}: block {} has Page {page}, outside 1..={pages}",
            block_id(block)
        );
        let geometry = block
            .get("Geometry")
            .unwrap_or_else(|| panic!("case {case:?}: block {} has no Geometry", block_id(block)));
        let bbox = geometry.get("BoundingBox").unwrap_or_else(|| {
            panic!(
                "case {case:?}: block {} has no BoundingBox",
                block_id(block)
            )
        });
        for member in ["Left", "Top", "Width", "Height"] {
            let value = bbox.get(member).and_then(Value::as_f64).unwrap_or(-1.0);
            assert!(
                (0.0..=1.0).contains(&value),
                "case {case:?}: block {} BoundingBox.{member} is {value}, outside 0..=1",
                block_id(block)
            );
        }
        let polygon = geometry
            .get("Polygon")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            polygon.len(),
            4,
            "case {case:?}: block {} Polygon must trace the bounding box, got {} points",
            block_id(block),
            polygon.len()
        );
        if let Some(confidence) = block.get("Confidence").and_then(Value::as_f64) {
            assert!(
                (0.0..=100.0).contains(&confidence),
                "case {case:?}: block {} Confidence is {confidence}, outside Percent's 0..=100",
                block_id(block)
            );
            if let Some(expected) = expected_confidence.map(f64::from) {
                assert!(
                    (confidence - expected).abs() < 0.001,
                    "case {case:?}: block {} must report the one confidence {expected} of a \
                     document read verbatim, got {confidence}",
                    block_id(block)
                );
            }
        }
        for rel in block
            .get("Relationships")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let kind = rel.get("Type").and_then(Value::as_str).unwrap_or_default();
            assert!(
                RELATIONSHIP_TYPE_VALUES.contains(&kind),
                "case {case:?}: {kind:?} is not a modelled RelationshipType"
            );
            for id in rel
                .get("Ids")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let id = id.as_str().unwrap_or_default();
                assert!(
                    find(blocks, id).is_some(),
                    "case {case:?}: block {} relates to {id}, which is not in the result",
                    block_id(block)
                );
            }
        }
        for entity in block
            .get("EntityTypes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let entity = entity.as_str().unwrap_or_default();
            assert!(
                ENTITY_TYPE_VALUES.contains(&entity),
                "case {case:?}: {entity:?} is not a modelled EntityType"
            );
        }
    }

    // A PAGE parents the page's top-level blocks; a LINE parents its own words.
    for page_block in &page_blocks {
        for id in child_ids(page_block) {
            let child = find(blocks, &id)
                .unwrap_or_else(|| panic!("case {case:?}: PAGE child {id} is missing"));
            assert!(
                matches!(block_type(child), "LINE" | "KEY_VALUE_SET" | "TABLE"),
                "case {case:?}: PAGE child {id} is a {:?}, not a top-level block type",
                block_type(child)
            );
        }
    }
    for line in of_type(blocks, "LINE") {
        let words: Vec<String> = child_ids(line)
            .iter()
            .map(|id| {
                let word = find(blocks, id)
                    .unwrap_or_else(|| panic!("case {case:?}: LINE child {id} is missing"));
                assert_eq!(
                    block_type(word),
                    "WORD",
                    "case {case:?}: a LINE's children must be WORD blocks, got {:?}",
                    block_type(word)
                );
                assert_eq!(
                    word.get("TextType").and_then(Value::as_str),
                    Some("PRINTED"),
                    "case {case:?}: WORD {id} must report a TextType, got {:?}",
                    word.get("TextType")
                );
                block_text(word).to_string()
            })
            .collect();
        assert_eq!(
            words.join(" "),
            collapse(block_text(line)),
            "case {case:?}: LINE {} words must reconstruct its text",
            block_id(line)
        );
    }
}

/// `RowIndex`, `ColumnIndex`, `RowSpan`, `ColumnSpan` and `EntityTypes` are not
/// returned by the text-detection paths (per the `Block` docs).
fn assert_text_detection_only(case: &str, blocks: &[Value]) {
    for block in blocks {
        assert!(
            matches!(block_type(block), "PAGE" | "LINE" | "WORD"),
            "case {case:?}: text detection returns PAGE/LINE/WORD only, got {:?}",
            block_type(block)
        );
        for member in [
            "RowIndex",
            "ColumnIndex",
            "RowSpan",
            "ColumnSpan",
            "EntityTypes",
        ] {
            assert!(
                block.get(member).is_none(),
                "case {case:?}: block {} must not carry {member}, got {:?}",
                block_id(block),
                block.get(member)
            );
        }
    }
}

fn line_texts(blocks: &[Value]) -> Vec<String> {
    of_type(blocks, "LINE")
        .iter()
        .map(|line| block_text(line).to_string())
        .collect()
}

/// One readable-document case of the DetectDocumentText table.
struct DetectCase {
    name: &'static str,
    text: &'static str,
    expected_pages: usize,
    /// The lines the document really holds, in reading order across its pages.
    expected_lines: Vec<&'static str>,
}

#[tokio::test]
async fn detect_document_text_reports_the_text_the_document_holds() {
    let router = harness();

    // Every row is a document DRAWN from known text (`text_document`), so what is
    // asserted is the round trip: the lines that come back are the lines that went in.
    // One raster is one page — the multi-page document formats (PDF, and a TIFF of
    // several frames) are not ones this emulator decodes, so every row is a page.
    let cases = vec![
        DetectCase {
            name: "one page",
            text: "Invoice Number: INV-1024\nTotal Due: 42.50\nThank you for your business",
            expected_pages: 1,
            expected_lines: vec![
                "Invoice Number: INV-1024",
                "Total Due: 42.50",
                "Thank you for your business",
            ],
        },
        DetectCase {
            name: "tab separated rows are still lines and words",
            text: "Item Code\tQuantity Shipped\nWidget Mk2\t3 Crates",
            expected_pages: 1,
            expected_lines: vec!["Item Code\tQuantity Shipped", "Widget Mk2\t3 Crates"],
        },
        DetectCase {
            name: "blank lines carry no block",
            text: "Only line\n\n   \n",
            expected_pages: 1,
            expected_lines: vec!["Only line"],
        },
        DetectCase {
            name: "digits and punctuation",
            text: "Ref: AB-7/9\nAmount 12.00 (net)",
            expected_pages: 1,
            expected_lines: vec!["Ref: AB-7/9", "Amount 12.00 (net)"],
        },
        DetectCase {
            name: "a single word",
            text: "Solo",
            expected_pages: 1,
            expected_lines: vec!["Solo"],
        },
    ];

    let mut first_page_ids: Vec<(&str, String)> = Vec::new();
    for case in &cases {
        let request = json!({ "Document": text_document(case.text) });
        let (status, body) = call(&router, "DetectDocumentText", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {body}",
            case.name
        );
        let blocks = blocks_of(&body);
        assert!(
            !blocks.is_empty(),
            "case {:?}: expected blocks, got {body}",
            case.name
        );
        // A recognised page reports the confidence measured over each block's own
        // characters, so only `Percent`'s range is asserted here.
        assert_block_graph(case.name, &body, &blocks, None);
        assert_text_detection_only(case.name, &blocks);
        assert_eq!(
            body.get("DetectDocumentTextModelVersion")
                .and_then(Value::as_str),
            Some("1.0"),
            "case {:?}: expected a model version, got {:?}",
            case.name,
            body.get("DetectDocumentTextModelVersion")
        );
        assert_eq!(
            body["DocumentMetadata"]["Pages"].as_u64(),
            Some(case.expected_pages as u64),
            "case {:?}: expected {} pages, got {body}",
            case.name,
            case.expected_pages
        );
        assert_eq!(
            line_texts(&blocks),
            case.expected_lines
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            "case {:?}: LINE blocks must be the document's own lines",
            case.name
        );

        // Same document, same blocks: the answer comes from stored state, not a roll.
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

        let page_id = of_type(&blocks, "PAGE")
            .first()
            .map(|page| block_id(page).to_string())
            .unwrap_or_default();
        first_page_ids.push((case.name, page_id));
    }

    for (i, (left_name, left)) in first_page_ids.iter().enumerate() {
        for (right_name, right) in first_page_ids.iter().skip(i + 1) {
            assert_ne!(
                left, right,
                "case {left_name:?} vs {right_name:?}: two different documents must not share a block id"
            );
        }
    }
}

#[tokio::test]
async fn detect_document_text_refuses_a_document_whose_text_it_cannot_read() {
    let router = harness();

    // A PNG signature followed by bytes that are not a PNG body: a document in a format
    // Textract accepts whose bytes cannot be decoded, which is a document no page can be
    // read out of — `UnsupportedDocumentException`, the same answer as bytes in no format
    // this service reads at all.
    let truncated_png = [
        0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x01, 0x02, 0xFF,
    ];
    // More marks than this emulator recognises on one page: a document too large to
    // process, whatever format it arrived in.
    let dense: Vec<String> = (0..120).map(|_| "x".repeat(90)).collect();
    let dense_page = crate::raster_documents::render(
        &dense.iter().map(String::as_str).collect::<Vec<_>>(),
        crate::raster_documents::Paper {
            scale: 1,
            ..crate::raster_documents::Paper::plain()
        },
    );
    let too_many_bytes = vec![b'a'; state::MAX_INLINE_DOCUMENT_BYTES + 1];

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a truncated PNG, which is a supported format this service cannot decode",
            json!({ "Bytes": aws_smithy_types::base64::encode(truncated_png) }),
            "UnsupportedDocumentException",
        ),
        (
            "bytes in no format this service reads",
            json!({ "Bytes": aws_smithy_types::base64::encode([0x00u8, 0x01, 0x02, 0xFF]) }),
            "UnsupportedDocumentException",
        ),
        (
            "characters, which are not a document format Document$Bytes accepts",
            json!({ "Bytes": aws_smithy_types::base64::encode(b"Invoice Number: INV-1\n") }),
            "UnsupportedDocumentException",
        ),
        (
            "an S3 object, which this emulator cannot read",
            json!({ "S3Object": { "Bucket": "docs", "Name": "invoice.pdf" } }),
            "InvalidS3ObjectException",
        ),
        (
            "an S3 object with a version",
            json!({ "S3Object": { "Bucket": "docs", "Name": "invoice.pdf", "Version": "v2" } }),
            "InvalidS3ObjectException",
        ),
        (
            "more bytes than a synchronous call accepts",
            json!({ "Bytes": aws_smithy_types::base64::encode(&too_many_bytes) }),
            "DocumentTooLargeException",
        ),
        (
            "more marks than one page may hold",
            crate::raster_documents::raster_document(&dense_page, image::ImageFormat::Png),
            "DocumentTooLargeException",
        ),
        (
            "neither Bytes nor S3Object",
            json!({}),
            "InvalidParameterException",
        ),
        (
            "both Bytes and S3Object",
            json!({
                "Bytes": aws_smithy_types::base64::encode(b"hello"),
                "S3Object": { "Bucket": "docs", "Name": "invoice.pdf" }
            }),
            "InvalidParameterException",
        ),
        (
            "S3Object without a Bucket",
            json!({ "S3Object": { "Name": "invoice.pdf" } }),
            "InvalidParameterException",
        ),
        (
            "S3Object without a Name",
            json!({ "S3Object": { "Bucket": "docs" } }),
            "InvalidParameterException",
        ),
    ];

    for (name, document, expected) in cases {
        let (status, body) = call(
            &router,
            "DetectDocumentText",
            &json!({ "Document": document }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {name:?}: expected {expected}, got {body}"
        );
        assert!(
            body.get("Blocks").is_none(),
            "case {name:?}: a refusal must carry no blocks, got {body}"
        );
    }
}

/// One case of the AnalyzeDocument feature table.
struct FeatureCase {
    name: &'static str,
    feature_types: Vec<&'static str>,
    status: u16,
    /// Block types the result must contain beyond PAGE/LINE/WORD.
    expected_extra: Vec<&'static str>,
    /// Block types the result must NOT contain.
    forbidden: Vec<&'static str>,
}

/// A document whose text carries both labelled fields and a tab-separated table, so
/// FORMS and TABLES each have real text to read.
///
/// Every cell of the table holds more than one word: what makes a gap a COLUMN break
/// rather than a word break is that it is distinctly wider than the line's own word gaps,
/// so a row of one-word cells is a row of words.
const ANALYSIS_DOCUMENT: &str = "Statement of Account\nInvoice Number: INV-7781\n\
                                 Customer Name: Acme Corporation\n\
                                 Item Code\tQuantity Sent\tAmount Due\n\
                                 Widget Mk2\t3 Crates\t12.00 USD\n\
                                 Gadget Mk1\t1 Crate\t99.95 USD\n\
                                 Thank you";

#[tokio::test]
async fn analyze_document_block_set_follows_the_requested_feature_types() {
    let router = harness();
    let document = text_document(ANALYSIS_DOCUMENT);

    let detect = call(
        &router,
        "DetectDocumentText",
        &json!({ "Document": document }),
    )
    .await;
    assert_eq!(
        detect.0, 200,
        "DetectDocumentText must succeed: {:?}",
        detect.1
    );
    let text_blocks = blocks_of(&detect.1);

    let cases = vec![
        FeatureCase {
            name: "FORMS only",
            feature_types: vec!["FORMS"],
            status: 200,
            expected_extra: vec!["KEY_VALUE_SET"],
            forbidden: vec!["TABLE", "CELL"],
        },
        FeatureCase {
            name: "TABLES only",
            feature_types: vec!["TABLES"],
            status: 200,
            expected_extra: vec!["TABLE", "CELL"],
            forbidden: vec!["KEY_VALUE_SET"],
        },
        FeatureCase {
            name: "FORMS and TABLES",
            feature_types: vec!["FORMS", "TABLES"],
            status: 200,
            expected_extra: vec!["KEY_VALUE_SET", "TABLE", "CELL"],
            forbidden: vec![],
        },
        FeatureCase {
            name: "the same features in the other order",
            feature_types: vec!["TABLES", "FORMS"],
            status: 200,
            expected_extra: vec!["KEY_VALUE_SET", "TABLE", "CELL"],
            forbidden: vec![],
        },
        FeatureCase {
            name: "an empty feature list",
            feature_types: vec![],
            status: 400,
            expected_extra: vec![],
            forbidden: vec![],
        },
        FeatureCase {
            // LAYOUT is a modelled FeatureType this slice derives no blocks for, so it
            // is refused rather than answered with a 200 that omits them.
            name: "a modelled feature with no derivation yet",
            feature_types: vec!["LAYOUT"],
            status: 501,
            expected_extra: vec![],
            forbidden: vec![],
        },
    ];

    for case in &cases {
        let request = json!({
            "Document": document,
            "FeatureTypes": case.feature_types,
        });
        let (status, body) = call(&router, "AnalyzeDocument", &request).await;
        assert_eq!(
            status, case.status,
            "case {:?}: expected HTTP {}, got {status} with {body}",
            case.name, case.status
        );
        if case.status != 200 {
            if case.status == 400 {
                assert_eq!(
                    error_code(&body),
                    "InvalidParameterException",
                    "case {:?}: expected InvalidParameterException, got {body}",
                    case.name
                );
            }
            continue;
        }

        let blocks = blocks_of(&body);
        assert_block_graph(case.name, &body, &blocks, None);
        assert_eq!(
            body.get("AnalyzeDocumentModelVersion")
                .and_then(Value::as_str),
            Some("1.0"),
            "case {:?}: expected a model version, got {:?}",
            case.name,
            body.get("AnalyzeDocumentModelVersion")
        );

        for wanted in &case.expected_extra {
            assert!(
                !of_type(&blocks, wanted).is_empty(),
                "case {:?}: expected at least one {wanted} block, got {:?}",
                case.name,
                blocks.iter().map(block_type).collect::<Vec<_>>()
            );
        }
        for forbidden in &case.forbidden {
            assert!(
                of_type(&blocks, forbidden).is_empty(),
                "case {:?}: expected no {forbidden} block, got {:?}",
                case.name,
                blocks.iter().map(block_type).collect::<Vec<_>>()
            );
        }

        // The text layer of the analysis is the SAME graph DetectDocumentText answered
        // for this document: analysis adds blocks, it does not re-read the text.
        let analysed_text: Vec<&Value> = blocks
            .iter()
            .filter(|block| matches!(block_type(block), "PAGE" | "LINE" | "WORD"))
            .collect();
        assert_eq!(
            analysed_text.iter().map(|b| block_id(b)).collect::<Vec<_>>(),
            text_blocks.iter().map(block_id).collect::<Vec<_>>(),
            "case {:?}: the PAGE/LINE/WORD layer must match DetectDocumentText",
            case.name
        );

        if case.expected_extra.contains(&"KEY_VALUE_SET") {
            let keys: Vec<&Value> = of_type(&blocks, "KEY_VALUE_SET")
                .into_iter()
                .filter(|block| {
                    block
                        .get("EntityTypes")
                        .and_then(Value::as_array)
                        .is_some_and(|types| types.iter().any(|t| t == "KEY"))
                })
                .collect();
            assert_eq!(
                keys.len(),
                2,
                "case {:?}: the document has two labelled fields, got {} KEY blocks",
                case.name,
                keys.len()
            );
            for key in keys {
                let values = related_ids(key, "VALUE");
                assert_eq!(
                    values.len(),
                    1,
                    "case {:?}: a KEY must point at exactly one VALUE, got {values:?}",
                    case.name
                );
                let value = find(&blocks, &values[0]).expect("the VALUE block");
                assert!(
                    value
                        .get("EntityTypes")
                        .and_then(Value::as_array)
                        .is_some_and(|types| types.iter().any(|t| t == "VALUE")),
                    "case {:?}: {} must be a VALUE KEY_VALUE_SET, got {value}",
                    case.name,
                    values[0]
                );
                // The key and the value parent the words of the real text they read.
                let key_words: Vec<String> = child_ids(key)
                    .iter()
                    .map(|id| block_text(find(&blocks, id).expect("a key word")).to_string())
                    .collect();
                let value_words: Vec<String> = child_ids(value)
                    .iter()
                    .map(|id| block_text(find(&blocks, id).expect("a value word")).to_string())
                    .collect();
                assert!(
                    key_words.last().is_some_and(|word| word.ends_with(':')),
                    "case {:?}: a KEY's last word must be the label, got {key_words:?}",
                    case.name
                );
                assert!(
                    !value_words.is_empty(),
                    "case {:?}: a VALUE must parent the words it is built from",
                    case.name
                );
                assert!(
                    ANALYSIS_DOCUMENT.contains(&value_words.join(" ")),
                    "case {:?}: the VALUE text {value_words:?} must come from the document",
                    case.name
                );
            }
        }

        if case.expected_extra.contains(&"TABLE") {
            let tables = of_type(&blocks, "TABLE");
            assert_eq!(
                tables.len(),
                1,
                "case {:?}: the document has one tab-separated run, got {} TABLE blocks",
                case.name,
                tables.len()
            );
            let cells = child_ids(tables[0]);
            assert_eq!(
                cells.len(),
                9,
                "case {:?}: a 3x3 run must yield 9 CELL blocks, got {}",
                case.name,
                cells.len()
            );
            let mut seen: Vec<(u64, u64)> = Vec::new();
            for id in &cells {
                let cell = find(&blocks, id).expect("the CELL block");
                let row = cell.get("RowIndex").and_then(Value::as_u64).unwrap_or(0);
                let column = cell.get("ColumnIndex").and_then(Value::as_u64).unwrap_or(0);
                assert!(
                    (1..=3).contains(&row) && (1..=3).contains(&column),
                    "case {:?}: CELL {id} is at {row}/{column}, outside the 1-based 3x3 grid",
                    case.name
                );
                assert_eq!(
                    cell.get("RowSpan").and_then(Value::as_u64),
                    Some(1),
                    "case {:?}: CELL {id} must span one row, got {:?}",
                    case.name,
                    cell.get("RowSpan")
                );
                assert!(
                    !seen.contains(&(row, column)),
                    "case {:?}: two cells claim row {row} column {column}",
                    case.name
                );
                seen.push((row, column));
            }
        }
    }
}

/// A line can be far wider than the page it is read off, in every feature set: the
/// layout must saturate rather than panic, and the geometry must stay on the page.
#[tokio::test]
async fn a_document_wider_than_the_page_grid_still_lays_out() {
    let router = harness();
    let long_word = "L".repeat(300);
    let many_words: String = (0..200)
        .map(|i| format!("w{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let wide_row: String = (0..40)
        .map(|i| format!("c{i} x"))
        .collect::<Vec<_>>()
        .join("\t");
    let text = format!("{long_word}\n{many_words}\nLabel: {long_word}\n{wide_row}\n{wide_row}");
    // Drawn at the font's own size: the page is as wide as its longest line of ink, and
    // this one is hundreds of characters long.
    let page = crate::raster_documents::render(
        &drawn_lines(&text)
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        crate::raster_documents::Paper {
            scale: 1,
            ..crate::raster_documents::Paper::plain()
        },
    );
    let document = crate::raster_documents::raster_document(&page, image::ImageFormat::Png);

    for features in [
        Vec::new(),
        vec!["FORMS"],
        vec!["TABLES"],
        vec!["FORMS", "TABLES"],
    ] {
        let case = format!("features {features:?}");
        let (status, body) = if features.is_empty() {
            call(
                &router,
                "DetectDocumentText",
                &json!({ "Document": document }),
            )
            .await
        } else {
            call(
                &router,
                "AnalyzeDocument",
                &json!({ "Document": document, "FeatureTypes": features }),
            )
            .await
        };
        assert_eq!(
            status, 200,
            "case {case:?}: a wide document must be laid out, got {status} with {body}"
        );
        let blocks = blocks_of(&body);
        assert!(
            blocks.len() > 200,
            "case {case:?}: expected a block per word, got {}",
            blocks.len()
        );
        // assert_block_graph checks every bounding box stays inside 0..=1.
        assert_block_graph(&case, &body, &blocks, None);
    }
}

#[tokio::test]
async fn analyze_document_refuses_a_feature_type_that_is_not_modelled() {
    let router = harness();
    let (status, body) = call(
        &router,
        "AnalyzeDocument",
        &json!({
            "Document": text_document("Invoice Number: INV-1"),
            "FeatureTypes": ["NOT_A_FEATURE"],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unmodelled FeatureType must be refused, got {status} with {body}"
    );
}

#[tokio::test]
async fn start_and_get_document_text_detection_round_trip_through_the_job() {
    let router = harness();
    let location = json!({ "S3Object": { "Bucket": "docs", "Name": "invoice.pdf" } });
    let other_location = json!({ "S3Object": { "Bucket": "docs", "Name": "receipt.pdf" } });

    let (status, body) = call(
        &router,
        "StartDocumentTextDetection",
        &json!({ "DocumentLocation": location, "ClientRequestToken": "tok1" }),
    )
    .await;
    assert_eq!(status, 200, "the start call must succeed, got {body}");
    let job_id = body["JobId"].as_str().unwrap_or_default().to_string();
    assert!(
        (1..=64).contains(&job_id.len())
            && job_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "JobId {job_id:?} must satisfy JobId's ^[a-zA-Z0-9-_]+$ and 1..64 length"
    );

    // The job is not done the instant it starts: a caller that polls immediately sees
    // the transient status, and no result.
    let (status, body) = call(
        &router,
        "GetDocumentTextDetection",
        &json!({ "JobId": job_id }),
    )
    .await;
    assert_eq!(status, 200, "the first poll must succeed, got {body}");
    assert_eq!(
        body["JobStatus"].as_str(),
        Some("IN_PROGRESS"),
        "a job polled immediately must still be IN_PROGRESS, got {body}"
    );
    assert!(
        body.get("Blocks").is_none(),
        "an IN_PROGRESS job must report no blocks, got {body}"
    );

    // Idempotency: same token, same document -> same job; different document -> refused.
    let (status, repeat) = call(
        &router,
        "StartDocumentTextDetection",
        &json!({ "DocumentLocation": location, "ClientRequestToken": "tok1" }),
    )
    .await;
    assert_eq!(status, 200, "the repeated start must succeed, got {repeat}");
    assert_eq!(
        repeat["JobId"].as_str(),
        Some(job_id.as_str()),
        "the same ClientRequestToken must return the same JobId, got {repeat}"
    );
    let (status, mismatch) = call(
        &router,
        "StartDocumentTextDetection",
        &json!({ "DocumentLocation": other_location, "ClientRequestToken": "tok1" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "reusing a token for another document must be refused, got {status} with {mismatch}"
    );
    assert_eq!(
        error_code(&mismatch),
        "IdempotentParameterMismatchException",
        "expected IdempotentParameterMismatchException, got {mismatch}"
    );

    // A start with no token mints a NEW job for the same document.
    let (status, untokened) = call(
        &router,
        "StartDocumentTextDetection",
        &json!({ "DocumentLocation": location }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a token-less start must succeed, got {untokened}"
    );
    assert_ne!(
        untokened["JobId"].as_str(),
        Some(job_id.as_str()),
        "a start without a ClientRequestToken must mint a new JobId, got {untokened}"
    );

    std::thread::sleep(PAST_JOB_COMPLETION);

    // No Amazon S3 is mounted beside this service here, so nothing answered the read of
    // the document object and nothing about it is established: the job is created and ends
    // in the modelled FAILED state with a reason — not a success carrying invented text.
    // (`crate::s3_sourced_documents` is where a mounted object store serves the document
    // and the job reaches SUCCEEDED.)
    let (status, body) = call(
        &router,
        "GetDocumentTextDetection",
        &json!({ "JobId": job_id }),
    )
    .await;
    assert_eq!(status, 200, "the settled poll must succeed, got {body}");
    assert_eq!(
        body["JobStatus"].as_str(),
        Some("FAILED"),
        "a job over a document that cannot be read must end FAILED, got {body}"
    );
    assert_eq!(
        body["StatusMessage"].as_str(),
        Some(UNREADABLE_OBJECT_MESSAGE),
        "a FAILED job must say why, in Textract's own wording for an object it could not \
         obtain, got {body}"
    );
    assert!(
        body.get("Blocks").is_none(),
        "a FAILED job must report no blocks, got {body}"
    );
    assert!(
        body.get("NextToken").is_none(),
        "a FAILED job must report no cursor, got {body}"
    );

    // An ill-formed or unknown JobId is refused, never answered with empty blocks.
    for bad in ["doesnotexist", "0123456789abcdef", &"a".repeat(64)] {
        let (status, body) =
            call(&router, "GetDocumentTextDetection", &json!({ "JobId": bad })).await;
        assert_eq!(
            status, 400,
            "case {bad:?}: an unknown JobId must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidJobIdException",
            "case {bad:?}: expected InvalidJobIdException, got {body}"
        );
    }

    // Every cursor the caller can invent is refused, and none of them panics the
    // handler: 32 ASCII characters that do not verify, a 32-BYTE token whose multi-byte
    // character straddles the offset split, and tokens of the wrong length.
    let non_ascii = format!("{}\u{20ac}{}", "a".repeat(23), "b".repeat(6));
    assert_eq!(non_ascii.len(), 32, "the non-ascii token must be 32 bytes");
    for token in [
        "f".repeat(32),
        non_ascii,
        "short".to_string(),
        "f".repeat(64),
        "0000000000000000000000000000000g".to_string(),
    ] {
        for operation in ["GetDocumentTextDetection", "GetDocumentAnalysis"] {
            let (status, body) = call(
                &router,
                operation,
                &json!({ "JobId": job_id, "NextToken": token }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {operation} {token:?}: a cursor the service never issued must be \
                 refused, got {status} with {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_cursor_pages_a_stored_block_list_exactly() {
    // The block list a real DetectDocumentText answer is built from, so the paging rule
    // is asserted over the same values the wire carries: the fixture document DRAWN onto
    // a page and read back off its pixels.
    let bytes = crate::raster_documents::encode(
        &text_page(ANALYSIS_DOCUMENT),
        image::ImageFormat::Png,
    );
    let document = service_com_amazonaws_textract_textract::ops::types::Document {
        bytes: Some(bytes.clone()),
        s3_object: None,
    };
    let reference = state::reference_for_document(&document).expect("a readable document");
    let content = state::read_content(&reference, Some(&bytes)).expect("a readable page");
    let all = state::blocks(
        &reference.identity_key(),
        &content,
        state::FeatureSet {
            forms: true,
            tables: true,
        },
    );
    assert!(
        all.len() > 10,
        "the fixture document must derive enough blocks to page, got {}",
        all.len()
    );
    let job_id = "job-under-test";

    for max_results in [1usize, 2, 7, all.len() - 1, all.len(), all.len() + 5] {
        let mut collected: Vec<String> = Vec::new();
        let mut offset = 0usize;
        let mut rounds = 0usize;
        loop {
            let limit = i32::try_from(max_results).expect("a representable MaxResults");
            let (page, next) = state::page_of_blocks(job_id, &all, Some(limit), offset)
                .expect("a positive MaxResults is a usable page size");
            assert!(
                page.len() <= max_results,
                "case MaxResults={max_results}: a page returned {} blocks",
                page.len()
            );
            collected.extend(page.iter().map(|block| block.id.clone().unwrap_or_default()));
            rounds += 1;
            assert!(
                rounds <= all.len() + 2,
                "case MaxResults={max_results}: paging did not terminate"
            );
            match next {
                None => break,
                Some(token) => {
                    offset = state::decode_page_token(job_id, &token).unwrap_or_else(|| {
                        panic!("case MaxResults={max_results}: the service must accept the cursor it issued ({token})")
                    });
                }
            }
        }
        assert_eq!(
            collected,
            all.iter()
                .map(|block| block.id.clone().unwrap_or_default())
                .collect::<Vec<_>>(),
            "case MaxResults={max_results}: the pages must concatenate to the whole block list"
        );
    }

    // A page size the handler does not implement is refused rather than defaulted.
    for bad in [0i32, -1, i32::MIN] {
        assert!(
            state::page_of_blocks(job_id, &all, Some(bad), 0).is_none(),
            "case MaxResults={bad}: a non-positive page size must be refused"
        );
    }
    // No MaxResults means one page for a list this size.
    let (page, next) = state::page_of_blocks(job_id, &all, None, 0).expect("a default page size");
    assert_eq!(
        page.len(),
        all.len(),
        "without MaxResults the whole list must come back in one page"
    );
    assert!(
        next.is_none(),
        "the last page must carry no cursor, got {next:?}"
    );

    // A cursor is bound to its job: the same offset under another job does not verify.
    let token = state::encode_page_token(job_id, 3);
    assert_eq!(
        state::decode_page_token(job_id, &token),
        Some(3),
        "a cursor must decode to the offset it was issued for"
    );
    assert_eq!(
        state::decode_page_token("another-job", &token),
        None,
        "a cursor issued for one job must not verify against another"
    );
}

#[tokio::test]
async fn get_document_analysis_reports_the_job_its_start_persisted() {
    let router = harness();
    let location = json!({ "S3Object": { "Bucket": "docs", "Name": "contract.pdf" } });

    /// One case: its name and the `FeatureTypes` to request.
    struct AnalysisCase {
        name: &'static str,
        features: Vec<&'static str>,
    }

    let cases = vec![
        AnalysisCase {
            name: "TABLES",
            features: vec!["TABLES"],
        },
        AnalysisCase {
            name: "FORMS",
            features: vec!["FORMS"],
        },
        AnalysisCase {
            name: "FORMS and TABLES",
            features: vec!["FORMS", "TABLES"],
        },
    ];

    let mut job_ids: Vec<(&str, String)> = Vec::new();
    for case in &cases {
        let (status, body) = call(
            &router,
            "StartDocumentAnalysis",
            &json!({
                "DocumentLocation": location,
                "FeatureTypes": case.features,
                "ClientRequestToken": format!("tok-{}", case.features.join("-")),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the start must succeed, got {body}",
            case.name
        );
        let job_id = body["JobId"].as_str().unwrap_or_default().to_string();
        assert!(
            !job_id.is_empty(),
            "case {:?}: the start must return a JobId, got {body}",
            case.name
        );
        job_ids.push((case.name, job_id));
    }

    std::thread::sleep(PAST_JOB_COMPLETION);

    for (name, job_id) in &job_ids {
        let (status, body) = call(&router, "GetDocumentAnalysis", &json!({ "JobId": job_id })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the poll must succeed, got {body}"
        );
        // No Amazon S3 is mounted beside this service here, so nothing answered the read
        // of the document object and nothing about it is established: the job is created
        // and ends in the modelled FAILED state with the reason rather than inventing an
        // analysis. (`crate::s3_sourced_documents` is where a mounted object store serves
        // the document and the job reaches SUCCEEDED.)
        assert_eq!(
            body["JobStatus"].as_str(),
            Some("FAILED"),
            "case {name:?}: expected FAILED, got {body}"
        );
        assert_eq!(
            body["StatusMessage"].as_str(),
            Some(UNREADABLE_OBJECT_MESSAGE),
            "case {name:?}: a FAILED job must say why, in Textract's own wording for an object \
             it could not obtain, got {body}"
        );
        assert!(
            body.get("Blocks").is_none(),
            "case {name:?}: a FAILED job must report no blocks, got {body}"
        );
        assert_eq!(
            body.get("AnalyzeDocumentModelVersion")
                .and_then(Value::as_str),
            Some("1.0"),
            "case {name:?}: expected a model version, got {body}"
        );

        // A text-detection reader must not serve an analysis job.
        let (status, crossed) = call(
            &router,
            "GetDocumentTextDetection",
            &json!({ "JobId": job_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: GetDocumentTextDetection must refuse an analysis job, got {crossed}"
        );
        assert_eq!(
            error_code(&crossed),
            "InvalidJobIdException",
            "case {name:?}: expected InvalidJobIdException, got {crossed}"
        );
    }

    // Every job got its own id.
    for (i, (left_name, left)) in job_ids.iter().enumerate() {
        for (right_name, right) in job_ids.iter().skip(i + 1) {
            assert_ne!(
                left, right,
                "case {left_name:?} vs {right_name:?}: two jobs must not share a JobId"
            );
        }
    }

    // Reusing a token with different FeatureTypes is a mismatch.
    let (status, body) = call(
        &router,
        "StartDocumentAnalysis",
        &json!({
            "DocumentLocation": location,
            "FeatureTypes": ["FORMS", "TABLES"],
            "ClientRequestToken": "tok-TABLES",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "reusing a token with other FeatureTypes must be refused, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "IdempotentParameterMismatchException",
        "expected IdempotentParameterMismatchException, got {body}"
    );

    // An analysis job that was never started is refused.
    let (status, body) = call(
        &router,
        "GetDocumentAnalysis",
        &json!({ "JobId": "doesnotexist" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unknown analysis JobId must be refused, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidJobIdException",
        "expected InvalidJobIdException, got {body}"
    );
}
