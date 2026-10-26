// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise enrichment job slice
//! (`CreateEnrichmentJob` -> `ListEnrichmentJobs`), driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with, so every assertion is about
//! the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a created job is PERSISTED and is reported back by the listing exactly as it
//!   was created — its dataset, the identifier its configuration named for the time
//!   series, and the job type that configuration makes it — whatever shape that
//!   configuration took;
//! * a listing enumerates exactly the jobs of the workspace the request names and
//!   no job of another workspace; a held workspace with no job is an empty page,
//!   and a workspace this service does not hold is the modelled not-found refusal;
//! * every summary reports the status the CLOCK has made due for that job, and the
//!   instant of its last status change only once a transition has fallen due — so a
//!   listing never moves a job on and two listings of one store agree;
//! * each filter selects only the jobs it names, filters combine, and a value
//!   outside a modelled catalogue is refused rather than answered with an empty
//!   page;
//! * the date filter is the half-open window `(startDate, endDate]` — `startDate`
//!   exclusive, `endDate` inclusive;
//! * the listing is ordered by `createdAt` descending, and `maxResults`/`nextToken`
//!   page that one enumeration whatever page size the caller asks for;
//! * a create whose configuration breaks a rule its own shapes state starts NO job,
//!   one naming a workspace or a dataset the store does not hold is the modelled
//!   not-found refusal, a retry carrying the same `clientToken` replays the job the
//!   first call started, and a second create for work a job is still doing is the
//!   modelled conflict;
//! * a `clientToken` of any length the input models makes its create idempotent —
//!   a retry replays the job that token started, identical or not, and starts
//!   nothing — while a token outside that length is refused as client input and
//!   starts no job; and the identity, status and creation instant a create answers
//!   are the ones the job it persisted carries.
//!
//! The HTTP harness (`call`, `router`, `router_with_engine`, `error_code`,
//! `hex_token`, `is_modelled_id`) is the one the asset suite already has; this suite
//! reuses it rather than growing a second.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{
    EnrichmentJobConfiguration, EnrichmentTrimSettings, EventDetection, TimeInNanos,
};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    enrichment_job_key, service_state, EnrichmentJobRecord, ENRICHMENT_JOB_STEP,
    MAX_ENRICHMENT_JOBS_PER_WORKSPACE,
};

use super::asset_lifecycle::{
    call, error_code, hex_token, is_modelled_id, router, router_with_engine,
};

/// The workspace every suite below works in unless it is about workspace scoping.
const WORKSPACE: &str = "enrichment-floor";

/// A second workspace, so a listing can be shown to report only the jobs of the one
/// it names.
const OTHER_WORKSPACE: &str = "other-floor";

/// A third, held workspace nobody creates a job in: absence of jobs is not absence
/// of the workspace.
const QUIET_WORKSPACE: &str = "quiet-floor";

/// Every `EnrichmentJobStatus` a job can report. Source:
/// `smithy_query(query="shape", shape="EnrichmentJobStatus")` — the enum's own
/// value list.
const ENRICHMENT_JOB_STATUSES: [&str; 6] = [
    "PENDING",
    "RUNNING",
    "COMPLETED",
    "FAILED",
    "TIMED_OUT",
    "CANCELLED",
];

/// Every `JobType` the model declares. Source: `smithy_query(query="shape",
/// shape="JobType")` — EVENT_DETECTION is the only one.
const JOB_TYPES: [&str; 1] = ["EVENT_DETECTION"];

/// A `TimeSeriesId` of the shortest form the modelled length (36..73) admits: a
/// single id, which is how this service names a stream no asset property owns.
pub(crate) const SHORT_SERIES: &str = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";

/// A `TimeSeriesId` of the longest form that length admits, which this service
/// builds as `<assetId>.<propertyId>`.
pub(crate) const LONG_SERIES: &str =
    "11111111-2222-3333-4444-555555555555.66666666-7777-8888-9999-aaaaaaaaaaaa";

/// A `propertyAlias` of the human-readable form the member documents.
pub(crate) const CAMERA_ALIAS: &str = "/company/windfarm/3/turbine/7/camera";

/// A second alias, so a filter naming one cannot pass by reporting every job that
/// carries any alias.
pub(crate) const DOOR_ALIAS: &str = "/company/warehouse/zone-a/door-camera";

/// An alias no job of this suite was created with.
const UNUSED_ALIAS: &str = "/company/warehouse/zone-z/nothing";

/// A `datasetId`-shaped value no `CreateDataset` minted, for the rows about a
/// filter and a create naming a dataset the store does not hold.
pub(crate) const UNKNOWN_DATASET: &str = "0a1b2c3d-4e5f-4a7b-8c9d-0e1f2a3b4c5d";

/// How long after the whole span the clock owes a job a read is taken, so a run
/// whose scheduler is late cannot observe a transient status.
pub(crate) fn settled() -> Duration {
    2 * ENRICHMENT_JOB_STEP + Duration::from_millis(250)
}

/// `/workspaces/{workspaceName}/enrichment-jobs`, as `OPERATIONS.md` reports the
/// route of both `CreateEnrichmentJob` (POST) and `ListEnrichmentJobs` (GET).
pub(crate) fn jobs_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/enrichment-jobs")
}

/// A modelled `TimeInNanos` on the wire.
pub(crate) fn instant(seconds: i64, nanos: i32) -> Value {
    json!({ "timeInSeconds": seconds, "offsetInNanos": nanos })
}

/// The same, as the stored shape.
fn time_in_nanos(seconds: i64, nanos: i32) -> TimeInNanos {
    TimeInNanos {
        time_in_seconds: seconds,
        offset_in_nanos: Some(nanos),
    }
}

/// The RFC 3339 form a query-bound timestamp takes on the wire: `startDate` and
/// `endDate` carry no `@timestampFormat`, so an `@httpQuery` timestamp uses
/// Smithy's default `date-time` format.
fn query_date(seconds: i64) -> String {
    DateTime::from_secs(seconds)
        .fmt(Format::DateTime)
        .expect("a formattable timestamp")
}

/// Which identifier a job's configuration names for the time series it analyses.
/// `EventDetection` takes one of two, or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// Only the dataset identifies the data to analyse.
    Dataset,
    /// "Present when the job was created using a propertyAlias."
    Alias(&'static str),
    /// "Present when the job was created using a timeSeriesId."
    Series(&'static str),
}

/// One row of the create-and-list-back table.
#[derive(Debug, Clone, Copy)]
struct JobCase {
    label: &'static str,
    /// Which of the datasets the fixture created this job analyses.
    dataset: usize,
    target: Target,
    /// The `trimSettings` window, as `(startSeconds, startNanos, endSeconds, endNanos)`.
    trim: (i64, i32, i64, i32),
    client_token: Option<&'static str>,
}

/// The rows: configurations spanning each identifying form the shapes admit — a
/// `timeSeriesId` of the shortest and of the longest length `TimeSeriesId` allows,
/// two different `propertyAlias`es, and neither identifier — crossed with two
/// datasets, trim windows of different lengths and with sub-second offsets, and a
/// present / absent `clientToken`. No two rows name the same dataset, time series
/// and job type, so none of them is the duplicate the create refuses.
const JOB_CASES: &[JobCase] = &[
    JobCase {
        label: "by the shortest timeSeriesId",
        dataset: 0,
        target: Target::Series(SHORT_SERIES),
        trim: (1_700_000_000, 0, 1_700_003_600, 0),
        client_token: None,
    },
    JobCase {
        label: "by the longest timeSeriesId",
        dataset: 0,
        target: Target::Series(LONG_SERIES),
        trim: (1_700_000_000, 250_000_000, 1_700_000_001, 0),
        client_token: Some("11111111-1111-1111-1111-111111111111"),
    },
    JobCase {
        label: "by a propertyAlias",
        dataset: 0,
        target: Target::Alias(CAMERA_ALIAS),
        trim: (1_700_000_000, 0, 1_700_000_060, 500_000_000),
        client_token: None,
    },
    JobCase {
        label: "by a second propertyAlias",
        dataset: 0,
        target: Target::Alias(DOOR_ALIAS),
        trim: (1_699_999_999, 0, 1_700_000_000, 1),
        client_token: Some("22222222-2222-2222-2222-222222222222"),
    },
    JobCase {
        label: "by the dataset alone",
        dataset: 1,
        target: Target::Dataset,
        trim: (1_700_000_000, 0, 1_700_007_200, 0),
        client_token: None,
    },
    JobCase {
        label: "by a propertyAlias on another dataset",
        dataset: 1,
        target: Target::Alias(CAMERA_ALIAS),
        trim: (1_700_000_100, 125_000_000, 1_700_000_200, 875_000_000),
        client_token: None,
    },
];

/// The `eventDetection` configuration a row sends.
fn configuration(case: &JobCase, dataset_id: &str) -> Value {
    let mut detection = json!({
        "datasetId": dataset_id,
        "trimSettings": {
            "startTime": instant(case.trim.0, case.trim.1),
            "endTime": instant(case.trim.2, case.trim.3),
        },
    });
    let object = detection.as_object_mut().expect("an object configuration");
    match case.target {
        Target::Dataset => {}
        Target::Alias(alias) => {
            object.insert("propertyAlias".to_string(), json!(alias));
        }
        Target::Series(series) => {
            object.insert("timeSeriesId".to_string(), json!(series));
        }
    }
    json!({ "eventDetection": detection })
}

/// The body a create sends for `case`.
fn create_body(case: &JobCase, dataset_id: &str) -> Value {
    let mut body = json!({ "jobConfiguration": configuration(case, dataset_id) });
    if let Some(token) = case.client_token {
        body.as_object_mut()
            .expect("an object body")
            .insert("clientToken".to_string(), json!(token));
    }
    body
}

/// `CreateEnrichmentJob` with `body`.
pub(crate) async fn create(
    router: &MultiServiceRouter,
    workspace: &str,
    body: Value,
) -> (u16, Value) {
    call(router, "POST", &jobs_uri(workspace), Some(body)).await
}

/// Create `name` through `CreateWorkspace`, so a job created in it names a
/// workspace this service really holds.
pub(crate) async fn create_workspace(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "workspaceName": name,
            "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
        })),
    )
    .await;
    assert_eq!(status, 200, "create workspace {name}: got {status} {body}");
}

/// Create a dataset through `CreateDataset` and answer its id, so a configuration
/// names a dataset the store really holds.
pub(crate) async fn create_dataset(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/datasets",
        Some(json!({
            "datasetName": name,
            "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
        })),
    )
    .await;
    assert_eq!(status, 202, "create dataset {name}: got {status} {body}");
    body["datasetId"]
        .as_str()
        .unwrap_or_else(|| panic!("create dataset {name}: no datasetId in {body}"))
        .to_string()
}

/// `ListEnrichmentJobs` on `workspace` with the raw query string `query` (empty for
/// none).
async fn list_raw(router: &MultiServiceRouter, workspace: &str, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        jobs_uri(workspace)
    } else {
        format!("{}?{query}", jobs_uri(workspace))
    };
    call(router, "GET", &uri, None).await
}

/// `ListEnrichmentJobs`, asserting it answered and that the required `jobs` member
/// is a list, and the body it answered with.
async fn listed(router: &MultiServiceRouter, workspace: &str, query: &str) -> Value {
    let (status, body) = list_raw(router, workspace, query).await;
    assert_eq!(
        status, 200,
        "list {workspace} ?{query}: expected 200, got {status} {body}"
    );
    assert!(
        body["jobs"].is_array(),
        "list {workspace} ?{query}: jobs is a required list, got {body}"
    );
    body
}

/// Every job of `workspace` in one page: `maxResults` carries the modelled range
/// 1..250, so 250 is the whole listing whenever a suite holds fewer jobs than that.
async fn list_all(router: &MultiServiceRouter, workspace: &str, filters: &str) -> Value {
    let query = if filters.is_empty() {
        "maxResults=250".to_string()
    } else {
        format!("{filters}&maxResults=250")
    };
    listed(router, workspace, &query).await
}

/// The `jobId`s a listing body reports, in the order it reported them.
fn ids(body: &Value) -> Vec<String> {
    body["jobs"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a jobs list, got {body}"))
        .iter()
        .map(|summary| {
            summary["jobId"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary carries a jobId, got {summary}"))
                .to_string()
        })
        .collect()
}

/// The summary a listing reported for `job_id`.
fn summary_of(body: &Value, job_id: &str) -> Value {
    body["jobs"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a jobs list, got {body}"))
        .iter()
        .find(|summary| summary["jobId"].as_str() == Some(job_id))
        .unwrap_or_else(|| panic!("the listing holds no summary for {job_id}: {body}"))
        .clone()
}

/// The workspaces, the two datasets and one job per row of [`JOB_CASES`]: the
/// fixture every suite about reading jobs works from. Answers the router, the
/// dataset ids and the `jobId` each row was created as.
async fn fixture() -> (MultiServiceRouter, Vec<String>, Vec<String>) {
    let router = router();
    for name in [WORKSPACE, OTHER_WORKSPACE, QUIET_WORKSPACE] {
        create_workspace(&router, name).await;
    }
    let datasets = vec![
        create_dataset(&router, "Camera footage").await,
        create_dataset(&router, "Door footage").await,
    ];

    let mut created = Vec::new();
    for case in JOB_CASES {
        let label = case.label;
        let (status, body) = create(
            &router,
            WORKSPACE,
            create_body(case, &datasets[case.dataset]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} {body}"
        );
        let job_id = body["jobId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&job_id),
            "case {label:?}: jobId must have the modelled form, got {body}"
        );
        assert!(
            ["PENDING", "RUNNING"].contains(&body["status"].as_str().unwrap_or("")),
            "case {label:?}: a job answered by its own create is not terminal, got {body}"
        );
        assert!(
            body["createdAt"].as_f64().unwrap_or(-1.0) > 0.0,
            "case {label:?}: createdAt is the instant the job was created, got {body}"
        );
        created.push(job_id);
    }
    (router, datasets, created)
}

#[tokio::test]
async fn a_created_job_is_listed_with_the_configuration_it_was_created_from() {
    let (router, datasets, created) = fixture().await;

    let body = list_all(&router, WORKSPACE, "").await;
    assert_eq!(
        ids(&body).into_iter().collect::<BTreeSet<String>>(),
        created.iter().cloned().collect::<BTreeSet<String>>(),
        "a listing reports exactly the jobs the creates started, got {body}"
    );

    for (case, job_id) in JOB_CASES.iter().zip(&created) {
        let label = case.label;
        let summary = summary_of(&body, job_id);
        assert_eq!(
            summary["workspaceName"].as_str(),
            Some(WORKSPACE),
            "case {label:?}: a summary names the workspace the job was created in, got {summary}"
        );
        assert_eq!(
            summary["jobType"].as_str(),
            Some("EVENT_DETECTION"),
            "case {label:?}: an eventDetection configuration makes an EVENT_DETECTION job, \
             got {summary}"
        );
        assert_eq!(
            summary["datasetId"].as_str(),
            Some(datasets[case.dataset].as_str()),
            "case {label:?}: a summary names the dataset the configuration did, got {summary}"
        );
        let (alias, series) = match case.target {
            Target::Dataset => (None, None),
            Target::Alias(alias) => (Some(alias), None),
            Target::Series(series) => (None, Some(series)),
        };
        assert_eq!(
            summary["propertyAlias"].as_str(),
            alias,
            "case {label:?}: propertyAlias is reported only when the create named one, \
             got {summary}"
        );
        assert_eq!(
            summary["timeSeriesId"].as_str(),
            series,
            "case {label:?}: timeSeriesId is reported only when the create named one, \
             got {summary}"
        );
        assert!(
            summary["createdAt"].as_f64().unwrap_or(-1.0) > 0.0,
            "case {label:?}: createdAt is the instant the job was created, got {summary}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_only_the_jobs_of_the_workspace_it_names() {
    let (router, datasets, here) = fixture().await;

    // The same configurations in ANOTHER workspace are jobs of their own, so a
    // listing that leaked across workspaces cannot pass.
    let mut there = Vec::new();
    for case in JOB_CASES.iter().take(2) {
        let (status, body) = create(
            &router,
            OTHER_WORKSPACE,
            create_body(case, &datasets[case.dataset]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?} in {OTHER_WORKSPACE}: got {status} {body}",
            case.label
        );
        there.push(body["jobId"].as_str().unwrap_or("").to_string());
    }

    for (case, workspace, expected) in [
        ("the workspace the jobs were created in", WORKSPACE, &here),
        ("another workspace with jobs of its own", OTHER_WORKSPACE, &there),
    ] {
        let body = list_all(&router, workspace, "").await;
        assert_eq!(
            ids(&body).into_iter().collect::<BTreeSet<String>>(),
            expected.iter().cloned().collect::<BTreeSet<String>>(),
            "case {case:?}: a listing reports exactly that workspace's jobs, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: a listing that reported everything carries no nextToken, got {body}"
        );
    }

    // A held workspace nobody created a job in is an EMPTY page.
    let empty = listed(&router, QUIET_WORKSPACE, "").await;
    assert!(
        ids(&empty).is_empty() && empty["nextToken"].is_null(),
        "a workspace with no job answers an empty page, got {empty}"
    );

    // A workspace this service does not hold, in every form of a name no
    // `CreateWorkspace` minted — including ones differing from a held name only in
    // case. The workspace the PATH names is resolved before the filters are read, so
    // a request that is wrong in both ways still reports the resource that is absent.
    for name in [
        "unheld-floor",
        "ENRICHMENT-FLOOR",
        "Enrichment-Floor",
        "enrichment-floor-copy",
    ] {
        for query in [
            "",
            "status=COMPLETED",
            "maxResults=1",
            "nextToken=zzzz",
            &format!("datasetId={UNKNOWN_DATASET}"),
        ] {
            let (status, body) = list_raw(&router, name, query).await;
            assert_eq!(
                status, 404,
                "workspace {name:?} ?{query}: expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "workspace {name:?} ?{query}: unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_summary_reports_the_status_the_clock_has_made_due() {
    let (router, datasets, created) = fixture().await;

    // ONE more job, created immediately before the read and timed, so the transient
    // assertions below rest on a measured elapsed time rather than on the whole
    // fixture finishing inside the span the clock owes a job.
    let mut fresh = JOB_CASES[0];
    fresh.target = Target::Alias("/company/windfarm/9/turbine/1/camera");
    fresh.client_token = None;
    let since = std::time::Instant::now();
    let (status, answered) = create(&router, WORKSPACE, create_body(&fresh, &datasets[0])).await;
    assert_eq!(status, 200, "the timed create got {status} {answered}");
    assert_eq!(
        answered["status"].as_str(),
        Some("PENDING"),
        "a job is PENDING at the instant its create answers, got {answered}"
    );
    let fresh_id = answered["jobId"].as_str().unwrap_or_default().to_string();

    let body = list_all(&router, WORKSPACE, "").await;
    let elapsed = since.elapsed();
    let summary = summary_of(&body, &fresh_id);
    let status = summary["status"].as_str().unwrap_or_default();
    // Guarded on the measured elapsed time: only a read taken inside the span the
    // clock owes this job can say anything about a transient status.
    if elapsed < 2 * ENRICHMENT_JOB_STEP {
        assert!(
            ["PENDING", "RUNNING"].contains(&status),
            "a job read {elapsed:?} after its create is not terminal yet, got {summary}"
        );
    }
    if status == "PENDING" {
        assert!(
            summary["updatedAt"].is_null(),
            "a job no transition has moved reports no updatedAt, got {summary}"
        );
    } else {
        assert!(
            summary["updatedAt"].as_f64().unwrap_or(-1.0)
                >= summary["createdAt"].as_f64().unwrap_or(0.0),
            "updatedAt precedes createdAt, got {summary}"
        );
    }

    // A listing REPORTS: two listings of one unchanged store agree, and neither
    // moves a job on.
    let again = list_all(&router, WORKSPACE, "").await;
    assert_eq!(
        ids(&again),
        ids(&body),
        "two listings of one store must report the same jobs in the same order"
    );

    tokio::time::sleep(settled()).await;

    let settled_body = list_all(&router, WORKSPACE, "").await;
    assert_eq!(
        ids(&settled_body).len(),
        created.len() + 1,
        "every created job is still listed once, got {settled_body}"
    );
    for summary in settled_body["jobs"].as_array().expect("a jobs list") {
        let job_id = summary["jobId"].as_str().unwrap_or_default();
        assert_eq!(
            summary["status"].as_str(),
            Some("COMPLETED"),
            "case {job_id}: a job the clock has finished is COMPLETED, got {summary}"
        );
        let created_at = summary["createdAt"].as_f64().unwrap_or(-1.0);
        let updated_at = summary["updatedAt"].as_f64().unwrap_or(-1.0);
        assert!(
            updated_at >= created_at && created_at > 0.0,
            "case {job_id}: updatedAt {updated_at} precedes createdAt {created_at}"
        );
    }

    // A terminal status never moves again, so a second read of the settled store
    // reports the same statuses.
    let settled_again = list_all(&router, WORKSPACE, "").await;
    assert_eq!(
        ids(&settled_again),
        ids(&settled_body),
        "two listings of one settled store must agree, got {settled_again}"
    );
}

#[tokio::test]
async fn each_filter_selects_only_the_jobs_it_names() {
    let (router, datasets, created) = fixture().await;
    let all: BTreeSet<String> = created.iter().cloned().collect();

    // The ids each row expects, named by the row of `JOB_CASES` that created them.
    let by_case = |wanted: &[usize]| -> BTreeSet<String> {
        wanted.iter().map(|at| created[*at].clone()).collect()
    };

    for (case, query, expected) in [
        (
            "the first dataset",
            format!("datasetId={}", datasets[0]),
            by_case(&[0, 1, 2, 3]),
        ),
        (
            "the second dataset",
            format!("datasetId={}", datasets[1]),
            by_case(&[4, 5]),
        ),
        (
            "one propertyAlias, which two jobs of different datasets carry",
            format!("propertyAlias={CAMERA_ALIAS}"),
            by_case(&[2, 5]),
        ),
        (
            "another propertyAlias",
            format!("propertyAlias={DOOR_ALIAS}"),
            by_case(&[3]),
        ),
        (
            "the shortest timeSeriesId",
            format!("timeSeriesId={SHORT_SERIES}"),
            by_case(&[0]),
        ),
        (
            "the longest timeSeriesId",
            format!("timeSeriesId={LONG_SERIES}"),
            by_case(&[1]),
        ),
        (
            "the only job type the model declares",
            format!("jobType={}", JOB_TYPES[0]),
            all.clone(),
        ),
        (
            "a dataset and an alias together",
            format!("datasetId={}&propertyAlias={CAMERA_ALIAS}", datasets[1]),
            by_case(&[5]),
        ),
        (
            "a dataset and an alias no one job carries together",
            format!("datasetId={}&propertyAlias={DOOR_ALIAS}", datasets[1]),
            BTreeSet::new(),
        ),
        (
            "a dataset the store does not hold",
            format!("datasetId={UNKNOWN_DATASET}"),
            BTreeSet::new(),
        ),
        (
            "an alias no job was created with",
            format!("propertyAlias={UNUSED_ALIAS}"),
            BTreeSet::new(),
        ),
    ] {
        let body = list_all(&router, WORKSPACE, &query).await;
        assert_eq!(
            ids(&body).into_iter().collect::<BTreeSet<String>>(),
            expected,
            "case {case:?} (?{query}): expected {} jobs, got {body}",
            expected.len()
        );
    }

    // While the clock still owes these jobs their transitions, the only invariant
    // that cannot depend on when the test runs: a filtered page reports jobs in THAT
    // status and nothing else.
    for status in ENRICHMENT_JOB_STATUSES {
        let body = list_all(&router, WORKSPACE, &format!("status={status}")).await;
        for summary in body["jobs"].as_array().expect("a jobs list") {
            assert_eq!(
                summary["status"].as_str(),
                Some(status),
                "case status={status}: a filtered page reports only that status, got {summary}"
            );
            assert!(
                all.contains(summary["jobId"].as_str().unwrap_or_default()),
                "case status={status}: a filtered page reports only this workspace's jobs, \
                 got {summary}"
            );
        }
    }

    tokio::time::sleep(settled()).await;

    // Terminal statuses never move again, so the partition is now stable: every job
    // this workspace holds is COMPLETED, and no other status holds one.
    for status in ENRICHMENT_JOB_STATUSES {
        let body = list_all(&router, WORKSPACE, &format!("status={status}")).await;
        let expected: BTreeSet<String> = if status == "COMPLETED" {
            all.clone()
        } else {
            BTreeSet::new()
        };
        assert_eq!(
            ids(&body).into_iter().collect::<BTreeSet<String>>(),
            expected,
            "case status={status}: expected {} jobs, got {body}",
            expected.len()
        );
    }

    // A value a modelled catalogue does not hold, or two filters that cannot both
    // be satisfied, name nothing this service could report, so each is the bad
    // request it is rather than an empty page. The enum members are refused by the
    // modelled value set before the handler runs; the rest are its own rules.
    for (case, query) in [
        ("a status in the wrong case", "status=completed".to_string()),
        ("a near-miss status", "status=RUNNIN".to_string()),
        ("a status of another API", "status=SUBMITTED".to_string()),
        ("a job type in the wrong case", "jobType=event_detection".to_string()),
        ("a job type the model does not declare", "jobType=OBJECT_DETECTION".to_string()),
        (
            "both identifiers of one time series",
            format!("propertyAlias={CAMERA_ALIAS}&timeSeriesId={SHORT_SERIES}"),
        ),
        (
            "a date window whose end precedes its start",
            format!(
                "startDate={}&endDate={}",
                query_date(1_700_000_600),
                query_date(1_700_000_000)
            ),
        ),
        ("a token this operation never issued", "nextToken=zzzz".to_string()),
        (
            "a token resuming at a job this workspace does not hold",
            format!("nextToken={}", hex_token(UNKNOWN_DATASET)),
        ),
    ] {
        let (status, body) = list_raw(&router, WORKSPACE, &query).await;
        assert_eq!(
            status, 400,
            "case {case:?} (?{query}): expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?} (?{query}): unexpected error code in {body}"
        );
    }

    // `maxResults` carries @range 1..250 (`smithy_query(query="operation",
    // operation="ListEnrichmentJobs")`): each published bound is served and one past
    // it on either side is refused.
    for (case, max_results, served) in [
        ("the range minimum", 1_i64, true),
        ("the range maximum", 250, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 251, false),
    ] {
        let (status, body) = list_raw(&router, WORKSPACE, &format!("maxResults={max_results}")).await;
        let expected = if served { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?} (maxResults={max_results}): expected {expected}, got {status} {body}"
        );
    }
}

/// The `jobId`s the date-window suite seeds, with the instant each was created at.
/// Chosen instants, well in the past, so every seeded job is settled and the window
/// bounds can name a job's creation instant EXACTLY.
const SEEDED: &[(&str, i64)] = &[
    ("aaaaaaaa-0000-4000-8000-000000000001", 1_700_000_000),
    ("aaaaaaaa-0000-4000-8000-000000000002", 1_700_000_060),
    ("aaaaaaaa-0000-4000-8000-000000000003", 1_700_000_120),
];

/// One stored job of `workspace` analysing `dataset_id` by `alias`, created at
/// `created_at` — the record a create would have written.
fn job_record(
    workspace: &str,
    dataset_id: &str,
    job_id: &str,
    alias: &str,
    created_at: std::time::SystemTime,
) -> EnrichmentJobRecord {
    EnrichmentJobRecord {
        job_id: job_id.to_string(),
        workspace_name: workspace.to_string(),
        configuration: EnrichmentJobConfiguration::eventDetection(EventDetection {
            dataset_id: dataset_id.to_string(),
            time_series_id: None,
            property_alias: Some(alias.to_string()),
            trim_settings: EnrichmentTrimSettings {
                start_time: time_in_nanos(1_700_000_000, 0),
                end_time: time_in_nanos(1_700_000_600, 0),
            },
        }),
        client_token: None,
        created_at,
    }
}

/// Seed the jobs of [`SEEDED`] into `workspace`, each analysing `dataset_id`.
///
/// Written straight into this service's own store, the one `ListEnrichmentJobs`
/// reads, because the instant a create stamps a job with is `now` — and the window
/// rule is about bounds that fall exactly on, before and after a job's creation.
fn seed_jobs(engine: &Arc<engine::InMemoryEngine>, workspace: &str, dataset_id: &str) {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    for (job_id, created_at) in SEEDED {
        let record = job_record(
            workspace,
            dataset_id,
            job_id,
            CAMERA_ALIAS,
            UNIX_EPOCH + Duration::from_secs(*created_at as u64),
        );
        store
            .enrichment_jobs
            .insert(enrichment_job_key(workspace, job_id), record);
    }
}

/// Fill `workspace` with `count` stored jobs, each on an alias of its own so none
/// of them is a duplicate of another: how the bound on how many jobs one workspace
/// holds is reached without driving a hundred creates through the API.
fn seed_filler_jobs(
    engine: &Arc<engine::InMemoryEngine>,
    workspace: &str,
    dataset_id: &str,
    count: usize,
) {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    for at in 0..count {
        // An `ID`-shaped id of its own per job: lowercase hex, version nibble 4.
        let job_id = format!("cccccccc-0000-4000-8000-{at:012}");
        let record = job_record(
            workspace,
            dataset_id,
            &job_id,
            &format!("/company/warehouse/filler/{at}"),
            std::time::SystemTime::now(),
        );
        store
            .enrichment_jobs
            .insert(enrichment_job_key(workspace, &job_id), record);
    }
}

/// The shortest `clientToken` the input admits. Source: `smithy_query(query="shape",
/// shape="ClientToken")` — `@length(min: 36, max: 64)` with pattern `^\S{36,64}$`.
const CLIENT_TOKEN_MIN: usize = 36;

/// The longest `clientToken` that same shape admits.
const CLIENT_TOKEN_MAX: usize = 64;

/// A `clientToken` of exactly `length` non-space characters, distinct for each
/// `label`: a token is what identifies ONE create, so no two rows may share one.
fn client_token(label: &str, length: usize) -> String {
    let mut token: String = label.chars().filter(|ch| !ch.is_whitespace()).collect();
    token.truncate(length);
    while token.len() < length {
        token.push('t');
    }
    token
}

#[tokio::test]
async fn a_client_token_replays_its_job_for_every_length_the_input_models() {
    let (router, engine) = router_with_engine();
    create_workspace(&router, WORKSPACE).await;
    let dataset_id = create_dataset(&router, "Camera footage").await;

    let held = |engine: &Arc<engine::InMemoryEngine>| -> usize {
        let state = service_state(&**engine).expect("the IoT SiteWise store");
        let store = state.lock();
        store.enrichment_jobs.len()
    };

    // Rows: the `@length` floor of `ClientToken`, a middling length and its
    // ceiling. Each row's job analyses a time series of its own, so no row is the
    // duplicate of the job another row started.
    let rows: [(&str, usize); 3] = [
        ("a token at the length floor", CLIENT_TOKEN_MIN),
        (
            "a token of a middling length",
            (CLIENT_TOKEN_MIN + CLIENT_TOKEN_MAX) / 2,
        ),
        ("a token at the length ceiling", CLIENT_TOKEN_MAX),
    ];

    let mut started: Vec<(String, f64)> = Vec::new();
    for (at, (case, length)) in rows.iter().enumerate() {
        let token = client_token(case, *length);
        assert_eq!(
            token.len(),
            *length,
            "case {case:?}: this row needs a token of {length} characters, got {token:?}"
        );
        let alias = format!("/company/windfarm/{at}/turbine/{at}/camera");
        let body = |alias: &str, token: &str| {
            json!({
                "clientToken": token,
                "jobConfiguration": { "eventDetection": {
                    "datasetId": dataset_id,
                    "propertyAlias": alias,
                    "trimSettings": {
                        "startTime": instant(1_700_000_000, 0),
                        "endTime": instant(1_700_003_600, 0),
                    },
                }},
            })
        };

        let since = std::time::Instant::now();
        let (status, first) = create(&router, WORKSPACE, body(&alias, &token)).await;
        let elapsed = since.elapsed();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {first}"
        );
        let job_id = first["jobId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&job_id),
            "case {case:?}: jobId must have the modelled form, got {first}"
        );
        let answered = first["status"].as_str().unwrap_or("").to_string();
        assert!(
            ENRICHMENT_JOB_STATUSES.contains(&answered.as_str()),
            "case {case:?}: status must be a modelled value, got {first}"
        );
        // "Initial status of the enrichment job, typically PENDING": guarded on the
        // measured elapsed time, because the status is the one the clock has made
        // due and only a call inside the first step can rely on seeing the first.
        if elapsed < ENRICHMENT_JOB_STEP {
            assert_eq!(
                answered, "PENDING",
                "case {case:?}: a job answered {elapsed:?} after its create has not started \
                 processing yet, got {first}"
            );
        }
        let created_at = first["createdAt"].as_f64().unwrap_or(-1.0);
        assert!(
            created_at > 0.0,
            "case {case:?}: createdAt is the instant the job was created, got {first}"
        );

        // The very same request again: the idempotency replay is resolved BEFORE the
        // duplicate-job rule, so an identical retry replays the job its token
        // started rather than colliding with it.
        let (status, replayed) = create(&router, WORKSPACE, body(&alias, &token)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an identical retry got {status} {replayed}"
        );
        assert_eq!(
            replayed["jobId"].as_str(),
            Some(job_id.as_str()),
            "case {case:?}: an identical retry replays the first job, got {replayed}"
        );
        assert_eq!(
            replayed["createdAt"].as_f64(),
            Some(created_at),
            "case {case:?}: a replay reports the job's own creation instant, got {replayed}"
        );

        // The same token asking for other work replays that same job too: the token
        // identifies the create, not the configuration it carried.
        let (status, again) = create(&router, WORKSPACE, body(UNUSED_ALIAS, &token)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a retry asking for other work got {status} {again}"
        );
        assert_eq!(
            again["jobId"].as_str(),
            Some(job_id.as_str()),
            "case {case:?}: the token replays the job it started, got {again}"
        );

        assert_eq!(
            held(&engine),
            at + 1,
            "case {case:?}: only the first create carrying a token starts a job"
        );
        started.push((job_id, created_at));
    }

    // Distinct tokens started jobs of their own, and each job the creates answered
    // is the one the store holds: the listing reports every jobId with the
    // createdAt its own create answered, so the answer came off the record.
    let ids_started: BTreeSet<String> = started.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(
        ids_started.len(),
        rows.len(),
        "every row started a job of its own, got {started:?}"
    );
    let body = list_all(&router, WORKSPACE, "").await;
    assert_eq!(
        ids(&body).into_iter().collect::<BTreeSet<String>>(),
        ids_started,
        "the listing reports exactly the jobs the rows started, got {body}"
    );
    for (job_id, created_at) in &started {
        let summary = summary_of(&body, job_id);
        assert_eq!(
            summary["createdAt"].as_f64(),
            Some(*created_at),
            "job {job_id}: the create answered the instant the stored job carries, got {summary}"
        );
    }

    // One character short of the floor and one past the ceiling: client input the
    // modelled length does not admit, refused without starting a job. Only the
    // STATUS is asserted, as the dataset bounds suite's past-the-ceiling row does:
    // a token outside the length is refused by the boundary validator before this
    // handler runs, and the error code that walk renders is its own subject.
    let before = held(&engine);
    for (case, length) in [
        ("one character short of the floor", CLIENT_TOKEN_MIN - 1),
        ("one character past the ceiling", CLIENT_TOKEN_MAX + 1),
    ] {
        let token = client_token("out of range", length);
        let (status, refused) = create(
            &router,
            WORKSPACE,
            json!({
                "clientToken": token,
                "jobConfiguration": { "eventDetection": {
                    "datasetId": dataset_id,
                    "propertyAlias": UNUSED_ALIAS,
                    "trimSettings": {
                        "startTime": instant(1_700_000_000, 0),
                        "endTime": instant(1_700_003_600, 0),
                    },
                }},
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a {length}-character clientToken must be refused, \
             got {status} {refused}"
        );
        assert_eq!(
            held(&engine),
            before,
            "case {case:?}: a refused create must start no job"
        );
    }

    // The same work with a token the length DOES admit is accepted, so the two rows
    // above were refused for the length they name and not for the rest of the
    // request.
    let (status, accepted) = create(
        &router,
        WORKSPACE,
        json!({
            "clientToken": client_token("in range", CLIENT_TOKEN_MIN),
            "jobConfiguration": { "eventDetection": {
                "datasetId": dataset_id,
                "propertyAlias": UNUSED_ALIAS,
                "trimSettings": {
                    "startTime": instant(1_700_000_000, 0),
                    "endTime": instant(1_700_003_600, 0),
                },
            }},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a create carrying a token inside the modelled length got {status} {accepted}"
    );
    assert_eq!(
        held(&engine),
        before + 1,
        "the accepted create started one job"
    );
}

/// A router over a fresh engine holding the workspace and the seeded jobs.
async fn seeded() -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    create_workspace(&router, WORKSPACE).await;
    let dataset_id = create_dataset(&router, "Seeded footage").await;
    seed_jobs(&engine, WORKSPACE, &dataset_id);
    router
}

#[tokio::test]
async fn the_date_window_excludes_its_start_and_includes_its_end() {
    let router = seeded().await;
    let job = |at: usize| SEEDED[at].0.to_string();
    let at = |at: usize| SEEDED[at].1;

    // Rows: each bound on a job's own creation instant, one second before it and one
    // second after it, each bound alone and the two together, and a window whose
    // bounds are the same instant.
    for (case, query, expected) in [
        ("no window at all", String::new(), vec![job(2), job(1), job(0)]),
        (
            "a startDate ON the middle job's creation instant excludes it",
            format!("startDate={}", query_date(at(1))),
            vec![job(2)],
        ),
        (
            "an endDate ON the middle job's creation instant includes it",
            format!("endDate={}", query_date(at(1))),
            vec![job(1), job(0)],
        ),
        (
            "a startDate one second before the middle job's instant keeps it",
            format!("startDate={}", query_date(at(1) - 1)),
            vec![job(2), job(1)],
        ),
        (
            "an endDate one second before the middle job's instant drops it",
            format!("endDate={}", query_date(at(1) - 1)),
            vec![job(0)],
        ),
        (
            "both bounds on creation instants keep only the inclusive end",
            format!(
                "startDate={}&endDate={}",
                query_date(at(0)),
                query_date(at(1))
            ),
            vec![job(1)],
        ),
        (
            "a window spanning every job",
            format!(
                "startDate={}&endDate={}",
                query_date(at(0) - 1),
                query_date(at(2))
            ),
            vec![job(2), job(1), job(0)],
        ),
        (
            "a window of one instant, which is exclusive at its start",
            format!(
                "startDate={}&endDate={}",
                query_date(at(1)),
                query_date(at(1))
            ),
            vec![],
        ),
        (
            "a startDate after every job",
            format!("startDate={}", query_date(at(2))),
            vec![],
        ),
        (
            "an endDate before every job",
            format!("endDate={}", query_date(at(0) - 1)),
            vec![],
        ),
    ] {
        let body = list_all(&router, WORKSPACE, &query).await;
        assert_eq!(
            ids(&body),
            expected,
            "case {case:?} (?{query}): expected {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn a_listing_is_ordered_newest_first_and_pages_one_stable_enumeration() {
    let router = seeded().await;
    let newest_first: Vec<String> = SEEDED.iter().rev().map(|(id, _)| id.to_string()).collect();
    let total = newest_first.len();

    // "Results are ordered by createdAt timestamp descending (newest first)."
    let baseline = list_all(&router, WORKSPACE, "").await;
    assert_eq!(
        ids(&baseline),
        newest_first,
        "a listing is ordered by createdAt descending, got {baseline}"
    );
    for summary in baseline["jobs"].as_array().expect("a jobs list") {
        let job_id = summary["jobId"].as_str().unwrap_or_default().to_string();
        let seeded_at = SEEDED
            .iter()
            .find(|(id, _)| *id == job_id)
            .map(|(_, at)| *at)
            .unwrap_or_else(|| panic!("an unseeded job {job_id} in {baseline}"));
        assert_eq!(
            summary["createdAt"].as_f64(),
            Some(seeded_at as f64),
            "case {job_id}: a summary reports the instant the job was created, got {summary}"
        );
        assert_eq!(
            summary["status"].as_str(),
            Some("COMPLETED"),
            "case {job_id}: a job created long ago is COMPLETED, got {summary}"
        );
    }

    // Rows: a page smaller than the listing, one that divides it, one that does not,
    // exactly the listing, more than it, and no `maxResults` at all.
    for page_size in [Some(1_i64), Some(2), Some(3), Some(4), Some(250), None] {
        let case = format!("maxResults={page_size:?}");
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0usize;
        let mut token: Option<String> = None;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = &token {
                query.push(format!("nextToken={token}"));
            }
            let body = listed(&router, WORKSPACE, &query.join("&")).await;
            let page = ids(&body);
            pages += 1;
            if let Some(size) = page_size {
                assert!(
                    page.len() as i64 <= size,
                    "case {case:?}: page {pages} holds {} summaries, more than asked: {body}",
                    page.len()
                );
            }
            walked.extend(page);
            match body["nextToken"].as_str() {
                Some(next) => {
                    assert!(
                        walked.len() < total,
                        "case {case:?}: page {pages} ends the listing yet carries {next}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
            assert!(
                pages <= total + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            walked, newest_first,
            "case {case:?}: the pages joined must be the whole listing once, in one order"
        );
        let expected_pages = match page_size {
            Some(size) => total.div_ceil(size as usize),
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages}"
        );
    }

    // The token a page carries names the job the NEXT page starts at, in the form
    // this listing issues: the hex of that job's id, built by an oracle that does not
    // share the handler's encoder.
    let first = listed(&router, WORKSPACE, "maxResults=1").await;
    assert_eq!(
        first["nextToken"].as_str(),
        Some(hex_token(&newest_first[1]).as_str()),
        "the first page's token resumes at the second job, got {first}"
    );
}

#[tokio::test]
async fn a_create_is_refused_without_starting_a_job_when_its_request_is_wrong() {
    let (router, engine) = router_with_engine();
    create_workspace(&router, WORKSPACE).await;
    let dataset_id = create_dataset(&router, "Camera footage").await;

    let held = |engine: &Arc<engine::InMemoryEngine>| -> usize {
        let state = service_state(&**engine).expect("the IoT SiteWise store");
        let store = state.lock();
        store.enrichment_jobs.len()
    };

    // Rows: both identifiers of one time series at once, and trim windows whose end
    // does not follow their start — each a rule the configuration's own shapes state.
    for (case, body, expected_status, expected_code) in [
        (
            "both identifiers of one time series",
            json!({
                "jobConfiguration": { "eventDetection": {
                    "datasetId": dataset_id,
                    "timeSeriesId": SHORT_SERIES,
                    "propertyAlias": CAMERA_ALIAS,
                    "trimSettings": {
                        "startTime": instant(1_700_000_000, 0),
                        "endTime": instant(1_700_000_600, 0),
                    },
                }},
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a trim window whose end precedes its start",
            json!({
                "jobConfiguration": { "eventDetection": {
                    "datasetId": dataset_id,
                    "propertyAlias": CAMERA_ALIAS,
                    "trimSettings": {
                        "startTime": instant(1_700_000_600, 0),
                        "endTime": instant(1_700_000_000, 0),
                    },
                }},
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a trim window of no time at all",
            json!({
                "jobConfiguration": { "eventDetection": {
                    "datasetId": dataset_id,
                    "propertyAlias": CAMERA_ALIAS,
                    "trimSettings": {
                        "startTime": instant(1_700_000_000, 500),
                        "endTime": instant(1_700_000_000, 500),
                    },
                }},
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a dataset the store does not hold",
            json!({
                "jobConfiguration": { "eventDetection": {
                    "datasetId": UNKNOWN_DATASET,
                    "propertyAlias": CAMERA_ALIAS,
                    "trimSettings": {
                        "startTime": instant(1_700_000_000, 0),
                        "endTime": instant(1_700_000_600, 0),
                    },
                }},
            }),
            404,
            "ResourceNotFoundException",
        ),
    ] {
        let (status, answered) = create(&router, WORKSPACE, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}: unexpected error code in {answered}"
        );
        assert_eq!(
            held(&engine),
            0,
            "case {case:?}: a refused create must start no job"
        );
    }

    // A workspace this service does not hold is the modelled not-found refusal, and
    // writes nothing.
    let good = create_body(&JOB_CASES[2], &dataset_id);
    let (status, answered) = create(&router, "unheld-floor", good.clone()).await;
    assert_eq!(
        status, 404,
        "a create in a workspace this service does not hold: got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ResourceNotFoundException",
        "a create in an unheld workspace: unexpected error code in {answered}"
    );
    assert_eq!(held(&engine), 0, "a refused create must start no job");

    // The same request in a held workspace is accepted, so the rows above were
    // refused for the reason each names and not because the request was unusable.
    let since = std::time::Instant::now();
    let (status, started) = create(&router, WORKSPACE, good).await;
    assert_eq!(status, 200, "the accepted create got {status} {started}");
    assert_eq!(held(&engine), 1, "the accepted create started one job");

    // "No duplicate job (same workspace, dataset, property, and job type) is
    // currently running": a second create for the work that job is still doing is
    // the modelled conflict, whatever else the request names, and starts no job.
    let mut duplicate = JOB_CASES[2];
    duplicate.trim = (1_700_000_000, 0, 1_700_009_000, 0);
    duplicate.client_token = None;
    let (status, refused) = create(&router, WORKSPACE, create_body(&duplicate, &dataset_id)).await;
    let elapsed = since.elapsed();
    // Guarded on the measured elapsed time: the job it collides with has to still be
    // running, which only a call inside the span the clock owes it can rely on.
    if elapsed < 2 * ENRICHMENT_JOB_STEP {
        assert_eq!(
            status, 409,
            "a duplicate of a job running for {elapsed:?}: expected 409, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictingOperationException",
            "a duplicate of a running job: unexpected error code in {refused}"
        );
        assert_eq!(
            refused["resourceId"].as_str(),
            started["jobId"].as_str(),
            "the conflict names the job it collides with, got {refused}"
        );
        assert_eq!(held(&engine), 1, "a refused create must start no job");
    }

    // Once that job has reached its terminal status it is no longer running, so the
    // same work can be started again.
    tokio::time::sleep(settled()).await;
    let before = held(&engine);
    let (status, second) = create(&router, WORKSPACE, create_body(&duplicate, &dataset_id)).await;
    assert_eq!(
        status, 200,
        "a job for work no running job does: got {status} {second}"
    );
    assert_ne!(
        second["jobId"].as_str(),
        started["jobId"].as_str(),
        "a second create started its own job, got {second}"
    );
    assert_eq!(
        held(&engine),
        before + 1,
        "the second create started one more job"
    );
}

#[tokio::test]
async fn a_retried_create_replays_the_job_its_token_already_started() {
    let (router, engine) = router_with_engine();
    for name in [WORKSPACE, OTHER_WORKSPACE] {
        create_workspace(&router, name).await;
    }
    let dataset_id = create_dataset(&router, "Camera footage").await;

    // The row carrying a `clientToken`, so both calls below send the same one.
    let case = JOB_CASES[1];
    assert!(
        case.client_token.is_some(),
        "case {:?}: this suite needs a row carrying a clientToken",
        case.label
    );
    let (status, first) = create(&router, WORKSPACE, create_body(&case, &dataset_id)).await;
    assert_eq!(status, 200, "the first create got {status} {first}");

    // A retry carrying the same token reports the job the FIRST call started, even
    // when the rest of the request differs, and starts nothing.
    let mut retried = case;
    retried.target = Target::Alias(UNUSED_ALIAS);
    retried.trim = (1_700_000_000, 0, 1_700_000_900, 0);
    let (status, replayed) = create(&router, WORKSPACE, create_body(&retried, &dataset_id)).await;
    assert_eq!(status, 200, "the retry got {status} {replayed}");
    assert_eq!(
        replayed["jobId"].as_str(),
        first["jobId"].as_str(),
        "a retry carrying the same token replays the first job, got {replayed}"
    );
    assert_eq!(
        replayed["createdAt"].as_f64(),
        first["createdAt"].as_f64(),
        "a replay reports the job's own creation instant, got {replayed}"
    );

    // The job it replayed is unchanged: the listing still reports the configuration
    // the FIRST create carried.
    let body = list_all(&router, WORKSPACE, "").await;
    assert_eq!(
        ids(&body),
        vec![first["jobId"].as_str().unwrap_or_default().to_string()],
        "a replayed retry wrote nothing, got {body}"
    );
    let summary = summary_of(&body, first["jobId"].as_str().unwrap_or_default());
    assert_eq!(
        (
            summary["timeSeriesId"].as_str(),
            summary["propertyAlias"].as_str()
        ),
        (Some(LONG_SERIES), None),
        "a replay must not rewrite the job it replays, got {summary}"
    );

    // The same token in ANOTHER workspace is a different call, since a job is
    // addressed within its workspace.
    let (status, elsewhere) = create(&router, OTHER_WORKSPACE, create_body(&case, &dataset_id)).await;
    assert_eq!(status, 200, "the create in {OTHER_WORKSPACE} got {status} {elsewhere}");
    assert_ne!(
        elsewhere["jobId"].as_str(),
        first["jobId"].as_str(),
        "the same token in another workspace starts a job of its own, got {elsewhere}"
    );

    let held = {
        let state = service_state(&*engine).expect("the IoT SiteWise store");
        let store = state.lock();
        store.enrichment_jobs.len()
    };
    assert_eq!(held, 2, "only the two accepted creates wrote a job");
}

#[tokio::test]
async fn a_create_past_the_bound_on_what_one_workspace_holds_is_refused() {
    let (router, engine) = router_with_engine();
    for name in [WORKSPACE, OTHER_WORKSPACE] {
        create_workspace(&router, name).await;
    }
    let dataset_id = create_dataset(&router, "Camera footage").await;

    // A create is the only thing that adds a job and nothing evicts one, so the
    // store bounds how many one workspace holds. Filled to that bound exactly.
    seed_filler_jobs(
        &engine,
        WORKSPACE,
        &dataset_id,
        MAX_ENRICHMENT_JOBS_PER_WORKSPACE,
    );

    let held = |engine: &Arc<engine::InMemoryEngine>| -> usize {
        let state = service_state(&**engine).expect("the IoT SiteWise store");
        let store = state.lock();
        store.enrichment_jobs.len()
    };
    assert_eq!(
        held(&engine),
        MAX_ENRICHMENT_JOBS_PER_WORKSPACE,
        "the fill seeded one job per unit of the bound"
    );

    // One more in THAT workspace is the modelled quota refusal, and writes nothing.
    let (status, refused) = create(&router, WORKSPACE, create_body(&JOB_CASES[2], &dataset_id)).await;
    assert_eq!(
        status, 410,
        "a create past the per-workspace bound: expected 410, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "a create past the per-workspace bound: unexpected error code in {refused}"
    );
    assert_eq!(
        held(&engine),
        MAX_ENRICHMENT_JOBS_PER_WORKSPACE,
        "a refused create must start no job"
    );

    // The bound is per WORKSPACE: another workspace is unaffected by how full this
    // one is, and the same request there is accepted.
    let (status, started) =
        create(&router, OTHER_WORKSPACE, create_body(&JOB_CASES[2], &dataset_id)).await;
    assert_eq!(
        status, 200,
        "a create in a workspace under the bound: got {status} {started}"
    );
    assert_eq!(
        held(&engine),
        MAX_ENRICHMENT_JOBS_PER_WORKSPACE + 1,
        "the accepted create started one job"
    );

    // The filled workspace still reports a full page of its own jobs, so the refusal
    // above is about the bound and not about a store this listing cannot read.
    let body = list_all(&router, WORKSPACE, "").await;
    assert_eq!(
        ids(&body).len(),
        MAX_ENRICHMENT_JOBS_PER_WORKSPACE.min(250),
        "the filled workspace lists every job it holds, got {body}"
    );
}
