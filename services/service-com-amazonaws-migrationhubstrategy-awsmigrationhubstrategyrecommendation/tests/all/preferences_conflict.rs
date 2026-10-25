// SPDX-License-Identifier: CC0-1.0

//! Saving the account's portfolio preferences while one of its tasks is still running.
//!
//! The rule under test is not about one request: the preferences are the account-wide
//! inputs the service's recommendations are computed from, so `PutPortfolioPreferences` is
//! refused with the modelled `ConflictException` (HTTP 409) for as long as ANY task of the
//! account has not reached a terminal status, and the stored record is left exactly as it
//! was; once no task is running the same save is accepted and `GetPortfolioPreferences`
//! reports it.
//!
//! So the tables below span what the rule generalises over rather than the one recorded
//! pair of calls: each kind of task this service runs (an import file task, an assessment,
//! a recommendation report generation task) and two of them at once, crossed with save
//! requests differing in how many preference members they carry — the refusal turns on the
//! STORE, so no row of it may depend on what the request holds. The accepting half is
//! spanned too, over both ways a task leaves its non-terminal status: the clock completing
//! it, and `StopAssessment` stopping it.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::COMPLETION_SPAN;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{call, emulator, stop_assessment, Answer};

/// The wire error code a save refused for an ongoing task carries, and its status.
const CONFLICT_EXCEPTION: &str = "ConflictException";
const CONFLICT_STATUS: u16 = 409;

/// A bucket name carrying the `migrationhub-strategy-` prefix every bucket member of this
/// service documents, so a start call here is refused for nothing else.
const BUCKET: &str = "migrationhub-strategy-conflict-probe";

async fn put_preferences(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/put-portfolio-preferences", Some(body)).await
}

async fn get_preferences(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", "/get-portfolio-preferences", None).await
}

/// The record a read reports, as JSON: a record with no member set serializes to an empty
/// object, which an empty body and a literal `{}` both mean.
fn reported(answer: &Answer) -> Value {
    if answer.body.is_null() {
        json!({})
    } else {
        answer.body.clone()
    }
}

/// The start call that puts one task of a given kind in flight: its URI and its request.
struct Start {
    uri: &'static str,
    body: Value,
}

fn start_import() -> Start {
    Start {
        uri: "/start-import-file-task",
        body: json!({"name": "conflict-probe", "S3Bucket": BUCKET, "s3key": "servers.csv"}),
    }
}

fn start_assessment() -> Start {
    Start {
        uri: "/start-assessment",
        body: json!({"s3bucketForAnalysisData": BUCKET}),
    }
}

fn start_report() -> Start {
    Start {
        uri: "/start-recommendation-report-generation",
        body: json!({"outputFormat": "Json"}),
    }
}

/// Every way this account can hold work in flight: each kind of task on its own, and more
/// than one at once. The rule is about there BEING an ongoing task, so the rows differ in
/// which kind is running and in how many are.
fn ongoing_cases() -> Vec<(String, Vec<Start>)> {
    vec![
        (
            "an import file task importing".to_string(),
            vec![start_import()],
        ),
        (
            "an assessment collecting data".to_string(),
            vec![start_assessment()],
        ),
        (
            "a recommendation report generating".to_string(),
            vec![start_report()],
        ),
        (
            "an import file task and an assessment at once".to_string(),
            vec![start_import(), start_assessment()],
        ),
        (
            "all three kinds of task at once".to_string(),
            vec![start_import(), start_assessment(), start_report()],
        ),
    ]
}

/// The saves each row is tried with: they differ in which preference members they carry
/// and how many, so a refusal that depended on the request would show up here.
fn save_requests() -> Vec<(String, Value)> {
    vec![
        ("no member at all".to_string(), json!({})),
        (
            "applicationMode only".to_string(),
            json!({"applicationMode": "ALL"}),
        ),
        (
            "business goals only".to_string(),
            json!({"prioritizeBusinessGoals": {"businessGoals": {"speedOfMigration": 1}}}),
        ),
        ("every preference member".to_string(), every_member()),
    ]
}

/// A request carrying all four preference members at once — the widest record the
/// operation admits, used as the record a refused save must not disturb.
fn every_member() -> Value {
    json!({
        "applicationMode": "KNOWN",
        "applicationPreferences": {
            "managementPreference": {
                "awsManagedResources": {"targetDestination": ["AWS Fargate"]}
            }
        },
        "databasePreferences": {
            "databaseManagementPreference": "AWS-managed",
            "databaseMigrationPreference": {
                "heterogeneous": {"targetDatabaseEngine": ["Amazon Aurora"]}
            }
        },
        "prioritizeBusinessGoals": {
            "businessGoals": {
                "speedOfMigration": 1,
                "licenseCostReduction": 5,
            }
        },
    })
}

/// Save a record on a quiet account, so the rows that follow have something to prove was
/// left untouched.
async fn establish(router: &MultiServiceRouter, case: &str) -> Value {
    let request = every_member();
    let saved = put_preferences(router, request.clone()).await;
    assert_eq!(
        saved.status, 200,
        "case {case:?}: a save with nothing running is accepted, got {} with {:?}",
        saved.status, saved.body
    );
    request
}

/// Put every start of a row in flight, answering the instant the first one started from.
async fn start_all(router: &MultiServiceRouter, case: &str, starts: &[Start]) -> Instant {
    let at = Instant::now();
    for start in starts {
        let started = call(router, "POST", start.uri, Some(start.body.clone())).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: starting {} answered {} with {:?}",
            start.uri, started.status, started.body
        );
    }
    at
}

/// A save made while any task of the account is running is refused with 409
/// `ConflictException`, and the stored record is left exactly as it was.
#[tokio::test]
async fn a_save_during_an_ongoing_task_is_refused_with_the_modelled_conflict() {
    // How many rows really observed the conflict: the guard below skips a row whose tasks
    // the clock finished before it got to assert anything, so the table has to state that
    // it did not skip everything.
    let mut observed = 0usize;
    for (case, starts) in ongoing_cases() {
        // A fresh emulator per row, so the only work in flight is the row's own.
        let router = emulator();
        let established = establish(&router, &case).await;
        let started_at = start_all(&router, &case, &starts).await;

        for (save, request) in save_requests() {
            if started_at.elapsed() >= COMPLETION_SPAN {
                // The clock has already completed the row's tasks, so there is no
                // conflict left to observe and the row proves nothing: it is skipped
                // rather than asserted wrongly. The same guard the other suites use.
                break;
            }
            let case = format!("{case}, saving {save}");
            let refused = put_preferences(&router, request).await;
            assert_eq!(
                refused.status, CONFLICT_STATUS,
                "case {case:?}: expected HTTP {CONFLICT_STATUS}, got {} with {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                CONFLICT_EXCEPTION,
                "case {case:?}: expected {CONFLICT_EXCEPTION}, got {:?}",
                refused.body
            );
            assert!(
                !refused.string("/message").is_empty(),
                "case {case:?}: the conflict carried no message, got {:?}",
                refused.body
            );

            let got = get_preferences(&router).await;
            assert_eq!(
                reported(&got),
                established,
                "case {case:?}: a refused save changed the record, expected {established:?}, \
                 got {:?}",
                reported(&got)
            );
            observed += 1;
        }
    }
    assert!(
        observed > 0,
        "no row observed the conflict at all: expected at least one of the {} rows to be \
         asserted inside the completion span",
        ongoing_cases().len()
    );
}

/// The conflict is lifted by the task finishing, not by anything the caller sends: once
/// the clock has given the row's tasks their terminal status the same save is accepted and
/// the read reports it.
#[tokio::test]
async fn a_save_is_accepted_once_the_clock_has_completed_the_tasks() {
    for (case, starts) in ongoing_cases() {
        let router = emulator();
        let _ = establish(&router, &case).await;
        let _ = start_all(&router, &case, &starts).await;

        // Nothing is done here but wait: no call drives the transition.
        std::thread::sleep(COMPLETION_SPAN + Duration::from_millis(200));

        let request = json!({"applicationMode": "UNKNOWN"});
        let saved = put_preferences(&router, request.clone()).await;
        assert_eq!(
            saved.status, 200,
            "case {case:?}: with every task finished the save is accepted, got {} with {:?}",
            saved.status, saved.body
        );
        let got = get_preferences(&router).await;
        assert_eq!(
            reported(&got),
            request,
            "case {case:?}: expected the saved record {request:?}, got {:?}",
            reported(&got)
        );
    }
}

/// An assessment stopped before its span has passed is terminal too, so the save is
/// accepted as soon as the stop is in — without waiting for any clock.
#[tokio::test]
async fn a_save_is_accepted_once_the_only_task_has_been_stopped() {
    let router = emulator();
    let established = establish(&router, "a stopped assessment").await;

    let started_at = Instant::now();
    let id = call(
        &router,
        "POST",
        "/start-assessment",
        Some(json!({"s3bucketForAnalysisData": BUCKET})),
    )
    .await
    .string("/assessmentId");

    let refused = put_preferences(&router, json!({"applicationMode": "ALL"})).await;
    if started_at.elapsed() < COMPLETION_SPAN {
        assert_eq!(
            refused.status, CONFLICT_STATUS,
            "case \"before the stop\": expected HTTP {CONFLICT_STATUS}, got {} with {:?}",
            refused.status, refused.body
        );
        let got = get_preferences(&router).await;
        assert_eq!(
            reported(&got),
            established,
            "case \"before the stop\": a refused save changed the record, got {:?}",
            reported(&got)
        );
    }

    let stopped = stop_assessment(&router, &id).await;
    assert_eq!(
        stopped.status, 200,
        "case \"the stop itself\": expected HTTP 200, got {} with {:?}",
        stopped.status, stopped.body
    );

    let request = json!({"applicationMode": "KNOWN"});
    let saved = put_preferences(&router, request.clone()).await;
    assert_eq!(
        saved.status, 200,
        "case \"after the stop\": a stopped assessment is terminal, so the save is accepted, \
         got {} with {:?}",
        saved.status, saved.body
    );
    let got = get_preferences(&router).await;
    assert_eq!(
        reported(&got),
        request,
        "case \"after the stop\": expected the saved record {request:?}, got {:?}",
        reported(&got)
    );
}

/// A request the service cannot accept at all is still refused for the value it carries
/// while a task is running: the conflict does not mask a validation failure, and neither
/// refusal writes anything.
#[tokio::test]
async fn an_undeclared_value_is_still_refused_as_a_validation_failure() {
    let router = emulator();
    let established = establish(&router, "an invalid save during a task").await;
    let started_at = start_all(
        &router,
        "an invalid save during a task",
        &[start_assessment()],
    )
    .await;

    let invalid: Vec<(&str, Value)> = vec![
        (
            "an applicationMode ApplicationMode does not declare",
            json!({"applicationMode": "SOME"}),
        ),
        (
            "a databaseManagementPreference the model does not declare",
            json!({"databasePreferences": {"databaseManagementPreference": "Managed"}}),
        ),
    ];

    for (case, request) in invalid {
        if started_at.elapsed() >= COMPLETION_SPAN {
            break;
        }
        let refused = put_preferences(&router, request).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
        let got = get_preferences(&router).await;
        assert_eq!(
            reported(&got),
            established,
            "case {case:?}: a refused save changed the record, got {:?}",
            reported(&got)
        );
    }
}
