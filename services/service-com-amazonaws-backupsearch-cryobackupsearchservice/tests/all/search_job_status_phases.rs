// SPDX-License-Identifier: CC0-1.0

//! The STATUS PHASES a search job passes through, driven over HTTP as a client drives
//! them.
//!
//! The rule under test: a resource this service accepts in a non-terminal status reports
//! every phase the service puts it through, in order and each for a non-zero span — a
//! search job is `CREATING` when `StartSearchJob` answers, `RUNNING` once its scan has
//! begun, and only then the terminal status its resolved scope owes it. No phase is
//! skipped, a read never advances one, and every operation that reports or judges the
//! job's status (`GetSearchJob`, `ListSearchJobs`, and the export gate that refuses a job
//! whose results have not settled) reports the SAME phase.
//!
//! Each assertion is made over a TABLE of start requests differing in the members the
//! rule generalises over — resource type, scope filters, name, tags, signing region, and
//! whether the scope resolves to anything, which is what decides the terminal status the
//! walk ends at — rather than over the one request the issue happened to cite.

use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{
    call, emulator_with_engine, recovery_point, seed_recovery_points, PAST_THE_CREATING_INSTANT,
    PAST_THE_DUE_INSTANT, WITHIN_THE_DUE_INSTANT,
};

const REGION: &str = "us-west-2";

/// One row of the table: the start request, the recovery points its scope is resolved
/// against, and the terminal status the walk therefore ends at.
struct Row {
    case: &'static str,
    region: &'static str,
    body: Value,
    held: Vec<types::SearchJobBackupsResult>,
    terminal: &'static str,
}

/// An S3 recovery point of the backup index, for the rows whose scope resolves to
/// something.
fn s3_point(suffix: &str) -> types::SearchJobBackupsResult {
    recovery_point(
        "S3",
        &format!("arn:aws:backup:us-west-2:000000000000:recovery-point:rp-{suffix}"),
        &format!("arn:aws:s3:::bucket-{suffix}"),
        SystemTime::now(),
    )
}

/// An EBS recovery point of the backup index.
fn ebs_point(suffix: &str) -> types::SearchJobBackupsResult {
    recovery_point(
        "EBS",
        &format!("arn:aws:backup:eu-west-1:000000000000:recovery-point:rp-{suffix}"),
        &format!("arn:aws:ec2:eu-west-1:000000000000:volume/vol-{suffix}"),
        SystemTime::now(),
    )
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a bare S3 scope resolving to nothing",
            region: REGION,
            body: json!({"SearchScope": {"BackupResourceTypes": ["S3"]}}),
            held: Vec::new(),
            terminal: state::FAILED,
        },
        Row {
            case: "a named S3 scope resolving to one recovery point",
            region: REGION,
            body: json!({
                "Name": "phases-s3",
                "SearchScope": {"BackupResourceTypes": ["S3"]}
            }),
            held: vec![s3_point("one")],
            terminal: state::COMPLETED,
        },
        Row {
            case: "a tagged EBS scope with item filters resolving to two recovery points",
            region: "eu-west-1",
            body: json!({
                "Name": "phases-ebs",
                "Tags": {"env": "test"},
                "SearchScope": {"BackupResourceTypes": ["EBS"]},
                "ItemFilters": {
                    "EBSItemFilters": [{
                        "FilePaths": [{"Value": "/var", "Operator": "BEGINS_WITH"}]
                    }]
                }
            }),
            held: vec![ebs_point("one"), ebs_point("two")],
            terminal: state::COMPLETED,
        },
        Row {
            case: "an S3 scope whose ARN filter excludes every held recovery point",
            region: "ap-southeast-2",
            body: json!({
                "ClientToken": "phases-token",
                "SearchScope": {
                    "BackupResourceTypes": ["S3"],
                    "BackupResourceArns": [
                        "arn:aws:backup:us-west-2:000000000000:recovery-point:rp-absent"
                    ]
                }
            }),
            held: vec![s3_point("one"), s3_point("two")],
            terminal: state::FAILED,
        },
    ]
}

#[tokio::test]
async fn a_search_job_walks_creating_then_running_then_its_terminal_status() {
    // `CREATING` is deliberately NOT one of the values the model's `SearchJobState` enum
    // declares (`smithy_query(query="shape", shape="SearchJobState")`, mirrored in
    // `types::SEARCH_JOB_STATE_VALUES`): restJson1 enums are open on the wire and the
    // service reports this status for a job it has accepted but not started scanning. The
    // pin is here so the walk below is read as the service's phases and not as the enum.
    assert!(
        !types::SEARCH_JOB_STATE_VALUES.contains(&state::CREATING),
        "the walk's first status is the unmodelled one the service reports, got {:?}",
        types::SEARCH_JOB_STATE_VALUES
    );

    // Every row is started BEFORE the first sample, so one sleep per PHASE carries the
    // whole table through that phase instead of one sleep per row.
    let mut started: Vec<(Row, MultiServiceRouter, String)> = Vec::new();
    for row in rows() {
        let (emulator, engine) = emulator_with_engine();
        seed_recovery_points(&engine, row.held.clone());
        let answer = call(
            &emulator,
            row.region,
            "PUT",
            "/search-jobs",
            Some(row.body.clone()),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: StartSearchJob must accept the request, got {} ({:?})",
            row.case, answer.status, answer.body
        );
        let identifier = answer.string("SearchJobIdentifier");
        assert!(
            !identifier.is_empty(),
            "case {:?}: StartSearchJob must report the minted identifier, got {:?}",
            row.case,
            answer.body
        );
        started.push((row, emulator, identifier));
    }

    // The phases, in the order every row must report them: how long to wait before
    // sampling (cumulative, so each sleep carries the table from one phase into the
    // next), and the status of that phase (`None` = the row's own terminal status).
    let phases: [(&str, std::time::Duration, Option<&str>); 3] = [
        ("created", WITHIN_THE_DUE_INSTANT, Some(state::CREATING)),
        ("scanning", PAST_THE_CREATING_INSTANT, Some(state::RUNNING)),
        ("settled", PAST_THE_DUE_INSTANT, None),
    ];

    for (phase, wait, phase_status) in phases {
        tokio::time::sleep(wait).await;
        for (row, emulator, identifier) in &started {
            let case = format!("{:?} in phase {phase:?}", row.case);
            let expected = phase_status.unwrap_or(row.terminal);
            let terminal = state::TERMINAL_STATES.contains(&expected);
            let job_uri = format!("/search-jobs/{identifier}");

            let fetched = call(emulator, row.region, "GET", &job_uri, None).await;
            assert_eq!(
                fetched.status, 200,
                "case {case}: expected 200 from GetSearchJob, got {} ({:?})",
                fetched.status, fetched.body
            );
            assert_eq!(
                fetched.string("Status"),
                expected,
                "case {case}: GetSearchJob must report {expected}, got {:?}",
                fetched.get("Status")
            );
            assert_eq!(
                fetched.get("CompletionTime").is_null(),
                !terminal,
                "case {case}: only a job in a terminal status has completed, got {:?}",
                fetched.get("CompletionTime")
            );

            // The listing reports the same record, so a client polling either one sees
            // one phase.
            let listed = call(emulator, row.region, "GET", "/search-jobs", None).await;
            let summary = listed.get("SearchJobs")[0].clone();
            assert_eq!(
                summary["Status"].as_str(),
                Some(expected),
                "case {case}: ListSearchJobs must report the same status, got {summary:?}"
            );

            // A read does not advance a phase: the same phase read twice answers
            // identically, and the completion time it reports is the instant the
            // transition was DUE, not the instant of the call.
            let again = call(emulator, row.region, "GET", &job_uri, None).await;
            assert_eq!(
                again.string("Status"),
                expected,
                "case {case}: reading a job must not move its status on, got {:?}",
                again.get("Status")
            );
            assert_eq!(
                again.get("CompletionTime").as_f64(),
                fetched.get("CompletionTime").as_f64(),
                "case {case}: reading a job must not change what a later reader sees, got {:?}",
                again.get("CompletionTime")
            );

            // The export gate judges the job against the very phase the readers report:
            // a job whose results have not settled is refused with its CURRENT status
            // named, and a job in an exportable terminal status is accepted.
            let export = call(
                emulator,
                row.region,
                "PUT",
                "/export-search-jobs",
                Some(json!({
                    "SearchJobIdentifier": identifier,
                    "ExportSpecification": {
                        "s3ExportSpecification": {"DestinationBucket": "phases-bucket"}
                    }
                })),
            )
            .await;
            if state::EXPORTABLE_SEARCH_JOB_STATES.contains(&expected) {
                assert_eq!(
                    export.status, 200,
                    "case {case}: an export of a settled search job must be accepted, got {} ({:?})",
                    export.status, export.body
                );
            } else {
                assert_eq!(
                    export.status, 400,
                    "case {case}: an export of a job in {expected} must be refused, got {} ({:?})",
                    export.status, export.body
                );
                let message = export.string("message");
                assert!(
                    message.contains(expected),
                    "case {case}: the refusal must name the phase the job is in ({expected}), got {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_stop_replaces_whatever_the_scan_still_owed() {
    // A stop of a running scan, with rows differing in the terminal status the scan was
    // still owed: the job winds down to STOPPED and never settles to the status its scan
    // would have reached, so the owed statuses the stop replaced cannot resurface. (A
    // stop issued before the scan is running is outside the status the operation acts
    // from and is refused instead; that is asserted in
    // `stop_search_job_status_gate.rs`.)
    let stop_cases = [
        ("stopped while scanning a scope that resolved to one recovery point", vec![s3_point("stopped")]),
        ("stopped while scanning a scope that resolved to nothing", Vec::new()),
    ];
    for (case, held) in stop_cases {
        let (emulator, engine) = emulator_with_engine();
        // Whatever the scan was owed — COMPLETED for a scope that resolved to something,
        // FAILED for one that resolved to nothing — reaching STOPPED shows the stop
        // replaced it.
        seed_recovery_points(&engine, held);
        let started = call(
            &emulator,
            REGION,
            "PUT",
            "/search-jobs",
            Some(json!({"Name": "to-stop", "SearchScope": {"BackupResourceTypes": ["S3"]}})),
        )
        .await;
        let identifier = started.string("SearchJobIdentifier");
        let job_uri = format!("/search-jobs/{identifier}");

        tokio::time::sleep(PAST_THE_CREATING_INSTANT).await;
        let stopped = call(
            &emulator,
            REGION,
            "PUT",
            &format!("{job_uri}/actions/cancel"),
            None,
        )
        .await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: StopSearchJob on a running scan must succeed, got {} ({:?})",
            stopped.status, stopped.body
        );

        let stopping = call(&emulator, REGION, "GET", &job_uri, None).await;
        assert_eq!(
            stopping.string("Status"),
            state::STOPPING,
            "case {case:?}: a job whose stop is in flight is STOPPING, got {:?}",
            stopping.get("Status")
        );

        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
        let settled = call(&emulator, REGION, "GET", &job_uri, None).await;
        assert_eq!(
            settled.string("Status"),
            state::STOPPED,
            "case {case:?}: a stopped job settles to STOPPED and not to the status its scan would have reached, got {:?}",
            settled.get("Status")
        );
        assert!(
            settled.get("CompletionTime").as_f64().is_some(),
            "case {case:?}: a stopped job carries a CompletionTime, got {:?}",
            settled.get("CompletionTime")
        );
    }
}

#[tokio::test]
async fn a_job_nobody_polled_reports_the_phase_the_clock_reached() {
    // Every phase the clock made due while nobody was looking is materialised at once:
    // the first read after both spans reports the terminal status, not the RUNNING the
    // job would have been in one phase earlier.
    let (emulator, engine) = emulator_with_engine();
    seed_recovery_points(&engine, vec![s3_point("unpolled"), s3_point("unpolled-2")]);
    let started = call(
        &emulator,
        REGION,
        "PUT",
        "/search-jobs",
        Some(json!({"SearchScope": {"BackupResourceTypes": ["S3"]}})),
    )
    .await;
    let identifier = started.string("SearchJobIdentifier");

    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    let settled = call(
        &emulator,
        REGION,
        "GET",
        &format!("/search-jobs/{identifier}"),
        None,
    )
    .await;
    assert_eq!(
        settled.string("Status"),
        state::COMPLETED,
        "the first read after both spans reports the terminal status, got {:?}",
        settled.get("Status")
    );
    // A completed scan got through its whole scope, which is what the record says it had
    // to scan — so no phase was consumed by the catch-up.
    assert_eq!(
        settled.get("CurrentSearchProgress")["RecoveryPointsScannedCount"].as_i64(),
        settled.get("SearchScopeSummary")["TotalRecoveryPointsToScanCount"].as_i64(),
        "a completed scan has scanned everything in scope, got {:?}",
        settled.get("CurrentSearchProgress")
    );
}
