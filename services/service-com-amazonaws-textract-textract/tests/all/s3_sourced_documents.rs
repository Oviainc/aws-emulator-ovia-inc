// SPDX-License-Identifier: CC0-1.0

//! Documents submitted as an AMAZON S3 OBJECT rather than as inline bytes, driven end to
//! end with the emulator's own Amazon S3 mounted beside textract on one engine — exactly
//! the way the assembled emulator runs them, and the way a client stages anything larger
//! than the inline limit.
//!
//! THE RULE UNDER TEST: a document is read from the bytes the request names — its own
//! `Bytes`, or the object Amazon S3 holds at the `S3Object` / `DocumentLocation` it gave —
//! and from there BOTH sources are read alike, so every read path answers with the text
//! the document really carries whichever way it arrived. `InvalidS3ObjectException` is
//! reserved for an object the store itself answers for with no such object (a bad bucket,
//! key or version), and an asynchronous `Start*` establishes that at SUBMIT time: a
//! location the store does not hold is refused HTTP 400 with no job created, while a
//! readable one walks `IN_PROGRESS` -> `SUCCEEDED` and reports the document's blocks.
//!
//! Every page here is rendered in the test from known text with the same public-domain
//! 8x8 font the recogniser holds templates for, encoded in a raster format Textract
//! accepts and stored with `PutObject`, so each assertion is a ROUND TRIP through real
//! object bytes and real pixels rather than a comparison against a recorded answer.
//!
//! The tables span what the rule generalises over: which operation reads the document
//! (both synchronous analyses, text detection and both asynchronous starts), which raster
//! format the object holds, how its key is shaped (a bare name, a nested prefix), which
//! part of the address does not resolve (the bucket, the key, a key that only prefixes or
//! only extends a stored one, a version the bucket never held), and what the page carries
//! (a ledger, a receipt, a table).

use std::sync::Arc;
use std::time::Duration;

use image::{GrayImage, ImageFormat};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::document_extraction::{assert_block_graph, blocks_of, error_code, error_message, of_type};
use crate::raster_documents::{encode, render, texts_of, Paper};
use service_com_amazonaws_textract_textract::state;

/// The Region both services are addressed in. Amazon S3 places a bucket created with no
/// `LocationConstraint` in us-east-1 ("If you don't specify a Region, the bucket is
/// created in the US East (N. Virginia) Region (us-east-1) by default"), which is also the
/// Region textract serves a request carrying no credential scope in, so one Region label
/// serves every call here.
const REGION: &str = "us-east-1";

/// A SigV4 credential scope naming Amazon S3, which is what routes a request to the object
/// store. No authenticator is registered, so the signature is never checked; the scope is
/// what claims the request.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The bucket every staged document below lives in.
///
/// `pub(crate)`: `crate::document_formats` stages its own objects in the same bucket,
/// through the helpers below, rather than standing up a second object store.
pub(crate) const BUCKET: &str = "textract-documents";

/// A bucket name no `CreateBucket` here ever creates.
const ABSENT_BUCKET: &str = "textract-documents-missing";

/// Comfortably past `state::JOB_COMPLETION_DELAY`, so a job's terminal transition is due.
/// `tokio`'s `time` feature is not enabled for generated crates, so this blocks the test
/// thread rather than the runtime's timer.
const PAST_JOB_COMPLETION: Duration = Duration::from_millis(900);

/// The message Amazon Textract states `InvalidS3ObjectException` with.
const UNREADABLE_OBJECT_MESSAGE: &str =
    "Unable to get object metadata from S3. Check object key, region and/or access permissions.";

/// One emulator: textract and Amazon S3 on a single shared engine behind the production
/// router — what `build_router` assembles, narrowed to the two services under test.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_textract_textract::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// One textract call in the service's own wire envelope (awsJson1_1), signed for
/// [`REGION`] so the in-process read of Amazon S3 is addressed to the same Region the
/// caller is in.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Textract.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/textract/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One path-style request to the emulator's Amazon S3 front door: the bucket and the key
/// live in the URI and the body is the object's own bytes, so this is a raw request rather
/// than a modelled call. The credential scope names `s3`, which is what claims it.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> u16 {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTHORIZATION)
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    router.dispatch(request).await.status().as_u16()
}

/// `PUT /<bucket>` — `CreateBucket` — so the staged objects below have a bucket to live in.
pub(crate) async fn given_bucket(router: &MultiServiceRouter) {
    let status = s3(router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert_eq!(
        status, 200,
        "the document bucket must be created first, got {status}"
    );
}

/// `PUT /<bucket>/<key>` — `PutObject` — storing `bytes` as the object's own body.
pub(crate) async fn given_object(router: &MultiServiceRouter, key: &str, bytes: &[u8]) {
    let status = s3(router, "PUT", &format!("/{BUCKET}/{key}"), bytes).await;
    assert_eq!(
        status, 200,
        "case {key:?}: the document object must be stored, got {status}"
    );
}

/// Store `page`, encoded in `format`, as the object `key`, and hand back the bytes that
/// were stored.
async fn given_raster_object(
    router: &MultiServiceRouter,
    key: &str,
    page: &GrayImage,
    format: ImageFormat,
) -> Vec<u8> {
    let bytes = encode(page, format);
    given_object(router, key, &bytes).await;
    bytes
}

/// A `Document` naming the staged object `key`.
pub(crate) fn s3_document(key: &str) -> Value {
    json!({ "S3Object": { "Bucket": BUCKET, "Name": key } })
}

/// A `DocumentLocation` naming the staged object `key`.
fn s3_location(key: &str) -> Value {
    json!({ "S3Object": { "Bucket": BUCKET, "Name": key } })
}

/// Collapse runs of whitespace to one space.
///
/// A raster carries no characters, only ink: what separates two words on a page is a GAP,
/// which the recogniser reports as the separator it measured. So the round trip is
/// asserted on the words of each line and the single separator between them, not on the
/// exact whitespace the page was drawn from — a tab drawn as a column gap comes back as
/// one space.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The lines a page drawn from `lines` must be read back as.
fn expected_lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| collapse(line)).collect()
}

/// The LINE texts of `blocks`, collapsed the same way [`expected_lines`] collapses what
/// was drawn.
fn read_lines(blocks: &[Value]) -> Vec<String> {
    texts_of(blocks, "LINE")
        .iter()
        .map(|line| collapse(line))
        .collect()
}

/// One staged document: the key it is stored under, the format it is encoded in, and the
/// lines it was drawn from.
struct Staged {
    case: &'static str,
    key: &'static str,
    format: ImageFormat,
    lines: Vec<&'static str>,
}

/// One asynchronous family a row drives: which start reads which staged object, with which
/// extra members, and the read and model-version member that answer for it.
struct Family<'row> {
    case: &'static str,
    /// The staged object the start points at.
    key: &'static str,
    /// The lines that object carries, which the job's blocks must report.
    lines: &'row [&'static str],
    start: &'static str,
    /// The members this family adds to the start request (`FeatureTypes`).
    extra: Value,
    read: &'static str,
    version_member: &'static str,
}

/// The documents every reading test stages. The rows differ along the raster format, the
/// shape of the key, the size the text is rendered at and what the page carries — a
/// ledger, a receipt with labelled lines, and a tab-separated table.
fn staged_documents() -> Vec<Staged> {
    vec![
        Staged {
            case: "a PNG at the bucket root",
            key: "ledger.png",
            format: ImageFormat::Png,
            lines: vec!["SIMPARITY LEDGER", "Account Number: 48213"],
        },
        Staged {
            case: "a JPEG under a nested prefix",
            key: "scans/2026/receipt.jpg",
            format: ImageFormat::Jpeg,
            lines: vec!["VENDOR NAME: ACME", "TOTAL: 1250.00"],
        },
        Staged {
            case: "a TIFF under a deeper prefix",
            key: "scans/2026/march/invoice.tiff",
            format: ImageFormat::Tiff,
            lines: vec!["INVOICE 7781", "Due Date: 2026-11-01"],
        },
    ]
}

/// The paper every staged page is drawn on: large enough for the recogniser to read the
/// characters back exactly, which is what makes the round trip an assertion about the
/// bytes that were stored.
fn paper() -> Paper {
    Paper {
        scale: 4,
        ..Paper::plain()
    }
}

/// RULE: a document named as an S3 object is read from the object the store holds, by
/// every synchronous read path, and answers with the text that object's pixels really
/// carry — the same text the same bytes answer with inline.
#[tokio::test]
async fn every_synchronous_path_reads_the_object_amazon_s3_holds() {
    let router = emulator();
    given_bucket(&router).await;

    for staged in staged_documents() {
        let page = render(&staged.lines, paper());
        let bytes = given_raster_object(&router, staged.key, &page, staged.format).await;
        let inline = json!({ "Bytes": aws_smithy_types::base64::encode(&bytes) });
        let drawn = expected_lines(&staged.lines);

        // (the operation, the request it is given for an S3-sourced document, the one for
        // the same bytes inline)
        let requests: Vec<(&str, Value, Value)> = vec![
            (
                "DetectDocumentText",
                json!({ "Document": s3_document(staged.key) }),
                json!({ "Document": inline.clone() }),
            ),
            (
                "AnalyzeDocument",
                json!({
                    "Document": s3_document(staged.key),
                    "FeatureTypes": ["TABLES", "FORMS"],
                }),
                json!({ "Document": inline.clone(), "FeatureTypes": ["TABLES", "FORMS"] }),
            ),
            (
                "AnalyzeExpense",
                json!({ "Document": s3_document(staged.key) }),
                json!({ "Document": inline.clone() }),
            ),
        ];

        for (operation, from_s3, from_bytes) in requests {
            let case = format!("{} / {operation}", staged.case);
            let (status, body) = call(&router, operation, &from_s3).await;
            assert_eq!(
                status, 200,
                "case {case:?}: an object the store holds must be read, got {status} with {body}"
            );
            assert_eq!(
                body["DocumentMetadata"]["Pages"].as_u64(),
                Some(1),
                "case {case:?}: one raster is one page, got {body}"
            );

            // The round trip: the lines read out of the stored object are the lines the
            // page was drawn from.
            let blocks = if operation == "AnalyzeExpense" {
                let documents = body
                    .get("ExpenseDocuments")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assert_eq!(
                    documents.len(),
                    1,
                    "case {case:?}: expected one expense document per page, got {body}"
                );
                // Cause 1's shape: a labelled line of the document is a SummaryField whose
                // text comes from the page that was drawn.
                let summary = documents[0]
                    .get("SummaryFields")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    !summary.is_empty(),
                    "case {case:?}: a page of labelled lines must report SummaryFields, got {}",
                    documents[0]
                );
                for field in &summary {
                    let label = field["LabelDetection"]["Text"].as_str().unwrap_or_default();
                    assert!(
                        drawn
                            .iter()
                            .any(|line| line.to_uppercase().contains(&label.to_uppercase())),
                        "case {case:?}: the summary field label {label:?} is no part of the \
                         document that was drawn ({drawn:?})"
                    );
                }
                documents[0]
                    .get("Blocks")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            } else {
                blocks_of(&body)
            };
            assert_block_graph(&case, &body, &blocks, None);
            assert_eq!(
                read_lines(&blocks),
                drawn,
                "case {case:?}: the LINE blocks must report the text the stored object was \
                 drawn from"
            );

            // One document, read ONE way: the same bytes submitted inline report the same
            // text (the block ids differ, because they are derived from the document's own
            // identity — an object's location, or the digest of the inline bytes). A TIFF
            // is where the two channels part: `Document$Bytes` carries a document "in PNG
            // or JPEG format", so the same bytes inline are refused as a document format
            // that channel does not take.
            let (status, inline_body) = call(&router, operation, &from_bytes).await;
            if staged.format == ImageFormat::Tiff {
                assert_eq!(
                    status, 400,
                    "case {case:?}: a TIFF inline is not a document Document$Bytes carries, \
                     got {status} with {inline_body}"
                );
                assert_eq!(
                    error_code(&inline_body),
                    "UnsupportedDocumentException",
                    "case {case:?}: expected UnsupportedDocumentException, got {inline_body}"
                );
                continue;
            }
            assert_eq!(
                status, 200,
                "case {case:?}: the same bytes inline must be read too, got {status} with \
                 {inline_body}"
            );
            let inline_blocks = if operation == "AnalyzeExpense" {
                inline_body["ExpenseDocuments"][0]
                    .get("Blocks")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            } else {
                blocks_of(&inline_body)
            };
            assert_eq!(
                texts_of(&inline_blocks, "LINE"),
                texts_of(&blocks, "LINE"),
                "case {case:?}: an object and the same bytes inline must read the same lines"
            );
            assert_eq!(
                texts_of(&inline_blocks, "WORD"),
                texts_of(&blocks, "WORD"),
                "case {case:?}: an object and the same bytes inline must read the same words"
            );
        }
    }
}

/// RULE: a synchronous read of a location the object store answers for with NO SUCH OBJECT
/// is refused with the modelled `InvalidS3ObjectException` (HTTP 400), whichever part of
/// the address does not resolve.
#[tokio::test]
async fn a_location_the_store_does_not_hold_is_an_invalid_s3_object() {
    let router = emulator();
    given_bucket(&router).await;
    let page = render(&["SIMPARITY LEDGER"], paper());
    given_raster_object(&router, "ledger.png", &page, ImageFormat::Png).await;

    // (case, the S3Object the request names)
    let addresses: Vec<(&str, Value)> = vec![
        (
            "a bucket no create made",
            json!({ "Bucket": ABSENT_BUCKET, "Name": "ledger.png" }),
        ),
        (
            "a key nothing was stored under",
            json!({ "Bucket": BUCKET, "Name": "does-not-exist.png" }),
        ),
        (
            "a key that only prefixes a stored one",
            json!({ "Bucket": BUCKET, "Name": "ledger" }),
        ),
        (
            "a key that only extends a stored one",
            json!({ "Bucket": BUCKET, "Name": "ledger.png.bak" }),
        ),
        (
            "a stored key under a prefix it does not live under",
            json!({ "Bucket": BUCKET, "Name": "scans/ledger.png" }),
        ),
        (
            "a version the bucket never held",
            json!({ "Bucket": BUCKET, "Name": "ledger.png", "Version": "nosuchversion" }),
        ),
    ];

    for (case, object) in addresses {
        for operation in ["DetectDocumentText", "AnalyzeExpense", "AnalyzeDocument"] {
            let case = format!("{case} / {operation}");
            let mut body = json!({ "Document": { "S3Object": object.clone() } });
            if operation == "AnalyzeDocument" {
                body["FeatureTypes"] = json!(["FORMS"]);
            }
            let (status, answer) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: an object the store does not hold must be refused, got {status} \
                 with {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "InvalidS3ObjectException",
                "case {case:?}: expected InvalidS3ObjectException, got {answer}"
            );
            assert_eq!(
                error_message(&answer),
                UNREADABLE_OBJECT_MESSAGE,
                "case {case:?}: expected Textract's wording for an unreadable object, got {answer}"
            );
            assert!(
                answer.get("Blocks").is_none() && answer.get("ExpenseDocuments").is_none(),
                "case {case:?}: a refusal must carry no result, got {answer}"
            );
        }
    }
}

/// RULE: an asynchronous start validates the document's LOCATION at submit time — a
/// location the object store answers for with no such object is refused HTTP 400
/// `InvalidS3ObjectException` and NO job is created, so nothing can be polled for it.
#[tokio::test]
async fn an_asynchronous_start_refuses_a_location_the_store_does_not_hold() {
    let router = emulator();
    given_bucket(&router).await;
    let page = render(&["SIMPARITY LEDGER"], paper());
    given_raster_object(&router, "ledger.png", &page, ImageFormat::Png).await;

    // (case, the S3Object the DocumentLocation names)
    let addresses: Vec<(&str, Value)> = vec![
        (
            "a bucket no create made",
            json!({ "Bucket": ABSENT_BUCKET, "Name": "ledger.png" }),
        ),
        (
            "a key nothing was stored under",
            json!({ "Bucket": BUCKET, "Name": "sim2real-does-not-exist.png" }),
        ),
        (
            "a key that only prefixes a stored one",
            json!({ "Bucket": BUCKET, "Name": "ledger" }),
        ),
        (
            "a key under a prefix nothing lives under",
            json!({ "Bucket": BUCKET, "Name": "scans/2026/ledger.png" }),
        ),
    ];

    for (case, object) in addresses {
        for (operation, extra) in [
            ("StartDocumentTextDetection", json!({})),
            (
                "StartDocumentAnalysis",
                json!({ "FeatureTypes": ["TABLES"] }),
            ),
        ] {
            let case = format!("{case} / {operation}");
            let mut body = json!({ "DocumentLocation": { "S3Object": object.clone() } });
            if let Some(members) = extra.as_object() {
                for (member, value) in members {
                    body[member] = value.clone();
                }
            }
            let (status, answer) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a start over an object the store does not hold must be refused, \
                 got {status} with {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "InvalidS3ObjectException",
                "case {case:?}: expected InvalidS3ObjectException, got {answer}"
            );
            assert!(
                answer.get("JobId").is_none(),
                "case {case:?}: a refused start must create no job, got {answer}"
            );
        }
    }
}

/// RULE: an asynchronous job over a document the object store serves walks `IN_PROGRESS`
/// -> `SUCCEEDED` and its terminal read reports the blocks of that document, with the
/// feature types the start asked for.
#[tokio::test]
async fn an_asynchronous_job_over_a_readable_object_succeeds_with_its_blocks() {
    let router = emulator();
    given_bucket(&router).await;

    // Two staged objects, so the rows differ along what the OBJECT holds as well as which
    // family reads it: a rendered page of labelled lines, and a page whose columns are the
    // table the TABLES feature reports — the second encoded as a TIFF, a document format
    // only an Amazon S3 object carries. Both arrive the same way, as the body of a
    // `PutObject`.
    let rendered = vec!["SIMPARITY LEDGER", "Account Number: 48213"];
    let page = render(&rendered, paper());
    given_raster_object(&router, "ledger.png", &page, ImageFormat::Png).await;
    let tabulated = vec![
        "SIMPARITY LEDGER",
        "Item Code\tAmount Due",
        "Freight Charge\t250.00",
        "Total Payable\t1250.00",
    ];
    let drawn = crate::document_extraction::drawn_lines(&tabulated.join("\n"));
    let tabulated_page = render(
        &drawn.iter().map(String::as_str).collect::<Vec<_>>(),
        paper(),
    );
    given_raster_object(&router, "ledger.tiff", &tabulated_page, ImageFormat::Tiff).await;

    // (case, the object, its lines, the start, its extra members, the matching read, the
    // model version it reports)
    let families: Vec<Family> = vec![
        Family {
            case: "text detection over a rendered page",
            key: "ledger.png",
            lines: &rendered,
            start: "StartDocumentTextDetection",
            extra: json!({}),
            read: "GetDocumentTextDetection",
            version_member: "DetectDocumentTextModelVersion",
        },
        Family {
            case: "text detection over a TIFF of a table",
            key: "ledger.tiff",
            lines: &tabulated,
            start: "StartDocumentTextDetection",
            extra: json!({}),
            read: "GetDocumentTextDetection",
            version_member: "DetectDocumentTextModelVersion",
        },
        Family {
            case: "analysis with TABLES",
            key: "ledger.tiff",
            lines: &tabulated,
            start: "StartDocumentAnalysis",
            extra: json!({ "FeatureTypes": ["TABLES"] }),
            read: "GetDocumentAnalysis",
            version_member: "AnalyzeDocumentModelVersion",
        },
        Family {
            case: "analysis with TABLES and FORMS",
            key: "ledger.tiff",
            lines: &tabulated,
            start: "StartDocumentAnalysis",
            extra: json!({ "FeatureTypes": ["TABLES", "FORMS"] }),
            read: "GetDocumentAnalysis",
            version_member: "AnalyzeDocumentModelVersion",
        },
        Family {
            case: "analysis with FORMS over a rendered page",
            key: "ledger.png",
            lines: &rendered,
            start: "StartDocumentAnalysis",
            extra: json!({ "FeatureTypes": ["FORMS"] }),
            read: "GetDocumentAnalysis",
            version_member: "AnalyzeDocumentModelVersion",
        },
    ];

    for Family {
        case,
        key,
        lines,
        start,
        extra,
        read,
        version_member,
    } in families
    {
        let mut body = json!({ "DocumentLocation": s3_location(key) });
        if let Some(members) = extra.as_object() {
            for (member, value) in members {
                body[member] = value.clone();
            }
        }
        let (status, answer) = call(&router, start, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a start over a readable object must be accepted, got {status} with \
             {answer}"
        );
        let job_id = answer["JobId"].as_str().unwrap_or_default().to_string();
        assert!(
            !job_id.is_empty(),
            "case {case:?}: an accepted start must answer with a JobId, got {answer}"
        );

        // The job is not done the instant it starts.
        let (status, polled) = call(&router, read, &json!({ "JobId": job_id })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first poll must succeed, got {polled}"
        );
        assert_eq!(
            polled["JobStatus"].as_str(),
            Some("IN_PROGRESS"),
            "case {case:?}: a job polled immediately must still be IN_PROGRESS, got {polled}"
        );

        std::thread::sleep(PAST_JOB_COMPLETION);

        let (status, settled) = call(&router, read, &json!({ "JobId": job_id })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the settled poll must succeed, got {settled}"
        );
        assert_eq!(
            settled["JobStatus"].as_str(),
            Some("SUCCEEDED"),
            "case {case:?}: a job over a readable object must reach SUCCEEDED, got {settled}"
        );
        assert!(
            settled.get("StatusMessage").is_none(),
            "case {case:?}: a job that succeeded states no failure reason, got {settled}"
        );
        assert!(
            settled[version_member]
                .as_str()
                .is_some_and(|version| !version.is_empty()),
            "case {case:?}: expected a non-empty {version_member}, got {settled}"
        );
        let blocks = blocks_of(&settled);
        assert_block_graph(case, &settled, &blocks, None);
        assert_eq!(
            read_lines(&blocks),
            expected_lines(lines),
            "case {case:?}: the job's LINE blocks must report the text the stored object \
             carries"
        );
        if body["FeatureTypes"]
            .as_array()
            .is_some_and(|features| features.iter().any(|feature| feature == "TABLES"))
        {
            let tables = of_type(&blocks, "TABLE");
            assert!(
                !tables.is_empty(),
                "case {case:?}: a TABLES job must report a TABLE block, got {:?}",
                blocks
                    .iter()
                    .map(|block| block["BlockType"].clone())
                    .collect::<Vec<_>>()
            );
            assert!(
                !of_type(&blocks, "CELL").is_empty(),
                "case {case:?}: a TABLE block must carry CELL children, got {blocks:?}"
            );
        }
    }
}

/// RULE: the bytes a read reports are the object's CURRENT bytes — an S3 object is
/// identified by its location, so a `PutObject` that replaces it is what the next read of
/// that location reports, and no stale text is kept for it.
#[tokio::test]
async fn a_replaced_object_is_read_again_rather_than_reported_from_before() {
    let router = emulator();
    given_bucket(&router).await;

    let first = vec!["SIMPARITY LEDGER", "Account Number: 48213"];
    let second = vec!["REVISED LEDGER", "Account Number: 99100"];
    let key = "ledger.png";

    for (case, lines) in [("the first write", &first), ("the replacement", &second)] {
        let page = render(lines, paper());
        given_raster_object(&router, key, &page, ImageFormat::Png).await;

        let (status, body) = call(
            &router,
            "DetectDocumentText",
            &json!({ "Document": s3_document(key) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored object must be read, got {status} with {body}"
        );
        assert_eq!(
            read_lines(&blocks_of(&body)),
            expected_lines(lines),
            "case {case:?}: the read must report the object's current bytes, got {body}"
        );

        // The asynchronous path reads the same object, so it reports the same text.
        let (status, started) = call(
            &router,
            "StartDocumentTextDetection",
            &json!({ "DocumentLocation": s3_location(key) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the start must be accepted, got {status} with {started}"
        );
        let job_id = started["JobId"].as_str().unwrap_or_default().to_string();
        std::thread::sleep(PAST_JOB_COMPLETION);
        let (status, settled) = call(
            &router,
            "GetDocumentTextDetection",
            &json!({ "JobId": job_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the settled poll must succeed, got {settled}"
        );
        assert_eq!(
            read_lines(&blocks_of(&settled)),
            expected_lines(lines),
            "case {case:?}: the job must report the object's current bytes, got {settled}"
        );
    }
}

/// RULE: a document is held to the ceiling documented for the SOURCE it arrived from — the
/// 5 MB AWS states for `Document$Bytes` and the 10 MB it states for a document a
/// synchronous operation reads, which this emulator applies to an S3-sourced document on
/// both paths because it reads the whole document into this process and recognises it there
/// (see `state::MAX_S3_DOCUMENT_BYTES`). A document over its ceiling is refused as too
/// large rather than read in part.
///
/// Asserted through `state::read_content`, the one decode every read path reaches: the
/// object store is not asked to hold megabytes for it, and what is pinned is the bound
/// itself — including the DIVERGENCE the constant documents, so an object between the
/// synchronous ceiling and the 500 MB AWS publishes for an asynchronous document is a
/// refusal this emulator makes knowingly.
#[test]
fn each_document_source_is_held_to_the_ceiling_documented_for_it() {
    use service_com_amazonaws_textract_textract::ops::types::{Document, S3Object};

    let inline_reference = |bytes: &[u8]| {
        state::reference_for_document(&Document {
            bytes: Some(bytes.to_vec()),
            s3_object: None,
        })
        .expect("inline bytes are a document source")
    };
    let object_reference = state::reference_for_document(&Document {
        bytes: None,
        s3_object: Some(S3Object {
            bucket: Some(BUCKET.to_string()),
            name: Some("ledger.png".to_string()),
            version: None,
        }),
    })
    .expect("an S3 object is a document source");

    // (case, the size, whether the source is an S3 object, whether it must be read)
    let rows: &[(&str, usize, bool, bool)] = &[
        ("inline bytes well inside their ceiling", 1024, false, true),
        (
            "inline bytes at their documented ceiling",
            state::MAX_INLINE_DOCUMENT_BYTES,
            false,
            true,
        ),
        (
            "inline bytes one byte over their ceiling",
            state::MAX_INLINE_DOCUMENT_BYTES + 1,
            false,
            false,
        ),
        (
            // The SAME size an inline document is refused at: an object is held to the
            // larger ceiling, so the two sources do not share one bound.
            "an object over the inline ceiling but inside its own",
            state::MAX_INLINE_DOCUMENT_BYTES + 1,
            true,
            true,
        ),
        (
            "an object at its documented ceiling",
            state::MAX_S3_DOCUMENT_BYTES,
            true,
            true,
        ),
        (
            "an object one byte over its ceiling",
            state::MAX_S3_DOCUMENT_BYTES + 1,
            true,
            false,
        ),
    ];

    for (case, size, from_an_object, must_be_read) in rows {
        // A page of real pixels, padded out to the size under test: what decides the
        // outcome is the SIZE, so the bytes are a document the service would otherwise
        // read (a PNG; the padding sits past the stream's end marker).
        let page = encode(&render(&["SIMPARITY LEDGER"], paper()), ImageFormat::Png);
        assert!(
            page.len() <= *size,
            "case {case:?}: the drawn page must fit inside the size under test"
        );
        let mut bytes = page;
        bytes.resize(*size, 0);
        let reference = if *from_an_object {
            object_reference.clone()
        } else {
            inline_reference(&bytes)
        };
        let read = state::read_content(&reference, Some(&bytes));
        match (must_be_read, &read) {
            (true, Ok(content)) => assert_eq!(
                content.page_count(),
                1,
                "case {case:?}: a document inside its ceiling is read as one page, got {:?}",
                content.page_count()
            ),
            (false, Err(state::Unreadable::TooLarge(message))) => assert!(
                message.contains(&size.to_string()),
                "case {case:?}: the refusal must name the size that was submitted, got \
                 {message:?}"
            ),
            (_, answered) => panic!(
                "case {case:?}: expected the document to be {}, got {answered:?}",
                if *must_be_read {
                    "read"
                } else {
                    "refused as too large"
                }
            ),
        }
    }
}
