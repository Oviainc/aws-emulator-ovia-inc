// SPDX-License-Identifier: CC0-1.0

//! What a captured turn records about the REQUEST is read in ONE place.
//!
//! RULE: a captured turn records the `Content-Type` the caller labelled its
//! inference payload with exactly as the request carried it — unnormalised, with
//! its parameters and its case kept, and absent when the call sent no readable
//! one — and which sampling operation served the turn does not change that
//! value, because both of them reach the store through the crate's one sampling
//! path (`state::serve_sampling_turn`, which derives it from the request headers
//! it is handed: `state::TurnRequest::request_content_type`).
//!
//! The rows span what the rule generalises over: whether the call labels its
//! payload at all, and — when it does — the media type it names, whether the
//! value carries parameters, and the case it is spelled in. Every row is run
//! twice, once through `/sample` and once through
//! `/sample-with-response-stream`, each on its own engine, and the two captured
//! values are compared with each other as well as against the rule; a further
//! row drives one trajectory that MIXES the two operations, which the service
//! documents as allowed, and the last case drives a header value that is not
//! readable as text through the shared read itself, which no HTTP request this
//! harness can build is able to carry.
//!
//! Every answer goes over the wire through a [`MultiServiceRouter`], and the
//! store is read afterwards the way the NEXT request would read it.

use serde_json::json;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use smithy_server_runtime::MultiServiceRouter;

// The shared harness: the wire-answer reduction and the job-directory setup every suite
// of this crate drives (`tests/all/harness.rs`).
use crate::harness::{
    create_job_at_arn as create_job, job_arn, mounted_service, send, Reply, JOB_ARN_HEADER,
    SAMPLE_PATH, SAMPLE_STREAM_PATH, TRAJECTORY_ID_HEADER,
};

/// One sampling call on `path`, labelling its payload with `content_type` — or
/// sending no `Content-Type` at all when the row carries none.
async fn sample(
    router: &MultiServiceRouter,
    path: &str,
    job_arn: &str,
    trajectory_id: &str,
    content_type: Option<&str>,
    payload: &[u8],
) -> Reply {
    let mut headers = vec![
        (JOB_ARN_HEADER, job_arn),
        (TRAJECTORY_ID_HEADER, trajectory_id),
    ];
    if let Some(content_type) = content_type {
        headers.push(("content-type", content_type));
    }
    send(router, path, &headers, payload.to_vec()).await
}

/// An OpenAI-compatible chat-completions request body.
fn chat_payload(text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "messages": [{ "role": "user", "content": text }] }))
        .expect("a serializable payload")
}

/// What every captured turn of the trajectory recorded as the request's
/// `Content-Type`, read the way the next request would read the store.
fn captured_content_types(
    engine: &engine::InMemoryEngine,
    job_arn: &str,
    trajectory_id: &str,
) -> Vec<Option<String>> {
    let store = engine.state::<ServiceState>();
    let guard = store.lock();
    guard
        .trajectory(job_arn, trajectory_id)
        .map_or_else(Vec::new, |trajectory| {
            trajectory
                .turns
                .iter()
                .map(|turn| turn.request_content_type.clone())
                .collect()
        })
}

/// One row: the label a call carries, and the value the captured turn owes it.
struct Case {
    case: &'static str,
    /// The `Content-Type` the call labels its payload with, or `None` when it
    /// sends none.
    sent: Option<&'static str>,
    /// What the captured turn must record for it.
    captured: Option<&'static str>,
}

/// The rows. The media types are the ones this service's own bodies carry
/// (`state::JSON_CONTENT_TYPE`, `state::EVENT_STREAM_CONTENT_TYPE`) plus values
/// that differ from them along what the rule generalises over: a parameterised
/// value, a different top-level type, and a differently cased spelling.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "no Content-Type at all",
            sent: None,
            captured: None,
        },
        Case {
            case: "the JSON media type the service's own bodies carry",
            sent: Some(state::JSON_CONTENT_TYPE),
            captured: Some(state::JSON_CONTENT_TYPE),
        },
        Case {
            case: "a parameterised value is kept whole",
            sent: Some("application/json; charset=utf-8"),
            captured: Some("application/json; charset=utf-8"),
        },
        Case {
            case: "a value the payload is not in is still recorded as sent",
            sent: Some("text/plain"),
            captured: Some("text/plain"),
        },
        Case {
            case: "an upper-cased value is not lowercased",
            sent: Some("APPLICATION/JSON"),
            captured: Some("APPLICATION/JSON"),
        },
        Case {
            case: "the event-stream media type",
            sent: Some(state::EVENT_STREAM_CONTENT_TYPE),
            captured: Some(state::EVENT_STREAM_CONTENT_TYPE),
        },
        Case {
            case: "an opaque binary media type",
            sent: Some("application/octet-stream"),
            captured: Some("application/octet-stream"),
        },
    ]
}

/// RULE: for every row, each sampling operation captures the request's
/// `Content-Type` as the call carried it, and both capture the SAME value.
#[tokio::test]
async fn both_sampling_operations_capture_the_request_content_type_alike() {
    let arn = job_arn("aws", "us-east-1", "123456789012", "AgentRFT", "run-1");
    for case in cases() {
        let mut captured_per_path: Vec<(&str, Option<String>)> = Vec::new();
        for path in [SAMPLE_PATH, SAMPLE_STREAM_PATH] {
            let (engine, router) = mounted_service();
            create_job(&engine, &arn);
            let reply = sample(
                &router,
                path,
                &arn,
                "traj-label",
                case.sent,
                &chat_payload("label this turn"),
            )
            .await;
            assert_eq!(
                reply.status,
                200,
                "case {:?} on {path}: expected the turn to be served, got {} / {}",
                case.case,
                reply.status,
                reply.text()
            );
            let captured = captured_content_types(&engine, &arn, "traj-label");
            assert_eq!(
                captured.len(),
                1,
                "case {:?} on {path}: expected exactly one captured turn, got {captured:?}",
                case.case
            );
            assert_eq!(
                captured[0].as_deref(),
                case.captured,
                "case {:?} on {path}: expected the captured request Content-Type to be {:?}, got {:?}",
                case.case,
                case.captured,
                captured[0]
            );
            captured_per_path.push((path, captured[0].clone()));
        }
        let (first_path, first) = &captured_per_path[0];
        let (second_path, second) = &captured_per_path[1];
        assert_eq!(
            first, second,
            "case {:?}: expected {first_path} and {second_path} to capture the same request \
             Content-Type, got {first:?} and {second:?}",
            case.case
        );
    }
}

/// RULE: one trajectory may mix turns served by either sampling operation, and
/// each of its turns records the label ITS OWN call carried — the same read,
/// whichever operation served the turn.
#[tokio::test]
async fn a_mixed_trajectory_records_each_turns_own_label() {
    let arn = job_arn("aws", "eu-west-1", "210987654321", "AgentRFTEvaluation", "mix");
    // Turn-by-turn: which operation serves it, the label it sends, and the value
    // the captured turn owes.
    let turns: [(&str, Option<&str>, Option<&str>); 4] = [
        (SAMPLE_PATH, Some(state::JSON_CONTENT_TYPE), Some(state::JSON_CONTENT_TYPE)),
        (
            SAMPLE_STREAM_PATH,
            Some("application/json; charset=utf-8"),
            Some("application/json; charset=utf-8"),
        ),
        (SAMPLE_STREAM_PATH, None, None),
        (SAMPLE_PATH, Some("text/plain"), Some("text/plain")),
    ];
    let (engine, router) = mounted_service();
    create_job(&engine, &arn);
    for (index, (path, sent, _)) in turns.iter().enumerate() {
        let reply = sample(
            &router,
            path,
            &arn,
            "traj-mixed",
            *sent,
            &chat_payload(&format!("turn {index}")),
        )
        .await;
        assert_eq!(
            reply.status,
            200,
            "case mixed turn {index} on {path}: expected the turn to be served, got {} / {}",
            reply.status,
            reply.text()
        );
    }
    let captured = captured_content_types(&engine, &arn, "traj-mixed");
    let expected: Vec<Option<&str>> = turns.iter().map(|(_, _, captured)| *captured).collect();
    let actual: Vec<Option<&str>> = captured.iter().map(Option::as_deref).collect();
    assert_eq!(
        actual, expected,
        "case mixed trajectory: expected each turn to record its own request Content-Type \
         ({expected:?}), got {actual:?}"
    );
}

/// RULE: a `Content-Type` header value that cannot be read as text labels the
/// payload with nothing, so the captured turn records none — the filter in the
/// one shared read, exercised through it because no request this harness can
/// build carries such a value.
#[test]
fn a_header_value_that_is_not_text_is_captured_as_no_label() {
    // Rows: an opaque byte no header value can be read as text, a value with a
    // readable media type, and no header at all.
    let opaque = http::HeaderValue::from_bytes(&[0xff]).expect("an opaque header value");
    let cases: [(&str, Option<http::HeaderValue>, Option<&str>); 3] = [
        ("an opaque header value", Some(opaque), None),
        (
            "a readable header value",
            Some(http::HeaderValue::from_static("application/json")),
            Some("application/json"),
        ),
        ("no header at all", None, None),
    ];
    for (case, value, expected) in cases {
        let mut headers = http::HeaderMap::new();
        if let Some(value) = value {
            headers.insert(http::header::CONTENT_TYPE, value);
        }
        let request = state::TurnRequest {
            job_arn: "arn:aws:sagemaker:us-east-1:123456789012:job/AgentRFT/run-1",
            trajectory_id: "traj-label",
            request_body: b"{}",
            headers: &headers,
            streamed: false,
            at: std::time::SystemTime::now(),
        };
        let actual = request.request_content_type();
        assert_eq!(
            actual.as_deref(),
            expected,
            "case {case:?}: expected the labelled Content-Type to be {expected:?}, got {actual:?}"
        );
    }
}
