// SPDX-License-Identifier: CC0-1.0

//! The refusal every operation that takes a job's CAPACITY answers when the request names
//! that capacity both ways: `CreateJob`, `UpdateJob` and `StartJobRun`.
//!
//! The rule under test: "do not set `MaxCapacity` if using `WorkerType` and
//! `NumberOfWorkers`" (`CreateJobRequest$MaxCapacity`) — a request carrying `MaxCapacity`
//! together with either of the worker members is refused with HTTP 400
//! `InvalidInputException` and ONE fixed sentence of the service's own, the same sentence
//! whichever of the three operations refused it and whatever values the request carried;
//! a request naming its capacity only ONE way is served.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! worker member accompanies `MaxCapacity`, how many of them do, the capacity and worker
//! values themselves, and which operation the request was sent to. The recorded case — a
//! create carrying `MaxCapacity` beside `WorkerType` and `NumberOfWorkers` — is one row.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok};

/// The role ARN every job definition below is created with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// The whole sentence the service refuses a doubly-named capacity with: fixed text with no
/// variable part, stating the two capacity members in the service's prose spelling and
/// ending in a period. Shared by `CreateJob`, `UpdateJob` and `StartJobRun`, which is what
/// the rows below assert of each.
const CAPACITY_REFUSAL: &str =
    "Please do not set Max Capacity if using Worker Type and Number of Workers.";

/// A `CreateJob` body: the `@required` members plus whatever `extra` adds.
fn create_body(name: &str, extra: &Value) -> Value {
    let mut body = json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/x.py" },
    });
    merge(&mut body, extra);
    body
}

/// A `JobUpdate` carrying the definition's `@required` members plus whatever `extra` adds,
/// which is what `UpdateJob` submits as the whole definition.
fn update_body(name: &str, extra: &Value) -> Value {
    let mut update = json!({
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/x.py" },
    });
    merge(&mut update, extra);
    json!({ "JobName": name, "JobUpdate": update })
}

/// A `StartJobRun` body for the job `name`, carrying whatever `extra` adds.
fn start_body(name: &str, extra: &Value) -> Value {
    let mut body = json!({ "JobName": name });
    merge(&mut body, extra);
    body
}

/// Copy every member of `extra` onto `body`.
fn merge(body: &mut Value, extra: &Value) {
    let members = extra.as_object().expect("the capacity members are an object");
    for (member, value) in members {
        body[member] = value.clone();
    }
}

/// The capacity members a request may carry two ways at once. Each row names
/// `MaxCapacity` beside at least one worker member, which is the ambiguity the rule
/// refuses; the values differ across rows so nothing can be keyed to one of them.
fn ambiguous_capacities() -> Vec<(&'static str, Value)> {
    vec![
        (
            // The recorded case: both worker members beside a maximum capacity.
            "a maximum capacity beside a worker type and a worker count",
            json!({ "MaxCapacity": 2.0, "WorkerType": "G.1X", "NumberOfWorkers": 2 }),
        ),
        (
            "a maximum capacity beside a worker type alone",
            json!({ "MaxCapacity": 10.0, "WorkerType": "G.2X" }),
        ),
        (
            "a maximum capacity beside a worker count alone",
            json!({ "MaxCapacity": 0.0625, "NumberOfWorkers": 17 }),
        ),
        (
            "a fractional maximum capacity beside the largest worker type",
            json!({ "MaxCapacity": 1.0, "WorkerType": "G.8X", "NumberOfWorkers": 1 }),
        ),
    ]
}

/// Assert that `operation` refused `body` with the one sentence the service words this
/// refusal as.
async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) {
    let (status, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?} / {operation}: expected 400, got {status} with {answer}"
    );
    assert!(
        error_code(&answer).contains("InvalidInputException"),
        "case {case:?} / {operation}: expected InvalidInputException, got {answer}"
    );
    let message = error_message(&answer);
    assert_eq!(
        message, CAPACITY_REFUSAL,
        "case {case:?} / {operation}: expected the service's own sentence {CAPACITY_REFUSAL:?}, \
         got {message:?}"
    );
}

/// A create, an update and a start that each name a job's capacity both ways are refused
/// with the same status, the same modelled error and the same sentence — and the refused
/// write stores nothing.
#[tokio::test]
async fn a_capacity_named_both_ways_is_refused_with_one_sentence() {
    let router = emulator();
    // One stored definition for the update and the start to address, created with no
    // capacity members at all so the refusals below are about the submitted request.
    ok(&router, "CreateJob", &create_body("capacity", &json!({}))).await;
    let stored = ok(&router, "GetJob", &json!({ "JobName": "capacity" })).await;

    for (case, capacity) in ambiguous_capacities() {
        assert_refused(
            &router,
            case,
            "CreateJob",
            &create_body("created-by-this-row", &capacity),
        )
        .await;
        // The refused create stored nothing: the name it asked for is still free.
        let (status, answer) = call(
            &router,
            "GetJob",
            &json!({ "JobName": "created-by-this-row" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must store no job definition, got {status} with \
             {answer}"
        );

        assert_refused(&router, case, "UpdateJob", &update_body("capacity", &capacity)).await;
        assert_refused(&router, case, "StartJobRun", &start_body("capacity", &capacity)).await;

        // The refused update and start left the stored definition exactly as it was.
        let after = ok(&router, "GetJob", &json!({ "JobName": "capacity" })).await;
        assert_eq!(
            after, stored,
            "case {case:?}: a refused request must leave the stored definition untouched; \
             expected {stored}, got {after}"
        );
        let runs = ok(&router, "GetJobRuns", &json!({ "JobName": "capacity" })).await;
        let started = runs["JobRuns"].as_array().map_or(0, Vec::len);
        assert_eq!(
            started, 0,
            "case {case:?}: a refused start must record no run, got {runs}"
        );
    }
}

/// A capacity named only ONE way is not this refusal's business: a maximum capacity alone,
/// and a worker type with a worker count alone, are each served by all three operations.
#[tokio::test]
async fn a_capacity_named_one_way_is_served() {
    // (case, the capacity members the request carries)
    let admitted: Vec<(&str, Value)> = vec![
        ("a maximum capacity alone", json!({ "MaxCapacity": 2.0 })),
        (
            "a worker type with a worker count",
            json!({ "WorkerType": "G.1X", "NumberOfWorkers": 2 }),
        ),
        ("a worker type alone", json!({ "WorkerType": "G.2X" })),
        ("a worker count alone", json!({ "NumberOfWorkers": 5 })),
    ];

    for (index, (case, capacity)) in admitted.iter().enumerate() {
        let router = emulator();
        let name = format!("served-{index}");
        let created = ok(&router, "CreateJob", &create_body(&name, capacity)).await;
        assert_eq!(
            created["Name"],
            json!(name),
            "case {case:?}: expected the create to report the job's name, got {created}"
        );

        let updated = ok(&router, "UpdateJob", &update_body(&name, capacity)).await;
        assert_eq!(
            updated["JobName"],
            json!(name),
            "case {case:?}: expected the update to report the job's name, got {updated}"
        );

        let started = ok(&router, "StartJobRun", &start_body(&name, capacity)).await;
        assert!(
            started["JobRunId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected the start to report the run it recorded, got {started}"
        );
    }
}
