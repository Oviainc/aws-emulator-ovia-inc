// SPDX-License-Identifier: CC0-1.0

//! The job-execution STATE MACHINE the device-facing operations enforce.
//!
//! Four rules, each asserted over a SET of inputs rather than one recorded case:
//!
//! 1. a device update may hold an execution at its lifecycle stage or carry it
//!    FORWARD (queued -> taken up -> finished), never back: an execution already
//!    taken up can neither return to the queue nor be rejected, and the refusal is
//!    total (409 `InvalidStateTransitionException`, nothing written);
//! 2. an `expectedVersion` that is not EXACTLY the stored version is the request's
//!    fault — 400 `InvalidRequestException` naming both versions — and changes
//!    nothing, whether the version named is in the past or in the future;
//! 3. a `statusDetails` map the request CARRIES replaces the stored map wholesale,
//!    and an OMITTED one leaves the stored map exactly as it was;
//! 4. `StartNextPendingJobExecution` starts a QUEUED execution and resolves again to
//!    one already in progress: a poll carrying NEITHER request member re-hands it
//!    unchanged (same version, same `startedAt`, same details), a poll carrying one
//!    applies it as a device update, and either way the next queued execution waits
//!    for the one in progress to finish.
//!
//! Every request goes over the wire (restJson1 through the multi-service router), so
//! what is asserted is the HTTP behaviour a device agent sees.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::ops::types::JOB_EXECUTION_STATUS_VALUES;
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    command_registry, iot_commands::CommandFacts,
};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name, which is how the router picks the service for a request.
const SIGV4: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/iot-jobs-data/aws4_request, SignedHeaders=host, Signature=0";

const COMMAND_ARN: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot";

/// The two statuses an execution is pending in, as `GetPendingJobExecutions`
/// documents them ("not in a terminal status").
const PENDING_STATUSES: &[&str] = &["QUEUED", "IN_PROGRESS"];

/// One emulator holding one shared engine, driven over HTTP.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // Commands are control-plane resources (`iot:CreateCommand`, a scaffold in
        // its own crate under its own issue), so the shared index they are
        // registered in is seeded here with the one command these tests start.
        command_registry(&*engine)
            .expect("the shared IoT command index")
            .lock()
            .register(COMMAND_ARN, CommandFacts::default())
            .expect("register the command these tests start");
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map_or_else(Bytes::new, |body| Bytes::from(body.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://data.jobs.iot.us-east-1.amazonaws.com{path}"))
            .header("authorization", SIGV4)
            .header("content-type", "application/json")
            .body(payload)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Enqueue one execution for `thing_name` and return its job id.
    async fn enqueue_on(&self, thing_name: &str) -> String {
        let (status, body) = self
            .call(
                "POST",
                "/command-executions",
                Some(json!({
                    "targetArn": format!("arn:aws:iot:us-east-1:111122223333:thing/{thing_name}"),
                    "commandArn": COMMAND_ARN,
                })),
            )
            .await;
        assert_eq!(
            status, 200,
            "thing {thing_name}: expected 200 from StartCommandExecution, got {status} {body}"
        );
        body["executionId"]
            .as_str()
            .unwrap_or_else(|| panic!("thing {thing_name}: expected an executionId, got {body}"))
            .to_string()
    }

    async fn pending(&self, thing_name: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing_name}/jobs"), None)
            .await
    }

    async fn start_next(&self, thing_name: &str, body: Value) -> (u16, Value) {
        self.call(
            "PUT",
            &format!("/things/{thing_name}/jobs/$next"),
            Some(body),
        )
        .await
    }

    async fn describe(&self, thing_name: &str, job_id: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing_name}/jobs/{job_id}"), None)
            .await
    }

    async fn update(&self, thing_name: &str, job_id: &str, body: Value) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/things/{thing_name}/jobs/{job_id}"),
            Some(body),
        )
        .await
    }
}

/// The wire error code of a modeled failure (restJson1 carries it in `__type`).
fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("")
}

/// The `jobId`s of one pending list, in the order the service returned them.
fn job_ids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap_or_else(|| panic!("expected a list of summaries, got {list}"))
        .iter()
        .map(|summary| summary["jobId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// How far through its lifecycle a status leaves an execution: enqueued, taken up by
/// its device, finished. Every value of the model's `JobExecutionStatus` enum that is
/// not one of the two pending ones ends the execution.
fn lifecycle_stage(status: &str) -> u8 {
    match status {
        "QUEUED" => 0,
        "IN_PROGRESS" => 1,
        _ => 2,
    }
}

/// THE RULE under test in [`a_status_update_may_never_carry_an_execution_backward`],
/// stated independently of the handler: an update may repeat an execution's own stage
/// or carry it forward, and `REJECTED` is reachable only from `QUEUED` — a device
/// rejects work it has not taken up.
fn transition_is_legal(current: &str, requested: &str) -> bool {
    if requested == "REJECTED" {
        return current == "QUEUED";
    }
    lifecycle_stage(requested) >= lifecycle_stage(current)
}

/// RULE: a device update may hold an execution at its lifecycle stage or carry it
/// FORWARD, never back — so an execution a device already took up accepts neither
/// `QUEUED` nor `REJECTED`, while `QUEUED` -> any terminal status (skipping
/// `IN_PROGRESS`) is accepted. A refused update is TOTAL: 409
/// `InvalidStateTransitionException` and neither the status nor the version moves.
///
/// The table is the whole cross product of the two pending stages an execution can be
/// updated from with EVERY value of the model's `JobExecutionStatus` catalog
/// (`JOB_EXECUTION_STATUS_VALUES`, rendered from the Smithy model), so it covers the
/// statuses the recorded cases never named.
#[tokio::test]
async fn a_status_update_may_never_carry_an_execution_backward() {
    let emulator = Emulator::new();

    for current in PENDING_STATUSES {
        for requested in JOB_EXECUTION_STATUS_VALUES {
            let case = format!("{current} -> {requested}");
            let thing = format!(
                "laser-{}-{}",
                current.to_lowercase().replace('_', "-"),
                requested.to_lowercase().replace('_', "-")
            );
            let id = emulator.enqueue_on(&thing).await;

            // Arrange the stage the update starts from. `$next` is what takes a
            // queued execution up, so it is also what moves the version to 2.
            let mut version = 1_i64;
            if *current == "IN_PROGRESS" {
                let (status, started) = emulator.start_next(&thing, json!({})).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 arranging the stage, got {status} {started}"
                );
                version = started["execution"]["versionNumber"]
                    .as_i64()
                    .unwrap_or_default();
                assert_eq!(
                    started["execution"]["status"], "IN_PROGRESS",
                    "case {case:?}: $next must take the execution up, got {started}"
                );
            }

            let (status, body) = emulator
                .update(
                    &thing,
                    &id,
                    json!({
                        "status": requested,
                        "expectedVersion": version,
                        "includeJobExecutionState": true,
                    }),
                )
                .await;

            if transition_is_legal(current, requested) {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a forward move is accepted, got {status} {body}"
                );
                assert_eq!(
                    body["executionState"]["status"], *requested,
                    "case {case:?}: expected the stored status to be {requested}, got {body}"
                );
                assert_eq!(
                    body["executionState"]["versionNumber"].as_i64(),
                    Some(version + 1),
                    "case {case:?}: an accepted update advances the version, got {body}"
                );
                continue;
            }

            assert_eq!(
                status, 409,
                "case {case:?}: a backward move is refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidStateTransitionException",
                "case {case:?}: expected InvalidStateTransitionException, got {body}"
            );
            // The refusal is total: the execution is still exactly where it was, so
            // it is still the thing's pending work at the version it had.
            let (status, described) = emulator.describe(&thing, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the refused update leaves a live execution, got {status} {described}"
            );
            assert_eq!(
                (
                    described["execution"]["status"].as_str(),
                    described["execution"]["versionNumber"].as_i64()
                ),
                (Some(*current), Some(version)),
                "case {case:?}: expected {current} at version {version} after the refusal, got {described}"
            );
            let list = if *current == "QUEUED" {
                "queuedJobs"
            } else {
                "inProgressJobs"
            };
            let (_, pending) = emulator.pending(&thing).await;
            assert_eq!(
                job_ids(&pending[list]),
                vec![id.clone()],
                "case {case:?}: the refused update must leave the execution in {list}, got {pending}"
            );
        }
    }
}

/// RULE: `expectedVersion` is an EXACT-match optimistic-concurrency guard, and a
/// request that names any other version is the request's fault: 400
/// `InvalidRequestException` whose message names the version asked for and the
/// version stored, with status, version and `statusDetails` all left untouched. The
/// stored version itself is accepted and advances by one.
///
/// The rows differ in how far the named version is from the stored one, and on which
/// side of it (the past and the future alike), and the whole table is run at two
/// different stored versions.
#[tokio::test]
async fn a_mismatched_expected_version_is_a_bad_request_that_changes_nothing() {
    let emulator = Emulator::new();

    // (case, how many updates to apply before the guard is tested)
    for (case, warmups) in [("a freshly queued execution", 0), ("after two updates", 2)] {
        let thing = format!("laser-guard-{warmups}");
        let id = emulator.enqueue_on(&thing).await;

        let mut stored = 1_i64;
        for _ in 0..warmups {
            let (status, body) = emulator
                .update(
                    &thing,
                    &id,
                    json!({"status": "IN_PROGRESS", "statusDetails": {"stage": "download"}}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 arranging the version, got {status} {body}"
            );
            stored += 1;
        }

        // Offsets from the stored version: the immediate past and future, further
        // out on each side, and zero (which no execution ever carries).
        let named: Vec<i64> = [-1_i64, 1, -(stored.min(5)), 5, 97]
            .iter()
            .map(|offset| stored + offset)
            .chain(std::iter::once(0))
            .filter(|version| *version != stored)
            .collect();

        for expected in named {
            let (status, refused) = emulator
                .update(
                    &thing,
                    &id,
                    json!({
                        "status": "SUCCEEDED",
                        "statusDetails": {"stage": "ignored"},
                        "expectedVersion": expected,
                    }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} expectedVersion={expected}: expected 400, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "InvalidRequestException",
                "case {case:?} expectedVersion={expected}: expected InvalidRequestException, got {refused}"
            );
            // The message is built from the request and the stored record, so it
            // names both versions.
            let wanted = format!("Expected version {expected} but found version {stored}");
            assert_eq!(
                refused["message"].as_str(),
                Some(wanted.as_str()),
                "case {case:?} expectedVersion={expected}: expected {wanted:?}, got {refused}"
            );

            let (status, described) = emulator.describe(&thing, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?} expectedVersion={expected}: the refused update leaves a live execution, got {status} {described}"
            );
            let details = if warmups == 0 {
                Value::Null
            } else {
                json!({"stage": "download"})
            };
            assert_eq!(
                (
                    described["execution"]["versionNumber"].as_i64(),
                    described["execution"]["status"].as_str(),
                    described["execution"]["statusDetails"].clone(),
                ),
                (
                    Some(stored),
                    Some(if warmups == 0 { "QUEUED" } else { "IN_PROGRESS" }),
                    details
                ),
                "case {case:?} expectedVersion={expected}: the refused update must change nothing, got {described}"
            );
        }

        // The stored version is the one the guard accepts, and it advances by one.
        let (status, accepted) = emulator
            .update(
                &thing,
                &id,
                json!({
                    "status": "IN_PROGRESS",
                    "expectedVersion": stored,
                    "includeJobExecutionState": true,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored version is accepted, got {status} {accepted}"
        );
        assert_eq!(
            accepted["executionState"]["versionNumber"].as_i64(),
            Some(stored + 1),
            "case {case:?}: the accepted update advances the version, got {accepted}"
        );

        // And the version it consumed is now one of the refused ones.
        let (status, refused) = emulator
            .update(
                &thing,
                &id,
                json!({"status": "IN_PROGRESS", "expectedVersion": stored}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a consumed version is refused, got {status} {refused}"
        );
    }
}

/// RULE: a `statusDetails` map the request CARRIES replaces the stored map wholesale
/// — a name the device no longer sends is dropped — while an OMITTED member leaves
/// the stored map exactly as it was ("If not specified, the statusDetails are
/// unchanged"). It holds for both operations that accept the member, and the stored
/// map is what every later read reports.
///
/// The rows differ in the size of the map written, in how much it overlaps the stored
/// one, and in whether the member is sent at all (including as an empty map, which
/// leaves the execution with no details to report).
#[tokio::test]
async fn a_supplied_status_details_map_replaces_the_stored_one_and_an_omitted_one_keeps_it() {
    let emulator = Emulator::new();
    let thing = "laser-details";
    let id = emulator.enqueue_on(thing).await;

    // The map `$next` writes is the first stored map.
    let (status, started) = emulator
        .start_next(
            thing,
            json!({"statusDetails": {"stage": "download", "attempt": "1"}}),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected 200 from $next, got {status} {started}"
    );
    assert_eq!(
        started["execution"]["statusDetails"],
        json!({"stage": "download", "attempt": "1"}),
        "$next must store the details it was sent, got {started}"
    );

    // (case, the map the update carries, the details every later read must report)
    let rows: Vec<(&str, Option<Value>, Value)> = vec![
        (
            "a smaller map drops the names it omits",
            Some(json!({"stage": "verify"})),
            json!({"stage": "verify"}),
        ),
        (
            "a disjoint map replaces every name",
            Some(json!({"phase": "install", "retry": "0"})),
            json!({"phase": "install", "retry": "0"}),
        ),
        (
            "a larger map replaces it too",
            Some(json!({"a": "1", "b": "2", "c": "3"})),
            json!({"a": "1", "b": "2", "c": "3"}),
        ),
        (
            "an omitted member keeps the stored map",
            None,
            json!({"a": "1", "b": "2", "c": "3"}),
        ),
        (
            "one name overwrites the whole map",
            Some(json!({"a": "9"})),
            json!({"a": "9"}),
        ),
        (
            // No pair left to report, which this service answers as no map at all
            // rather than an empty one.
            "an empty map leaves nothing to report",
            Some(json!({})),
            Value::Null,
        ),
        (
            "an omitted member keeps even an empty map",
            None,
            Value::Null,
        ),
    ];

    for (case, written, expected) in rows {
        let mut request = json!({"status": "IN_PROGRESS", "includeJobExecutionState": true});
        if let Some(written) = written.clone() {
            request["statusDetails"] = written;
        }
        let (status, body) = emulator.update(thing, &id, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["executionState"]["statusDetails"], expected,
            "case {case:?}: expected the update to report {expected}, got {body}"
        );

        // And the stored map is what a later read reports.
        let (status, described) = emulator.describe(thing, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 reading it back, got {status} {described}"
        );
        assert_eq!(
            described["execution"]["statusDetails"], expected,
            "case {case:?}: expected the read to report {expected}, got {described}"
        );
    }
}

/// RULE: `StartNextPendingJobExecution` STARTS a queued execution (version +1,
/// `startedAt` stamped) and resolves again to one the thing already has in progress: a
/// poll carrying NEITHER request member re-hands that execution unchanged, so the
/// version the device holds stays usable as an `expectedVersion` however often it polls
/// for nothing, while a poll that carries `statusDetails` or `stepTimeoutInMinutes`
/// applies it as a device update and hands back the version that update produced.
/// Either way the next queued execution is handed out only once the one in progress
/// reaches a terminal status, and `startedAt` is the first start's.
///
/// The rows differ in what the re-poll carries (nothing, details, a step timeout,
/// both), which is what decides whether it writes.
#[tokio::test]
async fn re_polling_an_in_progress_execution_applies_only_what_it_carries_and_the_queue_waits() {
    let emulator = Emulator::new();
    let thing = "laser-rehand";

    let first = emulator.enqueue_on(thing).await;
    let second = emulator.enqueue_on(thing).await;

    let (status, started) = emulator
        .start_next(thing, json!({"statusDetails": {"stage": "download"}}))
        .await;
    assert_eq!(
        status, 200,
        "expected 200 from the first $next, got {status} {started}"
    );
    let handed = started["execution"].clone();
    assert_eq!(
        (
            handed["jobId"].as_str(),
            handed["status"].as_str(),
            handed["versionNumber"].as_i64()
        ),
        (Some(first.as_str()), Some("IN_PROGRESS"), Some(2)),
        "starting the oldest queued execution moves its status and version, got {started}"
    );
    assert!(
        handed["startedAt"].as_i64().unwrap_or_default() > 0,
        "the start stamps startedAt, got {started}"
    );

    // The version the device currently holds, and the details the execution currently
    // carries: both move only with a poll that carries something to apply.
    let mut version = handed["versionNumber"].as_i64().unwrap_or_default();
    let mut details = handed["statusDetails"].clone();

    // (case, the body of the re-poll, the details it leaves stored or `None` when it
    //  carries no map)
    for (case, body, written) in [
        ("an empty re-poll", json!({}), None),
        (
            "a re-poll carrying details",
            json!({"statusDetails": {"stage": "verify"}}),
            Some(json!({"stage": "verify"})),
        ),
        (
            "a re-poll carrying a step timeout",
            json!({"stepTimeoutInMinutes": 7}),
            None,
        ),
        (
            "a re-poll carrying both",
            json!({"statusDetails": {"stage": "install"}, "stepTimeoutInMinutes": 10}),
            Some(json!({"stage": "install"})),
        ),
    ] {
        let applies = body.as_object().is_some_and(|body| !body.is_empty());
        let (status, again) = emulator.start_next(thing, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {again}"
        );
        version += i64::from(applies);
        if let Some(written) = written {
            details = written;
        }
        assert_eq!(
            (
                again["execution"]["jobId"].as_str(),
                again["execution"]["status"].as_str(),
                again["execution"]["versionNumber"].as_i64(),
                again["execution"]["startedAt"].as_i64(),
                again["execution"]["statusDetails"].clone(),
            ),
            (
                handed["jobId"].as_str(),
                handed["status"].as_str(),
                Some(version),
                handed["startedAt"].as_i64(),
                details.clone(),
            ),
            "case {case:?}: expected the same execution at version {version} carrying {details}, got {again}"
        );

        // The other execution is still waiting, and only that one.
        let (_, pending) = emulator.pending(thing).await;
        assert_eq!(
            job_ids(&pending["queuedJobs"]),
            vec![second.clone()],
            "case {case:?}: the next queued execution waits for the one in progress, got {pending}"
        );
        assert_eq!(
            job_ids(&pending["inProgressJobs"]),
            vec![first.clone()],
            "case {case:?}: expected only the started execution in progress, got {pending}"
        );
    }

    // The version the last poll handed out is the current one, so the device's own
    // update is accepted naming it.
    let (status, finished) = emulator
        .update(
            thing,
            &first,
            json!({
                "status": "SUCCEEDED",
                "expectedVersion": version,
                "includeJobExecutionState": true,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the version the last poll handed out is current, got {status} {finished}"
    );
    assert_eq!(
        finished["executionState"]["versionNumber"].as_i64(),
        Some(version + 1),
        "finishing it advances the version once, got {finished}"
    );

    // Only now is the queued execution handed out, and starting it is a real start.
    let (status, next) = emulator.start_next(thing, json!({})).await;
    assert_eq!(status, 200, "expected 200 from $next, got {status} {next}");
    assert_eq!(
        (
            next["execution"]["jobId"].as_str(),
            next["execution"]["status"].as_str(),
            next["execution"]["versionNumber"].as_i64(),
        ),
        (Some(second.as_str()), Some("IN_PROGRESS"), Some(2)),
        "the next queued execution is started once the previous one finished, got {next}"
    );
}
