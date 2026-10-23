// SPDX-License-Identifier: CC0-1.0

//! The protocol's SUCCESS STATUS, over the whole implemented surface.
//!
//! Amazon Transcribe speaks awsJson1_1, an RPC protocol:
//! `smithy_query(query="service_info")` reports `protocols [awsJson1_1]`, and every
//! operation's `@http` binding comes back `httpBindingsInert` — "RPC protocol
//! (awsJson/awsQuery/ec2Query/rpcv2Cbor): @http/httpLabel/httpQuery/httpHeader are
//! ignored on the wire". A successful call therefore answers **HTTP 200** with the
//! JSON envelope, whatever code the model's REST-shaped `@http` trait declares, and
//! an output shape with no members answers 200 with an empty JSON object rather than
//! `204 No Content`.
//!
//! The table below spans the class along the only axis the rule generalises over: the
//! modelled (inert) `@http` code — 200, 201 and 204 — crossed with whether the output
//! shape has members. Every modelled code is the one
//! `smithy_query(query="operation", operation="<Op>")` reports for that operation.
//!
//! The second test is the rule's boundary: a FAILED call is not flattened with the
//! successes — it answers the status its own failure carries (this service's
//! client-error status for a modelled client error, named in the body's `__type`; the
//! scaffold's 501 for an operation nobody has written yet), which
//! `crate::scaffolded::wire_status` is the single home of.

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// The awsJson1_1 content type, which every answer of this protocol carries.
const AWS_JSON_1_1: &str = "application/x-amz-json-1.1";

/// Long enough for every clock-owed transition of a resource created during the test
/// to have fallen due (a queued job's two 500 ms spans), so the deletes below meet a
/// resource in a terminal state.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_EVERY_TRANSITION: std::time::Duration = std::time::Duration::from_millis(1_300);

/// The region an unsigned request is served in, so a tagging ARN is predictable.
const REGION: &str = "us-east-1";

/// One call in the table: the operation, the request, the code the model's inert
/// `@http` trait declares for it, and whether its output shape declares any members.
struct Row {
    case: &'static str,
    operation: &'static str,
    /// The `http.code` `smithy_query(query="operation")` reports for this operation —
    /// inert on this protocol, and so NOT the status the call answers.
    modelled_code: u16,
    input: Value,
    /// True when the output shape has no members (`Unit`, or a response structure the
    /// model gives no members): the body is then an empty JSON object.
    empty_output: bool,
    /// True when the row needs every clock-owed transition to have fallen due first
    /// (the deletes, which refuse a resource in a non-terminal state).
    needs_terminal_state: bool,
}

/// Assert one row's answer: HTTP 200, the awsJson1_1 content type, and a JSON body
/// whose emptiness matches the output shape.
fn assert_rpc_success(row: &Row, reply: &Reply) {
    let case = row.case;
    let modelled = row.modelled_code;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: {op} is awsJson1_1, so a success answers HTTP 200 and not the \
         modelled (inert) @http code {modelled}, got {} with body {:?}",
        reply.status,
        reply.body,
        op = row.operation,
    );
    assert_eq!(
        reply.content_type.as_deref(),
        Some(AWS_JSON_1_1),
        "case {case:?}: a success must carry the awsJson1_1 content type, got {:?}",
        reply.content_type
    );
    assert!(
        reply.body.is_object(),
        "case {case:?}: a success must carry the JSON envelope, got {:?}",
        reply.body
    );
    let members = reply.body.as_object().expect("a JSON object").len();
    if row.empty_output {
        assert_eq!(
            members, 0,
            "case {case:?}: an output shape with no members answers an empty JSON \
             object, got {:?}",
            reply.body
        );
    } else {
        assert!(
            members > 0,
            "case {case:?}: an output shape with members must answer them, got {:?}",
            reply.body
        );
    }
}

/// Every implemented operation answers a success with HTTP 200, whatever its modelled
/// `@http` code is.
#[tokio::test]
async fn a_success_answers_200_whatever_the_modelled_http_code_is() {
    let transcribe = Transcribe::new();

    let vocabulary_arn = format!(
        "arn:aws:transcribe:{REGION}:{account}:vocabulary/status-vocab",
        account = engine::emulator_account_id()
    );

    let rows = vec![
        Row {
            case: "CreateVocabulary, modelled PUT 200, members in the output",
            operation: "CreateVocabulary",
            modelled_code: 200,
            input: json!({
                "VocabularyName": "status-vocab",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            empty_output: false,
            needs_terminal_state: false,
        },
        Row {
            case: "CreateVocabularyFilter, modelled POST 201, members in the output",
            operation: "CreateVocabularyFilter",
            modelled_code: 201,
            input: json!({
                "VocabularyFilterName": "status-filter",
                "LanguageCode": "en-US",
                "Words": ["crud", "bogus"]
            }),
            empty_output: false,
            needs_terminal_state: false,
        },
        Row {
            case: "StartTranscriptionJob, modelled PUT 200, members in the output",
            operation: "StartTranscriptionJob",
            modelled_code: 200,
            input: json!({
                "TranscriptionJobName": "status-job",
                "Media": {"MediaFileUri": "s3://example-media/audio.mp3"},
                "LanguageCode": "en-US",
                "MediaFormat": "mp3"
            }),
            empty_output: false,
            needs_terminal_state: false,
        },
        Row {
            case: "GetVocabulary, modelled GET 200, members in the output",
            operation: "GetVocabulary",
            modelled_code: 200,
            input: json!({"VocabularyName": "status-vocab"}),
            empty_output: false,
            needs_terminal_state: false,
        },
        Row {
            case: "UpdateVocabularyFilter, modelled PUT 200, members in the output",
            operation: "UpdateVocabularyFilter",
            modelled_code: 200,
            input: json!({
                "VocabularyFilterName": "status-filter",
                "Words": ["replaced"]
            }),
            empty_output: false,
            needs_terminal_state: false,
        },
        Row {
            case: "ListVocabularyFilters, modelled GET 200, members in the output",
            operation: "ListVocabularyFilters",
            modelled_code: 200,
            input: json!({"MaxResults": 100}),
            empty_output: false,
            needs_terminal_state: false,
        },
        Row {
            case: "TagResource, modelled PUT 200, output shape with no members",
            operation: "TagResource",
            modelled_code: 200,
            input: json!({
                "ResourceArn": vocabulary_arn,
                "Tags": [{"Key": "team", "Value": "speech"}, {"Key": "env", "Value": "test"}]
            }),
            empty_output: true,
            needs_terminal_state: false,
        },
        Row {
            case: "ListTagsForResource, modelled GET 200, members in the output",
            operation: "ListTagsForResource",
            modelled_code: 200,
            input: json!({"ResourceArn": vocabulary_arn}),
            empty_output: false,
            needs_terminal_state: false,
        },
        Row {
            case: "UntagResource, modelled DELETE 204, output shape with no members",
            operation: "UntagResource",
            modelled_code: 204,
            input: json!({"ResourceArn": vocabulary_arn, "TagKeys": ["env"]}),
            empty_output: true,
            needs_terminal_state: false,
        },
        Row {
            case: "DeleteVocabulary, modelled DELETE 204, Unit output",
            operation: "DeleteVocabulary",
            modelled_code: 204,
            input: json!({"VocabularyName": "status-vocab"}),
            empty_output: true,
            needs_terminal_state: true,
        },
        Row {
            case: "DeleteVocabularyFilter, modelled DELETE 204, Unit output",
            operation: "DeleteVocabularyFilter",
            modelled_code: 204,
            input: json!({"VocabularyFilterName": "status-filter"}),
            empty_output: true,
            needs_terminal_state: true,
        },
        Row {
            case: "DeleteTranscriptionJob, modelled DELETE 204, Unit output",
            operation: "DeleteTranscriptionJob",
            modelled_code: 204,
            input: json!({"TranscriptionJobName": "status-job"}),
            empty_output: true,
            needs_terminal_state: true,
        },
    ];

    // The table is only a test of the RULE if it spans more than the one code the
    // protocol default already is.
    let mut modelled: Vec<u16> = rows.iter().map(|row| row.modelled_code).collect();
    modelled.sort_unstable();
    modelled.dedup();
    assert!(
        modelled.len() >= 3,
        "the table must span several modelled @http codes, got {modelled:?}"
    );

    // The rows that need no clock-owed transition first, in order: each one leaves
    // behind what the next reads.
    for row in rows.iter().filter(|row| !row.needs_terminal_state) {
        let reply = transcribe.call(row.operation, row.input.clone()).await;
        assert_rpc_success(row, &reply);
    }

    // The deletes refuse a resource the service is still processing, so they run once
    // every transition has fallen due.
    std::thread::sleep(PAST_EVERY_TRANSITION);
    for row in rows.iter().filter(|row| row.needs_terminal_state) {
        let reply = transcribe.call(row.operation, row.input.clone()).await;
        assert_rpc_success(row, &reply);
    }
}

/// The boundary of the rule: a FAILED call is not flattened to 200 — it answers the
/// status its own kind of failure carries (this service's client-error status for a
/// modelled client error, with the shape named in the body's `__type`, and the
/// scaffold's 501 for an operation nobody has written yet).
#[tokio::test]
async fn a_refusal_keeps_its_own_status() {
    let transcribe = Transcribe::new();

    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "refusal-vocab",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
        )
        .await
        .ok("create the vocabulary the rows below address");

    // One row per kind of refusal reachable here: a resource that does not exist, a
    // resource the service is still processing, a boundary constraint violation, and
    // the scaffold's own not-implemented. The client-error rows carry this service's
    // own client-error status (`crate::scaffolded::wire_status`) with the error shape
    // in the body's `__type`; none of them is 200.
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "an unknown vocabulary",
            "GetVocabulary",
            json!({"VocabularyName": "never-created"}),
            400,
            "BadRequestException",
        ),
        (
            "an unknown vocabulary filter",
            "GetVocabularyFilter",
            json!({"VocabularyFilterName": "never-created"}),
            400,
            "BadRequestException",
        ),
        (
            "a delete while the vocabulary is still being processed",
            "DeleteVocabulary",
            json!({"VocabularyName": "refusal-vocab"}),
            400,
            "BadRequestException",
        ),
        (
            "a name past the member's length constraint",
            "GetVocabulary",
            json!({"VocabularyName": "a".repeat(201)}),
            400,
            "ValidationException",
        ),
        (
            "an operation that is still a stub",
            "ListLanguageModels",
            json!({}),
            501,
            "NotImplemented",
        ),
    ];

    for (case, operation, input, status, code) in &rows {
        let reply = transcribe.call(operation, input.clone()).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: {operation} must answer HTTP {status}, got {} with body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case:?}: expected the {code} error code, got {:?}",
            reply.body
        );
    }
}
