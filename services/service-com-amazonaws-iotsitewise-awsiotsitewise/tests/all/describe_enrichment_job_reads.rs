// SPDX-License-Identifier: CC0-1.0

//! `DescribeEnrichmentJob` as a REPORTING read over the jobs
//! `CreateEnrichmentJob` started, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a describe answers the job the store holds under the workspace and id the
//!   request addressed: its configuration verbatim as the create submitted it, the
//!   job type that configuration makes it, the instant the create stamped it with,
//!   and the status and lifecycle instants the CLOCK has made due — whatever shape
//!   that configuration took;
//! * the lifecycle members are reported only once they fall due: no `updatedAt`
//!   before the first transition, no `completedAt` before the terminal status, and
//!   neither `cancelledAt` nor `failureMessage` for a job nothing cancelled or
//!   failed;
//! * a describe writes NOTHING — the whole stored job map is identical before and
//!   after any number of reads, through every phase of the lifecycle, and the only
//!   members two polls may differ on are the ones the clock owes, which never move
//!   backwards;
//! * a job is addressed WITHIN its workspace: a workspace this service does not
//!   hold, a held workspace the job was not created in, and an id no create minted
//!   are each the modelled not-found refusal, in the wording every not-found path
//!   of this service uses.
//!
//! The HTTP harness (`call`, `error_code`, `is_modelled_id`,
//! `router_with_engine`) is the asset suite's and the `CreateEnrichmentJob` /
//! workspace / dataset wrappers are the enrichment-job suite's; this suite reuses
//! both rather than growing a third copy.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine};
use crate::enrichment_jobs::{
    create, create_dataset, create_workspace, instant, jobs_uri, settled, CAMERA_ALIAS, DOOR_ALIAS,
    LONG_SERIES, SHORT_SERIES, UNKNOWN_DATASET,
};

/// The workspace the jobs these suites read are created in.
const WORKSPACE: &str = "describe-enrichment-floor";

/// A second HELD workspace, so a job addressed through the wrong workspace is not
/// also a request about a workspace this service does not hold.
const OTHER_WORKSPACE: &str = "describe-enrichment-annex";

/// A workspace name no `CreateWorkspace` was called with.
const UNHELD_WORKSPACE: &str = "describe-enrichment-nowhere";

/// An `ID`-shaped `jobId` no `CreateEnrichmentJob` minted.
const UNKNOWN_JOB: &str = "9f8e7d6c-5b4a-4938-8271-605f4e3d2c1b";

/// The only `JobType` the model declares. Source: `smithy_query(query="shape",
/// shape="JobType")`.
const EVENT_DETECTION: &str = "EVENT_DETECTION";

/// How far along the lifecycle a reported status is, so a sequence of polls can be
/// checked for never going backwards. Source: the `EnrichmentJobStatus` values the
/// model names (`smithy_query(query="shape", shape="EnrichmentJobStatus")`:
/// CANCELLED | COMPLETED | FAILED | PENDING | RUNNING | TIMED_OUT), in the order
/// the owed transitions take them. The other three are modelled but nothing in this
/// emulator reports them, so they are diagnosed rather than ranked.
fn rank(status: &str) -> usize {
    match status {
        "PENDING" => 0,
        "RUNNING" => 1,
        "COMPLETED" => 2,
        other => panic!(
            "DescribeEnrichmentJob reported a status this emulator never reports: {other:?}"
        ),
    }
}

/// `GET /workspaces/{workspaceName}/enrichment-jobs/{jobId}`, as `OPERATIONS.md`
/// reports this operation's route.
async fn describe_raw(
    router: &MultiServiceRouter,
    workspace: &str,
    job_id: &str,
) -> (u16, Value) {
    let uri = format!("{}/{job_id}", jobs_uri(workspace));
    call(router, "GET", &uri, None).await
}

/// `DescribeEnrichmentJob`, asserting it answered, and the body it answered with.
async fn described(router: &MultiServiceRouter, workspace: &str, job_id: &str) -> Value {
    let (status, body) = describe_raw(router, workspace, job_id).await;
    assert_eq!(
        status, 200,
        "describe {workspace}/{job_id}: expected 200, got {status} {body}"
    );
    body
}

/// Every enrichment job the store holds, one line per entry and sorted, so two
/// snapshots compare as a WHOLE: what a read must leave untouched is not one member
/// of one record but the entire map.
///
/// Rendered through `Debug` rather than compared field by field so a record that
/// grows a member is covered without this suite being updated.
fn stored_jobs(shared: &Arc<engine::InMemoryEngine>) -> Vec<String> {
    let state = service_state(&**shared).expect("the IoT SiteWise store");
    let guard = state.lock();
    let mut rendered: Vec<String> = guard
        .enrichment_jobs
        .iter()
        .map(|(key, record)| format!("{key:?} => {record:?}"))
        .collect();
    drop(guard);
    rendered.sort();
    rendered
}

/// One row of the describe table: a label and the `eventDetection` configuration
/// the create carries, over the dataset at `dataset` of the fixture's two.
struct DescribeCase {
    label: &'static str,
    dataset: usize,
    configuration: Value,
}

/// The rows differ along what the rule generalises over — which identifier the
/// configuration names for the time series (the shortest and the longest
/// `timeSeriesId` the modelled length admits, two different `propertyAlias`es, and
/// neither), which dataset the job analyses, and how long the trim window is and
/// whether its bounds carry a sub-second offset. No two rows name the same dataset,
/// time series and job type, so none of them is the duplicate a create refuses.
fn describe_cases(datasets: &[String]) -> Vec<DescribeCase> {
    vec![
        DescribeCase {
            label: "the shortest timeSeriesId",
            dataset: 0,
            configuration: event_detection(
                &datasets[0],
                Some(("timeSeriesId", SHORT_SERIES)),
                (1_700_000_000, 0, 1_700_003_600, 0),
            ),
        },
        DescribeCase {
            label: "the longest timeSeriesId, over a sub-second window",
            dataset: 0,
            configuration: event_detection(
                &datasets[0],
                Some(("timeSeriesId", LONG_SERIES)),
                (1_700_000_000, 250_000_000, 1_700_000_000, 750_000_000),
            ),
        },
        DescribeCase {
            label: "a propertyAlias",
            dataset: 0,
            configuration: event_detection(
                &datasets[0],
                Some(("propertyAlias", CAMERA_ALIAS)),
                (1_700_000_000, 0, 1_700_000_060, 500_000_000),
            ),
        },
        DescribeCase {
            label: "a second propertyAlias",
            dataset: 0,
            configuration: event_detection(
                &datasets[0],
                Some(("propertyAlias", DOOR_ALIAS)),
                (1_699_999_999, 0, 1_700_000_000, 1),
            ),
        },
        DescribeCase {
            label: "the dataset alone",
            dataset: 1,
            configuration: event_detection(
                &datasets[1],
                None,
                (1_700_000_000, 0, 1_700_007_200, 0),
            ),
        },
        DescribeCase {
            label: "a propertyAlias on another dataset",
            dataset: 1,
            configuration: event_detection(
                &datasets[1],
                Some(("propertyAlias", CAMERA_ALIAS)),
                (1_700_000_100, 125_000_000, 1_700_000_200, 875_000_000),
            ),
        },
    ]
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

/// The workspaces and the two datasets every suite below works from.
async fn fixture() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Vec<String>) {
    let (router, engine) = router_with_engine();
    for name in [WORKSPACE, OTHER_WORKSPACE] {
        create_workspace(&router, name).await;
    }
    let datasets = vec![
        create_dataset(&router, "Describe camera footage").await,
        create_dataset(&router, "Describe door footage").await,
    ];
    (router, engine, datasets)
}

/// Start the job of `case` and answer `(jobId, the createdAt its create reported)`.
async fn start(router: &MultiServiceRouter, case: &DescribeCase) -> (String, f64) {
    let label = case.label;
    let (status, body) = create(
        router,
        WORKSPACE,
        json!({ "jobConfiguration": case.configuration }),
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
    let created_at = body["createdAt"].as_f64().unwrap_or(-1.0);
    assert!(
        created_at > 0.0,
        "case {label:?}: createdAt is the instant the job was created, got {body}"
    );
    (job_id, created_at)
}

/// Assert the lifecycle members `answer` reports agree with the status it reports:
/// each is present exactly once the transition it records has fallen due.
fn assert_lifecycle_is_coherent(label: &str, answer: &Value) {
    let status = answer["status"].as_str().unwrap_or_default();
    let created_at = answer["createdAt"].as_f64().unwrap_or(-1.0);
    assert!(
        created_at > 0.0,
        "case {label:?}: createdAt is required, got {answer}"
    );
    match status {
        "PENDING" => {
            assert!(
                answer.get("updatedAt").is_none(),
                "case {label:?}: a job no transition has moved reports no updatedAt, got {answer}"
            );
            assert!(
                answer.get("completedAt").is_none(),
                "case {label:?}: a job that has not completed reports no completedAt, \
                 got {answer}"
            );
        }
        "RUNNING" => {
            let updated_at = answer["updatedAt"].as_f64().unwrap_or(-1.0);
            assert!(
                updated_at >= created_at,
                "case {label:?}: updatedAt {updated_at} precedes createdAt {created_at}, \
                 got {answer}"
            );
            assert!(
                answer.get("completedAt").is_none(),
                "case {label:?}: a running job reports no completedAt, got {answer}"
            );
        }
        "COMPLETED" => {
            let completed_at = answer["completedAt"].as_f64().unwrap_or(-1.0);
            let updated_at = answer["updatedAt"].as_f64().unwrap_or(-1.0);
            assert!(
                completed_at >= created_at,
                "case {label:?}: completedAt {completed_at} precedes createdAt {created_at}, \
                 got {answer}"
            );
            assert_eq!(
                updated_at, completed_at,
                "case {label:?}: the last status change of a completed job is its completion, \
                 got {answer}"
            );
        }
        other => panic!("case {label:?}: unexpected status {other:?} in {answer}"),
    }
    assert!(
        answer.get("cancelledAt").is_none(),
        "case {label:?}: a job nothing cancelled reports no cancelledAt, got {answer}"
    );
    assert!(
        answer.get("failureMessage").is_none(),
        "case {label:?}: a job that did not fail reports no failureMessage, got {answer}"
    );
}

#[tokio::test]
async fn a_created_job_is_described_from_the_configuration_it_was_created_from() {
    let (router, _engine, datasets) = fixture().await;
    let cases = describe_cases(&datasets);

    let mut started = Vec::new();
    for case in &cases {
        let label = case.label;
        let (job_id, created_at) = start(&router, case).await;

        let answer = described(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            answer["jobId"].as_str(),
            Some(job_id.as_str()),
            "case {label:?}: a read reports the job it was asked for, got {answer}"
        );
        assert_eq!(
            answer["workspaceName"].as_str(),
            Some(WORKSPACE),
            "case {label:?}: a read names the workspace the job was created in, got {answer}"
        );
        assert_eq!(
            answer["jobType"].as_str(),
            Some(EVENT_DETECTION),
            "case {label:?}: an eventDetection configuration makes an EVENT_DETECTION job, \
             got {answer}"
        );
        assert_eq!(
            answer["jobConfiguration"], case.configuration,
            "case {label:?}: a read reports the configuration the create submitted, got {answer}"
        );
        assert_eq!(
            answer["createdAt"].as_f64().unwrap_or(-1.0),
            created_at,
            "case {label:?}: a read reports the instant the create stamped the job with, \
             got {answer}"
        );
        assert_eq!(
            answer["jobConfiguration"]["eventDetection"]["datasetId"].as_str(),
            Some(datasets[case.dataset].as_str()),
            "case {label:?}: a read reports the dataset the job analyses, got {answer}"
        );
        assert_lifecycle_is_coherent(label, &answer);

        started.push((label, job_id, case.configuration.clone(), created_at));
    }

    tokio::time::sleep(settled()).await;

    // Every job the clock has finished reports its terminal status, and the
    // configuration and creation instant a read reports never change.
    for (label, job_id, configuration, created_at) in &started {
        let answer = described(&router, WORKSPACE, job_id).await;
        assert_eq!(
            answer["status"].as_str(),
            Some("COMPLETED"),
            "case {label:?}: a job the clock has finished is COMPLETED, got {answer}"
        );
        assert_eq!(
            answer["jobConfiguration"], *configuration,
            "case {label:?}: the configuration a read reports does not change, got {answer}"
        );
        assert_eq!(
            answer["createdAt"].as_f64().unwrap_or(-1.0),
            *created_at,
            "case {label:?}: the creation instant a read reports does not change, got {answer}"
        );
        assert_lifecycle_is_coherent(label, &answer);
    }
}

#[tokio::test]
async fn a_describe_reports_the_store_and_writes_nothing() {
    let (router, engine, datasets) = fixture().await;
    let cases = describe_cases(&datasets);

    let mut jobs = Vec::new();
    for case in &cases[..3] {
        let (job_id, _) = start(&router, case).await;
        jobs.push((case.label, job_id));
    }

    let before = stored_jobs(&engine);
    assert_eq!(
        before.len(),
        jobs.len(),
        "the store holds one record per created job, got {before:?}"
    );

    // Polled repeatedly through every phase the clock owes a job: while it is still
    // transient, around the first transition, and once it has settled.
    let mut polls: Vec<(&str, String, Vec<Value>)> = jobs
        .iter()
        .map(|(label, job_id)| (*label, job_id.clone(), Vec::new()))
        .collect();
    for wait in [
        Duration::ZERO,
        Duration::ZERO,
        Duration::from_millis(300),
        settled(),
        Duration::ZERO,
    ] {
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        for (_, job_id, answers) in &mut polls {
            let answer = described(&router, WORKSPACE, job_id).await;
            answers.push(answer);
        }
    }

    assert_eq!(
        stored_jobs(&engine),
        before,
        "a describe writes nothing: the stored jobs must be identical before and after \
         every read"
    );

    for (label, _, answers) in &polls {
        let first = answers.first().expect("at least one poll").clone();
        let mut highest = 0;
        for answer in answers {
            assert_lifecycle_is_coherent(label, answer);
            let status = answer["status"].as_str().unwrap_or_default();
            let reached = rank(status);
            assert!(
                reached >= highest,
                "case {label:?}: a status never moves backwards, got {answer}"
            );
            highest = reached;
            // Everything but the members the clock owes is the same on every poll.
            for member in ["jobId", "workspaceName", "jobType", "jobConfiguration", "createdAt"] {
                assert_eq!(
                    answer[member], first[member],
                    "case {label:?}: {member} must be identical on every poll, got {answer}"
                );
            }
        }
        assert_eq!(
            highest,
            rank("COMPLETED"),
            "case {label:?}: the last poll is taken once the job has settled, got {answers:?}"
        );
    }
}

#[tokio::test]
async fn a_job_is_addressed_within_the_workspace_it_was_created_in() {
    let (router, engine, datasets) = fixture().await;
    let cases = describe_cases(&datasets);
    let (job_id, _) = start(&router, &cases[0]).await;

    let before = stored_jobs(&engine);

    // (case, the workspace addressed, the id addressed, the message the refusal
    // carries). Rows: a held workspace the job was NOT created in, a workspace this
    // service does not hold, a workspace whose name differs only in case, an
    // `ID`-shaped id no create minted, and an id that identifies a dataset rather
    // than a job.
    for (case, workspace, requested, expected) in [
        (
            "another held workspace",
            OTHER_WORKSPACE,
            job_id.as_str(),
            format!("Enrichment Job with ID {job_id} not found"),
        ),
        (
            "a workspace this service does not hold",
            UNHELD_WORKSPACE,
            job_id.as_str(),
            format!("Workspace with ID {UNHELD_WORKSPACE} not found"),
        ),
        (
            "a workspace whose name differs only in case",
            "Describe-Enrichment-Floor",
            job_id.as_str(),
            "Workspace with ID Describe-Enrichment-Floor not found".to_string(),
        ),
        (
            "an id no create minted",
            WORKSPACE,
            UNKNOWN_JOB,
            format!("Enrichment Job with ID {UNKNOWN_JOB} not found"),
        ),
        (
            "an id that names no job of this service",
            WORKSPACE,
            UNKNOWN_DATASET,
            format!("Enrichment Job with ID {UNKNOWN_DATASET} not found"),
        ),
    ] {
        let (status, body) = describe_raw(&router, workspace, requested).await;
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
            body["message"].as_str().unwrap_or_default(),
            expected,
            "case {case:?}: unexpected refusal wording in {body}"
        );
    }

    // `jobId` carries the modelled length 36..36
    // (`smithy_query(query="operation", operation="DescribeEnrichmentJob")`), so an
    // id outside that length is a BAD REQUEST rather than an absent job. The bound
    // is checked on each side: one character short and one past it.
    for (case, requested) in [
        ("one character short of the modelled length", "9f8e7d6c-5b4a-4938-8271-605f4e3d2c1"),
        ("one character past the modelled length", "9f8e7d6c-5b4a-4938-8271-605f4e3d2c1bb"),
    ] {
        let (status, body) = describe_raw(&router, WORKSPACE, requested).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        let code = error_code(&body);
        assert!(
            matches!(code, "InvalidRequestException" | "ValidationException"),
            "case {case:?}: unexpected error code {code:?} in {body}"
        );
    }

    // The job the request did address is still readable, and no refusal wrote
    // anything.
    let answer = described(&router, WORKSPACE, &job_id).await;
    assert_eq!(
        answer["jobId"].as_str(),
        Some(job_id.as_str()),
        "the addressed job is still readable, got {answer}"
    );
    assert_eq!(
        stored_jobs(&engine),
        before,
        "a refused describe writes nothing"
    );
}
