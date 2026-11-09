// SPDX-License-Identifier: CC0-1.0

//! The WORDING of a Textract refusal, end to end over the wire (`awsJson1_1`, `POST /`
//! with `X-Amz-Target: Textract.<Operation>`).
//!
//! THE RULE UNDER TEST: a refusal answers the modelled exception Amazon Textract defines
//! for the condition, and its message is that condition's own FIXED sentence — the same
//! string on every call, whichever operation refused and whatever the request held, with
//! nothing taken from the request interpolated into it:
//!
//! * a document no page can be read out of — bytes in no format the service reads, bytes
//!   in a document format the channel they arrived on does not carry, or bytes in an
//!   accepted format that its decoder cannot read — is `UnsupportedDocumentException`
//!   with [`UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE`];
//! * `QueriesConfig` supplied without the `QUERIES` `FeatureType` is
//!   `InvalidParameterException` with [`QUERIES_CONFIG_WITHOUT_FEATURE_MESSAGE`];
//! * an input that violates a modelled constraint (an absent required member, or a value
//!   outside a declared length, pattern or range) is `InvalidParameterException` with
//!   [`INVALID_PARAMETERS_MESSAGE`], naming neither the member nor the value;
//! * a `JobId` no `Start*` of this service issued is `InvalidJobIdException` with
//!   [`INVALID_JOB_ID_MESSAGE`].
//!
//! Each table spans what the rule generalises over — which operation refused, which kind
//! of unreadable bytes or violated constraint, and which value the request carried — and
//! each refusal is also asserted to quote NOTHING of the request back. Every table
//! carries the positive control for its rule: an input just inside the same boundary is
//! served, so none of these refusals is unconditional.

use image::ImageFormat;
use serde_json::{json, Value};
use service_com_amazonaws_textract_textract::state;

use crate::document_extraction::{call, error_code, error_message, harness, text_document};
use crate::queries_and_input_refusals::s3_location;
use crate::raster_documents::{encode, render, Paper};

/// The sentence Amazon Textract states `UnsupportedDocumentException` with: fixed text,
/// with no part of it taken from the request.
const UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE: &str = "Request has unsupported document format";

/// The sentence Amazon Textract states `InvalidParameterException` with for an input that
/// violates a modelled constraint.
const INVALID_PARAMETERS_MESSAGE: &str = "Request has invalid parameters";

/// The sentence Amazon Textract states `InvalidJobIdException` with.
const INVALID_JOB_ID_MESSAGE: &str = "Request has invalid Job Id";

/// The sentence Amazon Textract refuses `QueriesConfig` without the `QUERIES`
/// `FeatureType` with, to its trailing period.
const QUERIES_CONFIG_WITHOUT_FEATURE_MESSAGE: &str =
    "QueriesConfig must be used with QUERIES FeatureType.";

/// A document whose text is known from its own characters, for the rows that need a
/// readable document.
const INVOICE: &str = "Invoice Number: INV-2048\nTotal Due: 17.25";

/// Assert one refusal: the status, the modelled code and the exact sentence.
fn assert_refusal(case: &str, status: u16, body: &Value, code: &str, message: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        code,
        "case {case:?}: expected {code}, got {body}"
    );
    assert_eq!(
        error_message(body),
        message,
        "case {case:?}: expected the message {message:?}, got {:?}",
        error_message(body)
    );
}

/// A page of real pixels, encoded in `format`, as the bytes a request carries.
fn page_bytes(format: ImageFormat) -> Vec<u8> {
    let page = render(
        &["SIMPARITY LEDGER", "Total 42.50"],
        Paper {
            scale: 3,
            ..Paper::plain()
        },
    );
    encode(&page, format)
}

#[tokio::test]
async fn a_document_no_page_can_be_read_out_of_is_refused_as_an_unsupported_format() {
    let router = harness();

    // Rows span BOTH halves of the class: bytes in one of the raster formats Textract
    // accepts (PNG, JPEG, TIFF per `UnsupportedDocumentException`'s own documentation:
    // "Documents for operations can be in PNG, JPEG, PDF, or TIFF format") whose stream
    // is cut at several points, and bytes in no format this service reads at all. The
    // service draws no distinction between them, so one code and one sentence answer
    // every row.
    let png = page_bytes(ImageFormat::Png);
    let jpeg = page_bytes(ImageFormat::Jpeg);
    let tiff = page_bytes(ImageFormat::Tiff);

    let cases: Vec<(&str, Value)> = vec![
        (
            "a PNG truncated to its signature",
            bytes_document(&png[..8]),
        ),
        ("a PNG truncated mid-stream", bytes_document(&png[..64])),
        (
            "a PNG truncated to a quarter of its length",
            bytes_document(&png[..png.len() / 4]),
        ),
        ("a JPEG truncated mid-stream", bytes_document(&jpeg[..64])),
        (
            "a JPEG truncated to its first marker",
            bytes_document(&jpeg[..16]),
        ),
        ("a TIFF truncated mid-stream", bytes_document(&tiff[..64])),
        (
            "a TIFF truncated to half its length",
            bytes_document(&tiff[..tiff.len() / 2]),
        ),
        (
            // A GIF is a real raster format, and not one Textract accepts.
            "a GIF, a raster format Textract does not accept",
            bytes_document(b"GIF89a\x01\x00\x01\x00\x00\x00\x00;"),
        ),
        (
            // Textract accepts PDF; this emulator renders no PDF page, so it reads
            // nothing out of one.
            "a PDF, which this emulator does not decode",
            bytes_document(b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n%%EOF\n"),
        ),
        (
            "bytes in no format at all",
            bytes_document(&[0x00u8, 0x01, 0x02, 0xFF, 0xFE, 0x7F]),
        ),
        (
            // Characters are not one of the document formats Textract processes.
            "a text document",
            bytes_document(INVOICE.as_bytes()),
        ),
        (
            // `Document$Bytes` carries a document "in PNG or JPEG format"; a TIFF is one
            // only an Amazon S3 object may hold.
            "a whole TIFF, which inline Bytes do not carry",
            bytes_document(&tiff),
        ),
    ];

    for (name, document) in cases {
        // Every synchronous read path of this service, so the classification is the
        // service's and not one handler's.
        for (operation, request) in [
            (
                "DetectDocumentText",
                json!({ "Document": document.clone() }),
            ),
            (
                "AnalyzeDocument",
                json!({ "Document": document.clone(), "FeatureTypes": ["FORMS"] }),
            ),
            ("AnalyzeExpense", json!({ "Document": document.clone() })),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            let case = format!("{name} on {operation}");
            assert_refusal(
                &case,
                status,
                &body,
                "UnsupportedDocumentException",
                UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE,
            );
        }
    }

    // The positive control for the same rule: the WHOLE page those rows were cut from is
    // read, so the refusal is about bytes no page can be read out of and not about the
    // format itself.
    for format in [ImageFormat::Png, ImageFormat::Jpeg] {
        let (status, body) = call(
            &router,
            "DetectDocumentText",
            &json!({ "Document": bytes_document(&page_bytes(format)) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {format:?}: the untruncated page must be read, got {status} with {body}"
        );
    }
}

/// A `Document` carrying `bytes` inline.
fn bytes_document(bytes: &[u8]) -> Value {
    json!({ "Bytes": aws_smithy_types::base64::encode(bytes) })
}

#[tokio::test]
async fn queries_config_without_the_queries_feature_is_refused_in_textracts_own_wording() {
    let router = harness();

    // `FEATURE_TYPE_VALUES` is the FeatureType enum of the model; the rows are the
    // modelled values OTHER than QUERIES, alone and combined, over both operations that
    // take the pair and over more than one query list.
    let configs: Vec<(&str, Value)> = vec![
        (
            "one query",
            json!({ "Queries": [{ "Text": "What is the invoice number?" }] }),
        ),
        (
            "one aliased query",
            json!({ "Queries": [{
                "Text": "What is the invoice number?",
                "Alias": "INVOICE_NUMBER",
            }] }),
        ),
        (
            "three queries, one of them page-filtered",
            json!({ "Queries": [
                { "Text": "What is the total due?" },
                { "Text": "Who is the vendor?", "Pages": ["1"] },
                { "Text": "What is the invoice number?" },
            ] }),
        ),
    ];

    for feature_types in [
        json!(["FORMS"]),
        json!(["TABLES"]),
        json!(["FORMS", "TABLES"]),
        json!(["SIGNATURES"]),
        json!(["LAYOUT"]),
    ] {
        for (config_name, config) in &configs {
            for (operation, mut request) in [
                (
                    "AnalyzeDocument",
                    json!({ "Document": text_document(INVOICE), "FeatureTypes": feature_types }),
                ),
                (
                    "StartDocumentAnalysis",
                    json!({ "DocumentLocation": s3_location(), "FeatureTypes": feature_types }),
                ),
            ] {
                request["QueriesConfig"] = config.clone();
                let (status, body) = call(&router, operation, &request).await;
                let case = format!("{operation} with {feature_types} and {config_name}");
                assert_refusal(
                    &case,
                    status,
                    &body,
                    "InvalidParameterException",
                    QUERIES_CONFIG_WITHOUT_FEATURE_MESSAGE,
                );
                assert!(
                    !body.to_string().contains("invoice number"),
                    "case {case:?}: the refusal must quote none of the queries back, got {body}"
                );
            }
        }
    }

    // The positive control: the very same `QueriesConfig`, asked for with the feature it
    // pairs with, is served.
    for (config_name, config) in &configs {
        let (status, body) = call(
            &router,
            "AnalyzeDocument",
            &json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["QUERIES"],
                "QueriesConfig": config,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {config_name:?}: QueriesConfig with the QUERIES feature must be served, \
             got {status} with {body}"
        );
    }
}

/// One row of the constraint-violation table: the request, and the caller-supplied value
/// the refusal must not quote back.
struct ViolationCase {
    name: &'static str,
    operation: &'static str,
    request: Value,
    /// The offending value, where the row carries one in a member.
    offending: Option<String>,
}

#[tokio::test]
async fn a_modelled_constraint_violation_is_refused_in_textracts_own_wording() {
    let router = harness();

    // Member constraints from smithy_query: CreateAdapterRequest$AdapterName @length
    // 1..128 @pattern ^[a-zA-Z0-9-_]+$ and $FeatureTypes required;
    // UpdateAdapterRequest$AdapterName the same pattern and $AdapterId required @length
    // 12..1011; GetAdapterRequest$AdapterId required; ListAdaptersRequest$MaxResults
    // @range min 1; GetDocumentTextDetectionRequest$JobId required @length 1..64 @pattern
    // ^[a-zA-Z0-9-_]+$. The rows span the kinds of violation (pattern, length under and
    // over, absent required member, range) across five operations.
    let outside_pattern = "sim2real adapter 7!!invalid!!";
    let over_length = "a".repeat(129);
    let over_job_id_length = "j".repeat(65);
    let cases = vec![
        ViolationCase {
            name: "AdapterName outside the pattern",
            operation: "CreateAdapter",
            request: json!({ "AdapterName": outside_pattern, "FeatureTypes": ["QUERIES"] }),
            offending: Some(outside_pattern.to_string()),
        },
        ViolationCase {
            name: "AdapterName with a character just outside the pattern",
            operation: "CreateAdapter",
            request: json!({ "AdapterName": "adapter.one", "FeatureTypes": ["QUERIES"] }),
            offending: Some("adapter.one".to_string()),
        },
        ViolationCase {
            name: "AdapterName under the minimum length",
            operation: "CreateAdapter",
            request: json!({ "AdapterName": "", "FeatureTypes": ["QUERIES"] }),
            offending: None,
        },
        ViolationCase {
            name: "AdapterName one past the maximum length",
            operation: "CreateAdapter",
            request: json!({ "AdapterName": over_length, "FeatureTypes": ["QUERIES"] }),
            offending: Some(over_length.clone()),
        },
        ViolationCase {
            name: "a required member absent",
            operation: "CreateAdapter",
            request: json!({ "AdapterName": "no-features" }),
            offending: None,
        },
        ViolationCase {
            name: "another operation's name outside the pattern",
            operation: "UpdateAdapter",
            request: json!({
                "AdapterId": "a".repeat(12),
                "AdapterName": "renamed adapter!!",
            }),
            offending: Some("renamed adapter!!".to_string()),
        },
        ViolationCase {
            name: "another operation's required id absent",
            operation: "GetAdapter",
            request: json!({}),
            offending: None,
        },
        ViolationCase {
            name: "MaxResults under its published minimum",
            operation: "ListAdapters",
            request: json!({ "MaxResults": 0 }),
            offending: None,
        },
        ViolationCase {
            name: "JobId one past the maximum length",
            operation: "GetDocumentTextDetection",
            request: json!({ "JobId": over_job_id_length }),
            offending: Some(over_job_id_length.clone()),
        },
    ];

    for case in &cases {
        let (status, body) = call(&router, case.operation, &case.request).await;
        assert_refusal(
            case.name,
            status,
            &body,
            "InvalidParameterException",
            INVALID_PARAMETERS_MESSAGE,
        );
        if let Some(offending) = &case.offending {
            assert!(
                !body.to_string().contains(offending.as_str()),
                "case {:?}: the refusal must not quote the rejected value back, got {body}",
                case.name
            );
        }
    }

    // The positive control for the same boundary: a value AT each published bound of the
    // member the rows step past is served.
    for (name, adapter_name) in [
        ("AdapterName at the minimum length", "a".to_string()),
        ("AdapterName at the maximum length", "a".repeat(128)),
    ] {
        let (status, body) = call(
            &router,
            "CreateAdapter",
            &json!({ "AdapterName": adapter_name, "FeatureTypes": ["QUERIES"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a value inside the published bounds must be served, got \
             {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_job_id_this_service_never_issued_is_refused_in_textracts_own_wording() {
    let router = harness();

    // Two real jobs, one of each kind, so the table can also cross them: a JobId is
    // valid only for the operation whose Start* minted it. No Amazon S3 is mounted beside
    // this service here, so nothing about the object is established and the submit still
    // mints a job — which is all these rows need: a `JobId` this service really issued.
    let (status, body) = call(
        &router,
        "StartDocumentTextDetection",
        &json!({ "DocumentLocation": s3_location() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a text-detection job must start, got {status} with {body}"
    );
    let detection_job = body["JobId"].as_str().expect("a JobId").to_string();

    let (status, body) = call(
        &router,
        "StartDocumentAnalysis",
        &json!({ "DocumentLocation": s3_location(), "FeatureTypes": ["FORMS"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an analysis job must start, got {status} with {body}"
    );
    let analysis_job = body["JobId"].as_str().expect("a JobId").to_string();
    assert_ne!(
        detection_job, analysis_job,
        "the two jobs must be two jobs, both {detection_job:?}"
    );

    // Rows span the shapes a JobId inside its modelled `^[a-zA-Z0-9-_]+$` / 1..64 bounds
    // can take — the shortest, a hex-looking id, a dashed one, one at the maximum length
    // and one that differs from an issued id by a single character — so what is asserted
    // is "this service never issued it", not one recorded id.
    let mut near_miss = detection_job.clone();
    let last = near_miss.pop().expect("a non-empty JobId");
    near_miss.push(if last == '0' { '1' } else { '0' });
    assert_ne!(
        near_miss, detection_job,
        "the near miss must differ from the issued id, got {near_miss:?}"
    );
    let unissued: Vec<(&str, String)> = vec![
        ("a single character", "j".to_string()),
        (
            "an id that looks hexadecimal",
            "0123456789abcdef".to_string(),
        ),
        ("a dashed and underscored id", "sim2real-job_1".to_string()),
        ("an id at the maximum length", "a".repeat(64)),
        ("an id one character off an issued one", near_miss),
    ];

    for operation in ["GetDocumentTextDetection", "GetDocumentAnalysis"] {
        // The other kind's job is as invalid a JobId for this operation as one nothing
        // started: this operation never issued it.
        let crossed = if operation == "GetDocumentTextDetection" {
            analysis_job.clone()
        } else {
            detection_job.clone()
        };
        let mut rows = unissued.clone();
        rows.push(("the other operation's job", crossed));

        for (name, job_id) in rows {
            let (status, body) = call(&router, operation, &json!({ "JobId": job_id })).await;
            let case = format!("{operation} with {name}");
            assert_refusal(
                &case,
                status,
                &body,
                "InvalidJobIdException",
                INVALID_JOB_ID_MESSAGE,
            );
            assert!(
                !body.to_string().contains(job_id.as_str()),
                "case {case:?}: the refusal must not quote the submitted JobId back, got {body}"
            );
        }
    }

    // The positive control: each job IS readable through the operation that started it,
    // so the refusals above are about the id and not about the read.
    for (operation, job_id) in [
        ("GetDocumentTextDetection", &detection_job),
        ("GetDocumentAnalysis", &analysis_job),
    ] {
        let (status, body) = call(&router, operation, &json!({ "JobId": job_id })).await;
        assert_eq!(
            status, 200,
            "case {operation}: a job this service issued must be readable, got {status} \
             with {body}"
        );
    }
}

#[test]
fn every_refusal_sentence_this_service_states_is_fixed_text() {
    // The constants the handlers answer with are the sentences this suite asserts over
    // the wire: one place states each refusal, so no path can word it differently.
    for (name, stated, expected) in [
        (
            "UnsupportedDocumentException",
            state::UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE,
            UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE,
        ),
        (
            "InvalidParameterException at the boundary",
            state::INVALID_PARAMETERS_MESSAGE,
            INVALID_PARAMETERS_MESSAGE,
        ),
        (
            "InvalidJobIdException",
            state::INVALID_JOB_ID_MESSAGE,
            INVALID_JOB_ID_MESSAGE,
        ),
        (
            "QueriesConfig without the QUERIES feature",
            state::QUERIES_CONFIG_WITHOUT_FEATURE_MESSAGE,
            QUERIES_CONFIG_WITHOUT_FEATURE_MESSAGE,
        ),
    ] {
        assert_eq!(
            stated, expected,
            "case {name:?}: expected the sentence {expected:?}, got {stated:?}"
        );
    }

    // The unreadable-document refusal carries that sentence and nothing else, whatever
    // the reason behind it: `Unreadable::UnsupportedFormat` holds no text of its own.
    assert_eq!(
        state::Unreadable::UnsupportedFormat.message(),
        UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE,
        "the unreadable-document refusal must state the one sentence, got {:?}",
        state::Unreadable::UnsupportedFormat.message()
    );
}
