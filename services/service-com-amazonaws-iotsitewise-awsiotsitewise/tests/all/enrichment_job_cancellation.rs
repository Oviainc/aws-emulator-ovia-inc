// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CancelEnrichmentJob`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin, over a SET of jobs rather than one: a cancel RECORDS
//! the cancellation on a job that has not reached a terminal status and answers the
//! terminal status it took that job to, which every later reader of the job then
//! reports; a repeat cancel is a no-op answering the status the job stands in,
//! because the operation is modelled idempotent; a job already in a terminal status
//! its own lifecycle earned cannot be cancelled and is the modelled conflict; and a
//! workspace or a job this service does not hold is the modelled not-found refusal.
//!
//! The HTTP harness (`call`, `error_code`, `is_modelled_id`, `router_with_engine`)
//! is the asset suite's and the create / workspace / dataset wrappers are the
//! enrichment-job suite's; this suite reuses both rather than growing a third copy.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{
    EnrichmentJobConfiguration, EnrichmentTrimSettings, EventDetection, TimeInNanos,
};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    enrichment_job_key, service_state, EnrichmentJobRecord, ENRICHMENT_JOB_STEP,
};

use crate::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine, REGION};
use crate::enrichment_jobs::{
    create, create_dataset, create_workspace, instant, jobs_uri, settled, CAMERA_ALIAS, DOOR_ALIAS,
    LONG_SERIES, SHORT_SERIES,
};
use crate::hierarchies_etags_and_refusals::error_message;

/// The workspace the jobs these suites cancel are created in.
const WORKSPACE: &str = "cancel-enrichment-floor";

/// A second HELD workspace, so a job addressed through the wrong workspace is not
/// also a request about a workspace this service does not hold.
const OTHER_WORKSPACE: &str = "cancel-enrichment-annex";

/// A workspace name no `CreateWorkspace` was called with.
const UNHELD_WORKSPACE: &str = "cancel-enrichment-nowhere";

/// An `ID`-shaped `jobId` no `CreateEnrichmentJob` minted.
const UNKNOWN_JOB: &str = "7a6b5c4d-3e2f-4a1b-8c9d-0e1f2a3b4c5d";

/// `POST /workspaces/{workspaceName}/enrichment-jobs/{jobId}/cancel`, as
/// `OPERATIONS.md` reports this operation's route.
fn cancel_uri(workspace: &str, job_id: &str) -> String {
    format!("{}/{job_id}/cancel", jobs_uri(workspace))
}

/// `CancelEnrichmentJob` on a job of `workspace`.
async fn cancel(router: &MultiServiceRouter, workspace: &str, job_id: &str) -> (u16, Value) {
    call(router, "POST", &cancel_uri(workspace, job_id), None).await
}

/// `CancelEnrichmentJob` in [`WORKSPACE`], asserting it answered and named the job
/// it was asked about, and the `status` it reported.
async fn cancelled(router: &MultiServiceRouter, job_id: &str) -> String {
    let (status, body) = cancel(router, WORKSPACE, job_id).await;
    assert_eq!(
        status, 200,
        "cancel {job_id}: expected 200, got {status} {body}"
    );
    assert_eq!(
        body["jobId"].as_str(),
        Some(job_id),
        "cancel {job_id}: the response names the job it cancelled, got {body}"
    );
    body["status"]
        .as_str()
        .unwrap_or_else(|| panic!("cancel {job_id}: no status in {body}"))
        .to_string()
}

/// `DescribeEnrichmentJob`, asserting it answered, and the body it answered with.
async fn described(router: &MultiServiceRouter, workspace: &str, job_id: &str) -> Value {
    let uri = format!("{}/{job_id}", jobs_uri(workspace));
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "describe {workspace}/{job_id}: expected 200, got {status} {body}"
    );
    body
}

/// The instant a cancel was recorded on a stored job, as the store holds it — the
/// only way to observe that a request recorded nothing, or recorded no second
/// cancellation.
fn cancel_recorded_at(
    engine: &Arc<engine::InMemoryEngine>,
    workspace: &str,
    job_id: &str,
) -> Option<SystemTime> {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let store = state.lock();
    store
        .enrichment_jobs
        .get(&enrichment_job_key(workspace, job_id))
        .unwrap_or_else(|| panic!("the store holds {job_id}"))
        .cancelled_at
}

/// An `eventDetection` configuration over `dataset_id`, identifying its time series
/// by `target` (the member and its value, or neither member) and trimmed to the
/// window `(startSeconds, startNanos, endSeconds, endNanos)`.
fn event_detection(
    dataset_id: &str,
    target: Option<(&str, &str)>,
    trim: (i64, i32, i64, i32),
) -> Value {
    let mut detection = json!({
        "datasetId": dataset_id,
        "trimSettings": {
            "startTime": instant(trim.0, trim.1),
            "endTime": instant(trim.2, trim.3),
        },
    });
    if let Some((member, value)) = target {
        detection
            .as_object_mut()
            .expect("an object configuration")
            .insert(member.to_string(), json!(value));
    }
    json!({ "eventDetection": detection })
}

/// One row of the cancel table: a label and the configuration the create carries.
struct CancelCase {
    label: &'static str,
    configuration: Value,
}

/// The rows differ along what the rule generalises over — which identifier the
/// configuration names for the time series (the shortest and the longest
/// `timeSeriesId` the modelled length admits, two different `propertyAlias`es and
/// neither), which dataset the job analyses, and how long the trim window is and
/// whether its bounds carry a sub-second offset. No two rows name the same dataset,
/// time series and job type, so none of them is the duplicate a create refuses.
fn cancel_cases(datasets: &[String]) -> Vec<CancelCase> {
    vec![
        CancelCase {
            label: "the shortest timeSeriesId",
            configuration: event_detection(
                &datasets[0],
                Some(("timeSeriesId", SHORT_SERIES)),
                (1_700_000_000, 0, 1_700_003_600, 0),
            ),
        },
        CancelCase {
            label: "the longest timeSeriesId, over a sub-second window",
            configuration: event_detection(
                &datasets[0],
                Some(("timeSeriesId", LONG_SERIES)),
                (1_700_000_000, 250_000_000, 1_700_000_000, 750_000_000),
            ),
        },
        CancelCase {
            label: "a propertyAlias",
            configuration: event_detection(
                &datasets[0],
                Some(("propertyAlias", CAMERA_ALIAS)),
                (1_700_000_000, 0, 1_700_000_060, 500_000_000),
            ),
        },
        CancelCase {
            label: "a second propertyAlias on another dataset",
            configuration: event_detection(
                &datasets[1],
                Some(("propertyAlias", DOOR_ALIAS)),
                (1_699_999_999, 0, 1_700_000_000, 1),
            ),
        },
        CancelCase {
            label: "the dataset alone",
            configuration: event_detection(&datasets[1], None, (1_700_000_000, 0, 1_700_007_200, 0)),
        },
    ]
}

/// The workspaces and the two datasets every suite below works from.
async fn fixture() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Vec<String>) {
    let (router, engine) = router_with_engine();
    for name in [WORKSPACE, OTHER_WORKSPACE] {
        create_workspace(&router, name).await;
    }
    let datasets = vec![
        create_dataset(&router, "Cancel camera footage").await,
        create_dataset(&router, "Cancel door footage").await,
    ];
    (router, engine, datasets)
}

/// Start the job of `case` in [`WORKSPACE`] and answer the `jobId` it was minted as.
async fn start(router: &MultiServiceRouter, configuration: &Value, label: &str) -> String {
    let (status, body) = create(
        router,
        WORKSPACE,
        json!({ "jobConfiguration": configuration }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: create expected 200, got {status} {body}"
    );
    let job_id = body["jobId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_modelled_id(&job_id),
        "case {label:?}: a create mints a modelled jobId, got {body}"
    );
    job_id
}

/// Put one job straight into the store, so the suite can address a job standing in
/// EVERY status the lifecycle admits — including the terminal ones a test cannot
/// wait a job into inside one step of its lifecycle.
#[allow(clippy::too_many_arguments)]
fn seed_job(
    engine: &Arc<engine::InMemoryEngine>,
    workspace: &str,
    job_id: &str,
    dataset_id: &str,
    alias: &str,
    created_at: SystemTime,
    cancelled_at: Option<SystemTime>,
) {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    store.enrichment_jobs.insert(
        enrichment_job_key(workspace, job_id),
        EnrichmentJobRecord {
            job_id: job_id.to_string(),
            workspace_name: workspace.to_string(),
            configuration: EnrichmentJobConfiguration::eventDetection(EventDetection {
                dataset_id: dataset_id.to_string(),
                time_series_id: None,
                property_alias: Some(alias.to_string()),
                trim_settings: EnrichmentTrimSettings {
                    start_time: TimeInNanos {
                        time_in_seconds: 1_700_000_000,
                        offset_in_nanos: Some(0),
                    },
                    end_time: TimeInNanos {
                        time_in_seconds: 1_700_000_600,
                        offset_in_nanos: Some(0),
                    },
                },
            }),
            client_token: None,
            created_at,
            cancelled_at,
        },
    );
}

/// Assert `answer` reports a cancelled job: the terminal status, the instant the
/// cancellation was recorded as both `cancelledAt` and the last status change, and
/// neither the completion nor the failure a cancelled job never reached.
fn assert_reports_cancelled(label: &str, answer: &Value) -> f64 {
    assert_eq!(
        answer["status"].as_str(),
        Some("CANCELLED"),
        "case {label:?}: a cancelled job reports CANCELLED, got {answer}"
    );
    let cancelled_at = answer["cancelledAt"].as_f64().unwrap_or(-1.0);
    let created_at = answer["createdAt"].as_f64().unwrap_or(-1.0);
    assert!(
        cancelled_at >= created_at && created_at > 0.0,
        "case {label:?}: cancelledAt {cancelled_at} must not precede createdAt \
         {created_at}, got {answer}"
    );
    assert_eq!(
        answer["updatedAt"].as_f64().unwrap_or(-1.0),
        cancelled_at,
        "case {label:?}: the last status change of a cancelled job is its cancellation, \
         got {answer}"
    );
    assert!(
        answer.get("completedAt").is_none(),
        "case {label:?}: a cancelled job never completed, got {answer}"
    );
    assert!(
        answer.get("failureMessage").is_none(),
        "case {label:?}: a cancelled job did not fail, got {answer}"
    );
    cancelled_at
}

#[tokio::test]
async fn a_cancel_records_the_cancellation_and_every_reader_reports_it() {
    let (router, engine, datasets) = fixture().await;
    let cases = cancel_cases(&datasets);

    let mut cancelled_jobs: Vec<(&str, String, f64)> = Vec::new();
    for case in &cases {
        let label = case.label;
        let job_id = start(&router, &case.configuration, label).await;

        let answered = cancelled(&router, &job_id).await;
        assert_eq!(
            answered, "CANCELLED",
            "case {label:?}: a recorded cancellation answers the status it took the job to, \
             got {answered}"
        );
        let recorded = cancel_recorded_at(&engine, WORKSPACE, &job_id);
        assert!(
            recorded.is_some(),
            "case {label:?}: the cancellation is recorded on the stored job"
        );

        // Every reader of the job agrees with the cancel's own answer, and the
        // configuration the create submitted is untouched by it.
        let read = described(&router, WORKSPACE, &job_id).await;
        let cancelled_at = assert_reports_cancelled(label, &read);
        assert_eq!(
            read["jobConfiguration"], case.configuration,
            "case {label:?}: a cancel leaves the configuration the create submitted, got {read}"
        );

        // Idempotent: a repeat cancel answers the status the job stands in, records
        // no second cancellation and changes nothing a reader can see.
        let again = cancelled(&router, &job_id).await;
        assert_eq!(
            again, "CANCELLED",
            "case {label:?}: a repeat cancel reports the current status, got {again}"
        );
        assert_eq!(
            cancel_recorded_at(&engine, WORKSPACE, &job_id),
            recorded,
            "case {label:?}: a repeat cancel records no second cancellation"
        );
        assert_eq!(
            described(&router, WORKSPACE, &job_id).await,
            read,
            "case {label:?}: a repeat cancel changes nothing about the job"
        );

        cancelled_jobs.push((label, job_id, cancelled_at));
    }

    // CANCELLED is TERMINAL: the span the clock would otherwise have taken each job
    // through to its completion elapses and every one of them still reports the
    // cancellation, at the instant it was recorded.
    tokio::time::sleep(settled()).await;
    for (label, job_id, cancelled_at) in &cancelled_jobs {
        let read = described(&router, WORKSPACE, job_id).await;
        assert_eq!(
            assert_reports_cancelled(label, &read),
            *cancelled_at,
            "case {label:?}: the cancellation instant does not move, got {read}"
        );
    }

    // The listing is a reader too: filtering on the status selects exactly the jobs
    // the cancels took there, and each summary reports that status.
    let uri = format!("{}?status=CANCELLED&maxResults=250", jobs_uri(WORKSPACE));
    let (status, body) = call(&router, "GET", &uri, None).await;
    assert_eq!(status, 200, "the listing answers, got {status} {body}");
    let mut listed: Vec<String> = body["jobs"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a jobs list, got {body}"))
        .iter()
        .map(|summary| {
            assert_eq!(
                summary["status"].as_str(),
                Some("CANCELLED"),
                "a summary of a cancelled job reports CANCELLED, got {summary}"
            );
            summary["jobId"].as_str().unwrap_or_default().to_string()
        })
        .collect();
    listed.sort();
    let mut expected: Vec<String> = cancelled_jobs
        .iter()
        .map(|(_, job_id, _)| job_id.clone())
        .collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "the listing reports every cancelled job and no other, got {body}"
    );
}

/// One row of the status table: the status the stored job stands in and what a
/// cancel of a job in that status must answer.
#[derive(Debug)]
struct StatusCase {
    /// The case name.
    label: &'static str,
    /// The `ID`-shaped `jobId` the row is seeded under.
    job_id: &'static str,
    /// How long ago the job was created.
    created_ago: Duration,
    /// How long ago a cancel was recorded on it, if one was.
    cancel_ago: Option<Duration>,
    /// The status a read of it reports before the cancel.
    status: &'static str,
    /// The status a cancel answers with, or `None` when the cancel is refused.
    cancelled_to: Option<&'static str>,
}

#[tokio::test]
async fn a_cancel_is_accepted_or_refused_by_the_status_the_job_stands_in() {
    let (router, engine, datasets) = fixture().await;

    // Rows span every status this service's enrichment-job lifecycle reaches: the
    // two a job is still going through, the terminal one its own lifecycle earns,
    // and the terminal one a cancel itself leaves behind. Source of the catalogue:
    // `smithy_query(query="shape", shape="EnrichmentJobStatus")` — CANCELLED |
    // COMPLETED | FAILED | PENDING | RUNNING | TIMED_OUT; FAILED and TIMED_OUT are
    // statuses nothing in this emulator takes a job to, so no row can stand in one.
    let cases: Vec<StatusCase> = vec![
        StatusCase {
            label: "pending",
            job_id: "dddddddd-0000-4000-8000-000000000001",
            created_ago: Duration::ZERO,
            cancel_ago: None,
            status: "PENDING",
            cancelled_to: Some("CANCELLED"),
        },
        StatusCase {
            label: "running",
            job_id: "dddddddd-0000-4000-8000-000000000002",
            created_ago: ENRICHMENT_JOB_STEP,
            cancel_ago: None,
            status: "RUNNING",
            cancelled_to: Some("CANCELLED"),
        },
        StatusCase {
            label: "completed",
            job_id: "dddddddd-0000-4000-8000-000000000003",
            created_ago: 3 * ENRICHMENT_JOB_STEP,
            cancel_ago: None,
            status: "COMPLETED",
            cancelled_to: None,
        },
        StatusCase {
            label: "already cancelled",
            job_id: "dddddddd-0000-4000-8000-000000000004",
            created_ago: 3 * ENRICHMENT_JOB_STEP,
            cancel_ago: Some(ENRICHMENT_JOB_STEP),
            status: "CANCELLED",
            cancelled_to: Some("CANCELLED"),
        },
    ];

    for case in &cases {
        let label = case.label;
        let now = SystemTime::now();
        let recorded_at = case.cancel_ago.map(|ago| now - ago);
        // An alias of its own per row, so no row is the duplicate of another.
        seed_job(
            &engine,
            WORKSPACE,
            case.job_id,
            &datasets[0],
            &format!("/company/windfarm/cancel/{label}"),
            now - case.created_ago,
            recorded_at,
        );

        let before = described(&router, WORKSPACE, case.job_id).await;
        assert_eq!(
            before["status"].as_str(),
            Some(case.status),
            "case {label:?}: the row stands in {}, got {before}",
            case.status
        );

        let (status, body) = cancel(&router, WORKSPACE, case.job_id).await;
        match case.cancelled_to {
            Some(expected) => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: a cancellable job is accepted, got {status} {body}"
                );
                assert_eq!(
                    body["status"].as_str(),
                    Some(expected),
                    "case {label:?}: expected {expected}, got {body}"
                );
                assert_eq!(
                    body["jobId"].as_str(),
                    Some(case.job_id),
                    "case {label:?}: the response names the job it addressed, got {body}"
                );
                match recorded_at {
                    // A job a cancel was already recorded on is not cancelled a
                    // second time: the instant it carries is exactly the one it
                    // already had, so a retry cannot move the cancellation on.
                    Some(_) => assert_eq!(
                        cancel_recorded_at(&engine, WORKSPACE, case.job_id),
                        recorded_at,
                        "case {label:?}: a repeat cancel records no second cancellation"
                    ),
                    None => assert!(
                        cancel_recorded_at(&engine, WORKSPACE, case.job_id).is_some(),
                        "case {label:?}: the cancellation is recorded on the stored job"
                    ),
                }
            }
            None => {
                // A terminal status its own lifecycle earned: the cancel cannot
                // apply, which this operation answers as the modelled conflict
                // rather than a success that cancelled nothing.
                assert_eq!(
                    status, 409,
                    "case {label:?}: a terminal job cannot be cancelled, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ConflictingOperationException",
                    "case {label:?}: unexpected error code in {body}"
                );
                assert_eq!(
                    error_message(&body),
                    format!(
                        "The enrichment job {} is in the {} status; only a job that has not \
                         reached a terminal status can be cancelled.",
                        case.job_id, case.status
                    ),
                    "case {label:?}: unexpected message in {body}"
                );
                assert_eq!(
                    body["resourceId"].as_str(),
                    Some(case.job_id),
                    "case {label:?}: the conflict names the job that conflicts, got {body}"
                );
                assert_eq!(
                    body["resourceArn"].as_str(),
                    Some(
                        format!(
                            "arn:aws:iotsitewise:{REGION}:{}:enrichment-job/{}",
                            engine::EMULATOR_ACCOUNT_ID, case.job_id
                        )
                        .as_str()
                    ),
                    "case {label:?}: the conflict names the job's ARN, got {body}"
                );
                assert_eq!(
                    cancel_recorded_at(&engine, WORKSPACE, case.job_id),
                    None,
                    "case {label:?}: a refused cancel records nothing"
                );
            }
        }

        // Whatever the answer was, a read of the job agrees with it.
        let after = described(&router, WORKSPACE, case.job_id).await;
        let expected = case.cancelled_to.unwrap_or(case.status);
        assert_eq!(
            after["status"].as_str(),
            Some(expected),
            "case {label:?}: a read after the cancel reports {expected}, got {after}"
        );
    }
}

#[tokio::test]
async fn a_cancel_reaches_a_job_only_inside_the_workspace_it_was_created_in() {
    let (router, engine, datasets) = fixture().await;
    let cases = cancel_cases(&datasets);
    let job_id = start(&router, &cases[0].configuration, cases[0].label).await;

    // Rows span the ways a cancel addresses no job of this service: a workspace it
    // does not hold, a name differing from a held one only in case (a workspace is
    // named exactly), a held workspace the job was NOT created in, and an
    // `ID`-shaped id no create minted.
    for (case, workspace, addressed, expected) in [
        (
            "a workspace this service does not hold",
            UNHELD_WORKSPACE,
            job_id.as_str(),
            format!("Workspace with ID {UNHELD_WORKSPACE} not found"),
        ),
        (
            "a workspace whose name differs only in case",
            "Cancel-Enrichment-Floor",
            job_id.as_str(),
            "Workspace with ID Cancel-Enrichment-Floor not found".to_string(),
        ),
        (
            "another held workspace",
            OTHER_WORKSPACE,
            job_id.as_str(),
            format!("Enrichment Job with ID {job_id} not found"),
        ),
        (
            "an id no create minted",
            WORKSPACE,
            UNKNOWN_JOB,
            format!("Enrichment Job with ID {UNKNOWN_JOB} not found"),
        ),
    ] {
        let (status, body) = cancel(&router, workspace, addressed).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            error_message(&body),
            expected.as_str(),
            "case {case:?}: unexpected message in {body}"
        );
    }

    // None of those refusals cancelled the job they named: it still stands in the
    // status the clock owes it, and nothing was recorded on it.
    let read = described(&router, WORKSPACE, &job_id).await;
    assert!(
        matches!(
            read["status"].as_str(),
            Some("PENDING") | Some("RUNNING") | Some("COMPLETED")
        ),
        "a refused cancel cancels nothing, got {read}"
    );
    assert!(
        read.get("cancelledAt").is_none(),
        "a refused cancel records no cancellation instant, got {read}"
    );
    assert_eq!(
        cancel_recorded_at(&engine, WORKSPACE, &job_id),
        None,
        "a refused cancel records nothing on the stored job"
    );
}

#[tokio::test]
async fn a_cancelled_job_is_no_longer_the_duplicate_a_second_create_is_refused_for() {
    let (router, _engine, datasets) = fixture().await;
    let cases = cancel_cases(&datasets);

    // Two rows, so the rule is not pinned on one shape of configuration: the work a
    // job does is its dataset, its time series and its job type, whichever member
    // named that series.
    for case in [&cases[0], &cases[2]] {
        let label = case.label;
        let first = start(&router, &case.configuration, label).await;

        // While that job is going, a second create for the same work is the modelled
        // conflict rather than a second job.
        let (status, body) = create(
            &router,
            WORKSPACE,
            json!({ "jobConfiguration": case.configuration }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {label:?}: a duplicate of a job still going is refused, got {status} {body}"
        );

        // Once the job is cancelled it is doing no work, so the same create starts a
        // job of its own.
        let answered = cancelled(&router, &first).await;
        assert_eq!(
            answered, "CANCELLED",
            "case {label:?}: the first job is cancelled, got {answered}"
        );
        let second = start(&router, &case.configuration, label).await;
        assert_ne!(
            second, first,
            "case {label:?}: a create after the cancel starts a job of its own"
        );
        assert_eq!(
            described(&router, WORKSPACE, &first).await["status"].as_str(),
            Some("CANCELLED"),
            "case {label:?}: the cancelled job stays cancelled"
        );
    }
}
