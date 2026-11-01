// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the two refusals a Textract client branches on most, end to end over
//! the wire (`awsJson1_1`, `POST /` with `X-Amz-Target: Textract.<Operation>`).
//!
//! THE RULE UNDER TEST: a refusal carries the message Amazon Textract states for that
//! error class — its fixed words to the character, with no punctuation of this emulator's
//! own and nothing taken from the request interpolated — the same string whichever
//! operation refused and whatever the request held:
//!
//! * a lookup whose `AdapterId` names no adapter this service holds is
//!   `ResourceNotFoundException` (HTTP 400) with [`ADAPTER_NOT_FOUND_MESSAGE`], and a
//!   tagging operation whose `ResourceARN` names one is refused the same way, with the
//!   same words in the wording that path states them in
//!   ([`TAGGED_RESOURCE_NOT_FOUND_MESSAGE`]);
//! * an input that violates a constraint on this service's input — an absent, empty or
//!   inconsistent member, a value outside a declared enum, length or range, or a cursor
//!   this listing never issued — is `InvalidParameterException` (HTTP 400) with
//!   [`INVALID_PARAMETERS_MESSAGE`].
//!
//! Each table spans what the rule generalises over: which operation refused, how the
//! resource is addressed (an `AdapterId` or an ARN), why it names nothing, and which kind
//! of constraint the request broke. Every row also asserts the refusal quotes NOTHING of
//! the request back, and each table carries the positive control for its rule — the same
//! operation, one step inside the same boundary, is served — so neither refusal is
//! unconditional.
//!
//! The wire harness (`harness`, `call`, `error_code`, `error_message`) is the one
//! `document_extraction` exposes to this test target.

use serde_json::{json, Value};
use service_com_amazonaws_textract_textract::state;

use crate::document_extraction::{call, error_code, error_message, harness, text_document};
use crate::queries_and_input_refusals::s3_location;

/// The sentence Amazon Textract states `ResourceNotFoundException` with on the path that
/// takes an `AdapterId` directly: fixed text, stated as a sentence.
const ADAPTER_NOT_FOUND_MESSAGE: &str = "Adapter Id not found.";

/// The sentence the tagging operations state that same refusal with, when the resource
/// their `ResourceARN` resolves to is an adapter this service holds none of.
const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Adapter Id not found";

/// The sentence Amazon Textract states `InvalidParameterException` with for an input that
/// violates a constraint.
const INVALID_PARAMETERS_MESSAGE: &str = "Request has invalid parameters";

/// The region the ARNs in this suite are written for; any region reaches this store.
const REGION: &str = "us-east-1";

/// A document whose text is known from its own characters, for the rows that need a
/// readable document.
const INVOICE: &str = "Invoice Number: INV-2048\nTotal Due: 17.25";

/// Assert one refusal: the status, the modelled code, the exact sentence, and that the
/// sentence carries nothing the request supplied.
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
    let actual = error_message(body);
    assert_eq!(
        actual, message,
        "case {case:?}: expected the message {message:?}, got {actual:?}"
    );
}

/// `CreateAdapter`, asserting it succeeded, and the `AdapterId` it minted.
async fn create_adapter(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreateAdapter",
        &json!({ "AdapterName": name, "FeatureTypes": ["QUERIES"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAdapter must succeed, got {status} with {body}"
    );
    body.get("AdapterId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `Start*`, asserting it minted a job, and the `JobId` it answered with.
async fn start_job(
    router: &smithy_server_runtime::MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> String {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 200,
        "case {operation:?}: a job must start, got {status} with {body}"
    );
    body.get("JobId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn an_adapter_lookup_that_finds_nothing_is_refused_in_textracts_own_wording() {
    let router = harness();

    let live = create_adapter(&router, "the one live adapter", "still-here").await;
    let deleted = create_adapter(&router, "an adapter to delete", "gone").await;
    let (status, body) = call(
        &router,
        "DeleteAdapter",
        &json!({ "AdapterId": deleted.clone() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteAdapter of {deleted:?} must succeed, got {status} with {body}"
    );

    // Ids span the modelled 12..1011 length of `AdapterId` (smithy_query member
    // GetAdapterRequest$AdapterId @length 12..1011) — each published bound, a plausible
    // 32-character id, an id a delete removed, and a live id with its last character
    // changed — so what is asserted is "this service holds no such adapter", not one
    // recorded id.
    let mut near_miss = live.clone();
    let last = near_miss.pop().expect("a non-empty AdapterId");
    near_miss.push(if last == '0' { '1' } else { '0' });
    let ids: Vec<(String, String)> = vec![
        ("an id at the modelled minimum length".to_string(), "a".repeat(12)),
        ("an id at the modelled maximum length".to_string(), "b".repeat(1011)),
        ("a plausible 32-character id".to_string(), "c".repeat(32)),
        ("an id a delete removed".to_string(), deleted.clone()),
        ("a live id with one character changed".to_string(), near_miss),
    ];

    for (name, adapter_id) in &ids {
        assert_ne!(
            adapter_id, &live,
            "case {name:?}: the table must not name the adapter that exists"
        );
        // Every operation that looks an adapter up BY ID.
        for (operation, request) in [
            ("GetAdapter", json!({ "AdapterId": adapter_id })),
            (
                "UpdateAdapter",
                json!({ "AdapterId": adapter_id, "Description": "no such adapter" }),
            ),
            ("DeleteAdapter", json!({ "AdapterId": adapter_id })),
            ("ListAdapterVersions", json!({ "AdapterId": adapter_id })),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            let case = format!("{name} on {operation}");
            assert_refusal(
                &case,
                status,
                &body,
                "ResourceNotFoundException",
                ADAPTER_NOT_FOUND_MESSAGE,
            );
            assert!(
                !body.to_string().contains(adapter_id.as_str()),
                "case {case:?}: the refusal must not quote the id back, got {body}"
            );
        }

        // The same adapter, addressed as the tagging operations address it: by the
        // identifier of a well-formed Amazon Textract ARN. `ResourceARN`'s modelled
        // length is 1..1011 (smithy_query member TagResourceRequest$ResourceARN), so the
        // 1011-character id is left to the rows above.
        if adapter_id.len() > 64 {
            continue;
        }
        let arn = state::adapter_arn(REGION, adapter_id);
        for (operation, request) in [
            (
                "TagResource",
                json!({ "ResourceARN": arn, "Tags": { "env": "test" } }),
            ),
            (
                "UntagResource",
                json!({ "ResourceARN": arn, "TagKeys": ["env"] }),
            ),
            ("ListTagsForResource", json!({ "ResourceARN": arn })),
        ] {
            let (status, body) = call(&router, operation, &request).await;
            let case = format!("{name} on {operation}");
            assert_refusal(
                &case,
                status,
                &body,
                "ResourceNotFoundException",
                TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
            );
            assert!(
                !body.to_string().contains(adapter_id.as_str()),
                "case {case:?}: the refusal must not quote the ARN or the id back, got {body}"
            );
        }
    }

    // The positive control for the same lookup: the adapter that DOES exist is read and
    // tagged through the very same operations, so the refusals above are about the
    // resource and not about the call.
    let (status, body) = call(&router, "GetAdapter", &json!({ "AdapterId": live })).await;
    assert_eq!(
        status, 200,
        "the live adapter must be readable, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "ListTagsForResource",
        &json!({ "ResourceARN": state::adapter_arn(REGION, &live) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the live adapter's tags must be readable, got {status} with {body}"
    );
}

/// One row of the constraint table: the request, and the value the refusal must not quote
/// back when the row carries one.
struct ParameterCase {
    name: String,
    operation: &'static str,
    request: Value,
    offending: Option<String>,
}

#[tokio::test]
async fn an_input_that_violates_a_constraint_is_refused_in_textracts_own_wording() {
    let router = harness();

    let adapter_id = create_adapter(&router, "the adapter that must not move", "untouched").await;
    let detection_job = start_job(
        &router,
        "StartDocumentTextDetection",
        &json!({ "DocumentLocation": s3_location() }),
    )
    .await;
    let analysis_job = start_job(
        &router,
        "StartDocumentAnalysis",
        &json!({ "DocumentLocation": s3_location(), "FeatureTypes": ["FORMS"] }),
    )
    .await;

    // The rows span the KINDS of constraint this service's input carries: a required list
    // present but empty, a value outside a modelled enum, two members that must be
    // supplied exactly one of, a member absent from a nested structure, a request naming
    // no change at all, a value that is no ARN, a `Query.Pages` entry outside the forms
    // AWS documents, and a cursor the listing never issued. `FEATURE_TYPE_VALUES` and
    // `AUTO_UPDATE_VALUES` are the FeatureType / AutoUpdate enums of the model.
    let unissued_cursor = "bm90LWEtY3Vyc29y";
    let cases = vec![
        ParameterCase {
            name: "AnalyzeDocument with FeatureTypes present but empty".to_string(),
            operation: "AnalyzeDocument",
            request: json!({ "Document": text_document(INVOICE), "FeatureTypes": [] }),
            offending: None,
        },
        ParameterCase {
            name: "StartDocumentAnalysis with FeatureTypes present but empty".to_string(),
            operation: "StartDocumentAnalysis",
            request: json!({ "DocumentLocation": s3_location(), "FeatureTypes": [] }),
            offending: None,
        },
        ParameterCase {
            name: "a FeatureTypes value outside the modelled enum".to_string(),
            operation: "AnalyzeDocument",
            request: json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["HANDWRITING"],
            }),
            offending: Some("HANDWRITING".to_string()),
        },
        ParameterCase {
            name: "a Document carrying neither Bytes nor S3Object".to_string(),
            operation: "AnalyzeDocument",
            request: json!({ "Document": {}, "FeatureTypes": ["FORMS"] }),
            offending: None,
        },
        ParameterCase {
            name: "a Document carrying both Bytes and S3Object".to_string(),
            operation: "DetectDocumentText",
            request: json!({ "Document": {
                "Bytes": aws_smithy_types::base64::encode(INVOICE.as_bytes()),
                "S3Object": { "Bucket": "documents", "Name": "invoice.txt" },
            } }),
            offending: None,
        },
        ParameterCase {
            name: "a DocumentLocation carrying no S3Object".to_string(),
            operation: "StartDocumentTextDetection",
            request: json!({ "DocumentLocation": {} }),
            offending: None,
        },
        ParameterCase {
            name: "an S3Object naming a bucket but no object".to_string(),
            operation: "StartDocumentAnalysis",
            request: json!({
                "DocumentLocation": { "S3Object": { "Bucket": "ledger-bucket" } },
                "FeatureTypes": ["FORMS"],
            }),
            offending: Some("ledger-bucket".to_string()),
        },
        ParameterCase {
            name: "CreateAdapter with FeatureTypes present but empty".to_string(),
            operation: "CreateAdapter",
            request: json!({ "AdapterName": "no-features", "FeatureTypes": [] }),
            offending: None,
        },
        ParameterCase {
            name: "CreateAdapter with a feature no adapter trains on".to_string(),
            operation: "CreateAdapter",
            request: json!({ "AdapterName": "forms-adapter", "FeatureTypes": ["FORMS"] }),
            offending: None,
        },
        ParameterCase {
            name: "UpdateAdapter naming no updatable member".to_string(),
            operation: "UpdateAdapter",
            request: json!({ "AdapterId": adapter_id.clone() }),
            offending: Some(adapter_id.clone()),
        },
        ParameterCase {
            name: "UpdateAdapter with an AutoUpdate outside the modelled enum".to_string(),
            operation: "UpdateAdapter",
            request: json!({ "AdapterId": adapter_id.clone(), "AutoUpdate": "MAYBE" }),
            offending: Some("MAYBE".to_string()),
        },
        ParameterCase {
            name: "TagResource with a value that is no Textract ARN".to_string(),
            operation: "TagResource",
            request: json!({ "ResourceARN": "not-an-arn", "Tags": { "env": "test" } }),
            offending: Some("not-an-arn".to_string()),
        },
        ParameterCase {
            name: "UntagResource with another service's ARN".to_string(),
            operation: "UntagResource",
            request: json!({
                "ResourceARN": format!("arn:aws:comprehend:{REGION}:000000000000:/adapters/{adapter_id}"),
                "TagKeys": ["env"],
            }),
            offending: Some("comprehend".to_string()),
        },
        ParameterCase {
            name: "ListTagsForResource with an ARN whose resource field is empty".to_string(),
            operation: "ListTagsForResource",
            request: json!({
                "ResourceARN": format!("arn:aws:textract:{REGION}:000000000000:/adapters/"),
            }),
            offending: None,
        },
        ParameterCase {
            // AWS, on `Query$Pages`: `*` must be the only element when it is used, and a
            // page must be greater than 0.
            name: "a Query.Pages using * beside another entry".to_string(),
            operation: "AnalyzeDocument",
            request: json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["QUERIES"],
                "QueriesConfig": { "Queries": [{
                    "Text": "What is the total due?",
                    "Pages": ["*", "2"],
                }] },
            }),
            offending: None,
        },
        ParameterCase {
            name: "a Query.Pages naming page 0".to_string(),
            operation: "AnalyzeDocument",
            request: json!({
                "Document": text_document(INVOICE),
                "FeatureTypes": ["QUERIES"],
                "QueriesConfig": { "Queries": [{
                    "Text": "What is the total due?",
                    "Pages": ["0"],
                }] },
            }),
            offending: None,
        },
        ParameterCase {
            name: "ListAdapters with a cursor it never issued".to_string(),
            operation: "ListAdapters",
            request: json!({ "NextToken": unissued_cursor }),
            offending: Some(unissued_cursor.to_string()),
        },
        ParameterCase {
            name: "ListAdapterVersions with a cursor it never issued".to_string(),
            operation: "ListAdapterVersions",
            request: json!({ "NextToken": unissued_cursor }),
            offending: Some(unissued_cursor.to_string()),
        },
        ParameterCase {
            name: "GetDocumentTextDetection with a cursor its job never issued".to_string(),
            operation: "GetDocumentTextDetection",
            request: json!({ "JobId": detection_job.clone(), "NextToken": unissued_cursor }),
            offending: Some(unissued_cursor.to_string()),
        },
        ParameterCase {
            name: "GetDocumentAnalysis with a cursor its job never issued".to_string(),
            operation: "GetDocumentAnalysis",
            request: json!({ "JobId": analysis_job.clone(), "NextToken": unissued_cursor }),
            offending: Some(unissued_cursor.to_string()),
        },
    ];

    for case in &cases {
        let (status, body) = call(&router, case.operation, &case.request).await;
        assert_refusal(
            &case.name,
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

    // The positive controls for the same boundaries: one step inside each, the very same
    // operations are served.
    for (name, operation, request) in [
        (
            "AnalyzeDocument with one feature asked for",
            "AnalyzeDocument",
            json!({ "Document": text_document(INVOICE), "FeatureTypes": ["FORMS"] }),
        ),
        (
            "CreateAdapter with the feature an adapter trains on",
            "CreateAdapter",
            json!({ "AdapterName": "queries-adapter", "FeatureTypes": ["QUERIES"] }),
        ),
        (
            "UpdateAdapter naming one updatable member",
            "UpdateAdapter",
            json!({ "AdapterId": adapter_id.clone(), "Description": "a real change" }),
        ),
        (
            "ListAdapters with no cursor at all",
            "ListAdapters",
            json!({}),
        ),
        (
            "GetDocumentTextDetection with no cursor at all",
            "GetDocumentTextDetection",
            json!({ "JobId": detection_job.clone() }),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a request inside the boundary must be served, got {status} \
             with {body}"
        );
    }
}

#[test]
fn the_sentences_this_service_states_these_refusals_with_are_fixed_text() {
    // The sentences the handlers answer with are the sentences this suite asserts over
    // the wire: one place states each refusal, so no lookup and no validation path can
    // word it differently.
    for (name, stated, expected) in [
        (
            "ResourceNotFoundException for an AdapterId",
            state::ADAPTER_NOT_FOUND_MESSAGE,
            ADAPTER_NOT_FOUND_MESSAGE,
        ),
        (
            "ResourceNotFoundException for a tagged ResourceARN",
            state::TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
            TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
        ),
        (
            "InvalidParameterException for a constraint violation",
            state::INVALID_PARAMETERS_MESSAGE,
            INVALID_PARAMETERS_MESSAGE,
        ),
    ] {
        assert_eq!(
            stated, expected,
            "case {name:?}: expected the sentence {expected:?}, got {stated:?}"
        );
    }

    // The two lookup paths state the same WORDS; only the terminal period differs, so
    // neither sentence may drift into the other's wording.
    assert_eq!(
        state::ADAPTER_NOT_FOUND_MESSAGE.trim_end_matches('.'),
        state::TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
        "the two lookup refusals must state the same words, got {:?} and {:?}",
        state::ADAPTER_NOT_FOUND_MESSAGE,
        state::TAGGED_RESOURCE_NOT_FOUND_MESSAGE
    );
    assert!(
        !state::INVALID_PARAMETERS_MESSAGE.ends_with('.'),
        "the constraint refusal must carry no trailing period, got {:?}",
        state::INVALID_PARAMETERS_MESSAGE
    );
}
