// SPDX-License-Identifier: CC0-1.0

//! The request id a Batch response names itself with, end to end over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts and
//! reads the RESPONSE HEADER an SDK surfaces as the call's request id — so what is asserted
//! is what a `batch` client can observe and log, not an internal call.
//!
//! The rule under test, asserted over a TABLE of answers rather than one recorded request:
//! every response this service sends names the request it answered in the
//! `x-amzn-requestid` header — a success exactly as a failure, whichever layer rendered it
//! — and where the response's own document STATES that id (the `RequestId:` segment of the
//! frontend envelope a wrapped `ClientException` carries) the header reports that same
//! string, so a client correlating on the header and one reading the message hold one
//! value.
//!
//! The rows vary what the rule generalises over: whether the answer is a success or a
//! refusal, whether its document states an id or none, the HTTP status (200, 400, 404, 409,
//! 501), the method (`POST`, `GET`), and the LAYER that produced it — a handler's output, a
//! handler-raised refusal, a refusal the boundary validator raised while an operation was
//! being served, the scaffold's 501, the tagging layer's refusal, and a routing failure no
//! operation produced at all.

use serde_json::{json, Value};

use crate::job_lifecycle::{error_message, Batch};
use crate::refusal_contract::{assert_uuid_shaped, enveloped, ENVELOPE_REQUEST_ID};
use crate::resource_tags::tags_uri;

/// The header AWS reports a response's own request id in, and the name botocore reads
/// `ResponseMetadata.RequestId` off. Written out as the literal a client sees rather than
/// taken from the service's constant, so changing that constant fails this suite instead of
/// moving with it.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// What a row's own document states about the request id, which is what decides whether the
/// header has to report a value the body already holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stated {
    /// The document states the id, inside the frontend envelope.
    InTheEnvelope,
    /// The document states no id: a success, a refusal Batch words bare, the scaffold's
    /// 501, and an answer no operation produced.
    Nothing,
}

/// One answer of this service: the call that produces it, the status it comes back with and
/// what its document states about its own request id.
struct Answer {
    /// What this row varies; named in every assertion.
    label: String,
    method: &'static str,
    uri: String,
    payload: Option<Value>,
    status: u16,
    stated: Stated,
}

/// Every layer of this service that can answer, with the resources the rows that need one
/// created first.
async fn answers(batch: &Batch) -> Vec<Answer> {
    // A policy that exists, so the rows below can collide with its name and read its tags.
    let created = batch
        .ok(
            "/v1/createschedulingpolicy",
            json!({"name": "sp-reported", "quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"}}),
        )
        .await;
    let policy_arn = created["arn"]
        .as_str()
        .expect("the create answers the policy ARN")
        .to_string();

    vec![
        Answer {
            // The recorded row: a value outside the closed set its enum declares, refused
            // by the boundary validator while the operation was being served.
            label: "CreateSchedulingPolicy with a strategy outside its enum".to_string(),
            method: "POST",
            uri: "/v1/createschedulingpolicy".to_string(),
            payload: Some(json!({
                "name": "sp-reported-enum",
                "quotaSharePolicy": {"idleResourceAssignmentStrategy": "STRICT"},
            })),
            status: 400,
            stated: Stated::InTheEnvelope,
        },
        Answer {
            // A handler-raised enveloped refusal, on another operation.
            label: "CreateComputeEnvironment MANAGED with no computeResources".to_string(),
            method: "POST",
            uri: "/v1/createcomputeenvironment".to_string(),
            payload: Some(json!({"computeEnvironmentName": "ce-reported", "type": "MANAGED"})),
            status: 400,
            stated: Stated::InTheEnvelope,
        },
        Answer {
            // A handler-raised enveloped refusal of a name the member's pattern excludes.
            label: "CreateSchedulingPolicy with a name outside its pattern".to_string(),
            method: "POST",
            uri: "/v1/createschedulingpolicy".to_string(),
            payload: Some(json!({"name": "sp reported!"})),
            status: 400,
            stated: Stated::InTheEnvelope,
        },
        Answer {
            // A refusal Batch words BARE: its document states no id at all, so the header
            // is the only place a client can read one.
            label: "UpdateComputeEnvironment naming nothing stored".to_string(),
            method: "POST",
            uri: "/v1/updatecomputeenvironment".to_string(),
            payload: Some(json!({"computeEnvironment": "ce-reported-absent"})),
            status: 400,
            stated: Stated::Nothing,
        },
        Answer {
            // A bare refusal with a status of its own.
            label: "CreateSchedulingPolicy re-creating a stored name".to_string(),
            method: "POST",
            uri: "/v1/createschedulingpolicy".to_string(),
            payload: Some(json!({"name": "sp-reported"})),
            status: 409,
            stated: Stated::Nothing,
        },
        Answer {
            // The tagging layer's own refusal, under its own code and status.
            label: "ListTagsForResource on an ARN naming nothing".to_string(),
            method: "GET",
            uri: tags_uri(&crate::job_lifecycle::arn(
                "scheduling-policy/sp-reported-absent",
            )),
            payload: None,
            status: 404,
            stated: Stated::Nothing,
        },
        Answer {
            // The scaffold's 501: an answer a still-stubbed operation produced.
            label: "TerminateServiceJobs, a scaffolded operation".to_string(),
            method: "POST",
            uri: "/v1/terminateservicejobs".to_string(),
            payload: Some(json!({"jobs": ["sj-reported"], "reason": "because"})),
            status: 501,
            stated: Stated::Nothing,
        },
        Answer {
            // A SUCCESS, which names its request exactly as a failure does.
            label: "CreateSchedulingPolicy accepted".to_string(),
            method: "POST",
            uri: "/v1/createschedulingpolicy".to_string(),
            payload: Some(json!({
                "name": "sp-reported-accepted",
                "quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"},
            })),
            status: 200,
            stated: Stated::Nothing,
        },
        Answer {
            // A reporting success, on the `GET` route the tag reader binds.
            label: "ListTagsForResource on a stored policy".to_string(),
            method: "GET",
            uri: tags_uri(&policy_arn),
            payload: None,
            status: 200,
            stated: Stated::Nothing,
        },
        Answer {
            // An answer NO operation produced: a path this service's model binds no
            // operation to, rendered by the framework's routing-failure renderer.
            label: "a path naming no modelled operation".to_string(),
            method: "POST",
            uri: "/v1/thereisnosuchoperation".to_string(),
            payload: Some(json!({})),
            status: 501,
            stated: Stated::Nothing,
        },
    ]
}

/// RULE: every response this service sends names the request it answered in the
/// `x-amzn-requestid` header, and a response whose document states that id states the SAME
/// string the header reports.
#[tokio::test]
async fn every_response_names_the_request_it_answered() {
    let batch = Batch::new();

    for answer in answers(&batch).await {
        let case = &answer.label;
        let (status, headers, body) = batch
            .send_reported(answer.method, &answer.uri, answer.payload.clone())
            .await;
        assert_eq!(
            status, answer.status,
            "case {case}: expected HTTP {}, got {status} {body}",
            answer.status
        );

        let reported = headers
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected the response to name its request in \
                     {REQUEST_ID_HEADER:?}, got the headers {:?} and the body {body}",
                    headers.keys().collect::<Vec<_>>()
                )
            })
            .to_string();
        assert_uuid_shaped(case, &reported);

        match answer.stated {
            Stated::InTheEnvelope => {
                // The envelope is read the way a client reads it off the `message`, through
                // the one reader of its shape this target holds
                // (`refusal_contract::enveloped`).
                let (_, stated) = enveloped(case, &error_message(&body));
                assert_eq!(
                    stated, reported,
                    "case {case}: the id the message states and the id the header reports \
                     must be one string, got {stated:?} in the message and {reported:?} in \
                     the header"
                );
            }
            Stated::Nothing => {
                let message = error_message(&body);
                assert!(
                    !message.contains(ENVELOPE_REQUEST_ID),
                    "case {case}: this answer states no request id in its document, got \
                     {message:?}"
                );
            }
        }
    }
}

/// RULE (the id is the CALL's): two answers never name the same request, whether they are
/// successes, refusals that state their id or refusals that state none.
#[tokio::test]
async fn two_responses_never_name_the_same_request() {
    let batch = Batch::new();
    let mut reported: Vec<String> = Vec::new();

    for answer in answers(&batch).await {
        // Twice per row, so the id is shown to be per CALL rather than per answer kind.
        for attempt in 1..=2 {
            let case = format!("{} (attempt {attempt})", answer.label);
            let (_, headers, body) = batch
                .send_reported(answer.method, &answer.uri, answer.payload.clone())
                .await;
            let id = headers
                .get(REQUEST_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a reported request id, got the body {body}")
                })
                .to_string();
            reported.push(id);
        }
    }

    let mut distinct = reported.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        reported.len(),
        "case every answered call: expected one request id per call, got {reported:?}"
    );
}
