// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListDatasetExportJobs`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a listing enumerates exactly the jobs `CreateDatasetExportJob` started in the
//!   workspace the request names, and no job of another workspace;
//! * a workspace this service holds no record of is the modelled not-found refusal
//!   rather than an empty page, while a workspace it holds with no job of its own
//!   is an empty page;
//! * every summary is read off the stored job, with the status and the completion
//!   instant DERIVED from the instant the create started it — the same members a
//!   read of that one job reports — and a listing moves no job on;
//! * `filter` reports only the jobs in the status it names, measured against the
//!   status the CLOCK has made due, and `ALL` — its documented default — narrows
//!   nothing;
//! * `maxResults`/`nextToken` page one stable enumeration: the pages joined are the
//!   whole listing once, in one order, whatever page size the caller asks for, and
//!   a token this listing never issued is refused.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `hex_token`) and the workspace
//! create are the ones the sibling suites already have; this suite reuses them
//! rather than growing a second copy.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::EXPORT_JOB_STEP;

use crate::application_lifecycle::create_workspace;
use crate::asset_lifecycle::{call, error_code, hex_token, router};

/// The workspace every suite below lists.
const WORKSPACE: &str = "Plant1";

/// A second workspace holding jobs of its own, so a listing can be shown to report
/// only the jobs of the one it names.
const OTHER_WORKSPACE: &str = "Plant2";

/// A third workspace nobody starts a job in, so an empty page can be told apart
/// from an absent workspace.
const EMPTY_WORKSPACE: &str = "Plant3";

/// A fourth workspace, opened by the job starter below rather than by
/// [`fixture`], and holding jobs of its own: a listing is keyed on the workspace
/// half of a job's key, so the jobs started here belong to this workspace's
/// listing and to no other.
const LATE_OPENED_WORKSPACE: &str = "Plant4";

/// How long after a step falls due a read is taken, so a run whose scheduler is
/// late cannot observe the previous status.
const SLACK: Duration = Duration::from_millis(250);

/// The published bounds of `maxResults` (smithy_query query="operation"
/// operation="ListDatasetExportJobs": `@range(min: 1, max: 250)`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 250;

/// The page size this listing serves when the caller sends no `maxResults`, which
/// is the one every paginated listing of this service shares. The fixture is
/// shorter than it, so an unpaged call answers in ONE page.
const DEFAULT_MAX_RESULTS: usize = 50;

/// The `DatasetExportJobFilter` values (smithy_query query="shape"
/// shape="DatasetExportJobFilter").
const FILTER_VALUES: [&str; 6] = [
    "ALL",
    "SUBMITTED",
    "RUNNING",
    "COMPLETED",
    "COMPLETED_WITH_ERRORS",
    "FAILED",
];

/// A `TimeSeriesId` of the associated form, which this service builds as
/// `<assetId>.<propertyId>` — the longest the modelled length (36..73) admits.
const ASSOCIATED_SERIES: &str =
    "11111111-2222-3333-4444-555555555555.66666666-7777-8888-9999-aaaaaaaaaaaa";

/// A `TimeSeriesId` of the shortest form that length admits: a single id, which is
/// how this service names a stream no asset property owns.
const DISASSOCIATED_SERIES: &str = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";

/// The `ExportDataType` values a `DatasetItem` may name a subset of (smithy_query
/// query="shape" shape="ExportDataType").
const EXPORT_DATA_TYPES: [&str; 3] = ["VIDEO", "TELEMETRY", "ANNOTATION"];

/// `POST|GET /workspaces/{workspaceName}/dataset-export-jobs`, as `OPERATIONS.md`
/// reports the route of both `CreateDatasetExportJob` (POST) and
/// `ListDatasetExportJobs` (GET).
fn jobs_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/dataset-export-jobs")
}

/// `GET /workspaces/{workspaceName}/dataset-export-jobs/{jobId}`.
fn job_uri(workspace: &str, job_id: &str) -> String {
    format!("/workspaces/{workspace}/dataset-export-jobs/{job_id}")
}

/// A `TimeInNanos`, as the two members the shape declares.
fn nanos(seconds: i64, offset: i64) -> Value {
    json!({ "timeInSeconds": seconds, "offsetInNanos": offset })
}

/// Create a dataset through `CreateDataset` and answer its id, so a row whose
/// `ProcessingInput` names a dataset names one the store really holds.
async fn create_dataset(router: &MultiServiceRouter, name: &str) -> String {
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

/// One row of the fixture: a job to start, its destination and the processing input
/// the create carries, built from the id of the dataset the suite created.
struct JobCase {
    label: &'static str,
    destination: &'static str,
    input: fn(&str) -> Value,
}

/// The rows: the destination differing per job so a summary cannot be an echo of
/// one job's shape, crossed with `ProcessingInput` spanning both members the union
/// declares — a `timeseries` list of one and of several items, identified by
/// `timeSeriesId` of each length the type admits and by `propertyAlias`, with and
/// without `trimSettings`, and a `dataset` item with no options, with a trim range
/// and with every `ExportDataType`.
fn job_cases() -> Vec<JobCase> {
    vec![
        JobCase {
            label: "one-timeseries-by-the-longest-id",
            destination: "s3://sitewise-exports/clips/one/",
            input: |_| json!({ "timeseries": [{ "timeSeriesId": ASSOCIATED_SERIES }] }),
        },
        JobCase {
            label: "one-timeseries-by-the-shortest-id",
            destination: "s3://sitewise-exports/clips/two/",
            input: |_| json!({ "timeseries": [{ "timeSeriesId": DISASSOCIATED_SERIES }] }),
        },
        JobCase {
            label: "one-timeseries-by-alias-trimmed",
            destination: "s3://sitewise-exports/clips/three/",
            input: |_| {
                json!({
                    "timeseries": [{
                        "propertyAlias": "/company/windfarm/3/turbine/7/temperature",
                        "trimSettings": {
                            "startTime": nanos(1_760_000_000, 0),
                            "endTime": nanos(1_760_003_600, 0),
                        },
                    }]
                })
            },
        },
        JobCase {
            label: "several-timeseries-mixing-ids-and-aliases",
            destination: "s3://sitewise-exports/clips/four/",
            input: |_| {
                json!({
                    "timeseries": [
                        { "timeSeriesId": ASSOCIATED_SERIES },
                        { "propertyAlias": "/company/windfarm/3/turbine/7/vibration" },
                        {
                            "timeSeriesId": DISASSOCIATED_SERIES,
                            "formatSettings": { "framesPerSecond": 1 },
                        },
                    ]
                })
            },
        },
        JobCase {
            label: "dataset-with-no-options",
            destination: "s3://sitewise-exports/clips/five/",
            input: |dataset_id| json!({ "dataset": { "datasetId": dataset_id } }),
        },
        JobCase {
            label: "dataset-trimmed-to-one-data-type",
            destination: "s3://sitewise-exports/clips/six/",
            input: |dataset_id| {
                json!({
                    "dataset": {
                        "datasetId": dataset_id,
                        "trimSettings": {
                            "startTime": nanos(1_700_000_000, 0),
                            "endTime": nanos(1_700_000_001, 0),
                        },
                        "exportDataTypes": [EXPORT_DATA_TYPES[1]],
                    }
                })
            },
        },
        JobCase {
            label: "dataset-with-every-data-type",
            destination: "s3://sitewise-exports/clips/seven/",
            input: |dataset_id| {
                json!({
                    "dataset": { "datasetId": dataset_id, "exportDataTypes": EXPORT_DATA_TYPES }
                })
            },
        },
    ]
}

/// `CreateDatasetExportJob` in `workspace`, answering the `jobId` it minted.
///
/// A job is started INSIDE a workspace, which the create resolves through the
/// workspace registry, so the workspace is opened first.
async fn start_job(
    router: &MultiServiceRouter,
    workspace: &str,
    destination: &str,
    input: Value,
) -> String {
    crate::support::ensure_workspace(router, workspace).await;
    let (status, body) = call(
        router,
        "POST",
        &jobs_uri(workspace),
        Some(json!({
            "destinationS3Uri": destination,
            "errorReportLocation": { "s3Uri": "s3://sitewise-exports/errors/" },
            "input": input,
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "start a job in {workspace}: expected 202, got {status} {body}"
    );
    body["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("start a job in {workspace}: no jobId in {body}"))
        .to_string()
}

/// Start every row of [`job_cases`] in `workspace`: the ids minted, in the order
/// they were started, each with the destination its row named.
async fn start_job_cases(
    router: &MultiServiceRouter,
    workspace: &str,
    dataset_id: &str,
) -> Vec<(String, &'static str, &'static str)> {
    let mut started = Vec::new();
    for case in job_cases() {
        let input = (case.input)(dataset_id);
        let job_id = start_job(router, workspace, case.destination, input).await;
        started.push((job_id, case.label, case.destination));
    }
    started
}

/// `ListDatasetExportJobs` on `workspace` with the raw query string `query` (empty
/// for none).
async fn list_raw(router: &MultiServiceRouter, workspace: &str, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        jobs_uri(workspace)
    } else {
        format!("{}?{query}", jobs_uri(workspace))
    };
    call(router, "GET", &uri, None).await
}

/// `ListDatasetExportJobs`, asserting it answered and that the required `jobs`
/// member is a list, and the body it answered with.
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

/// The summaries of a listing body.
fn summaries(body: &Value) -> &Vec<Value> {
    body["jobs"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a jobs list, got {body}"))
}

/// The job ids a listing body reports, in the order it reported them.
fn job_ids(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary["jobId"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary carries a jobId, got {summary}"))
                .to_string()
        })
        .collect()
}

/// Every page of a listing of `workspace`, walked with `page_size` when given and
/// with no `maxResults` otherwise: the ids in order, and how many pages it took.
async fn walk(
    router: &MultiServiceRouter,
    workspace: &str,
    page_size: Option<i64>,
    expected_total: usize,
) -> (Vec<String>, usize) {
    let case = match page_size {
        Some(size) => format!("maxResults={size}"),
        None => "the default page size".to_string(),
    };
    let mut collected: Vec<String> = Vec::new();
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
        let body = listed(router, workspace, &query.join("&")).await;
        let page = job_ids(&body);
        pages += 1;
        if let Some(size) = page_size {
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: page {pages} holds {} summaries, more than asked: {body}",
                page.len()
            );
        }
        collected.extend(page);
        match body["nextToken"].as_str() {
            Some(next) => {
                // A page followed by a token has to leave something to resume from.
                assert!(
                    collected.len() < expected_total,
                    "case {case:?}: page {pages} ends the listing yet carries {next}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
        assert!(
            pages <= expected_total + 2,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
    }
    (collected, pages)
}

/// The three registered workspaces the suites work in.
async fn fixture(router: &MultiServiceRouter) {
    for name in [WORKSPACE, OTHER_WORKSPACE, EMPTY_WORKSPACE] {
        create_workspace(router, name).await;
    }
}

#[tokio::test]
async fn a_listing_reports_the_jobs_of_the_workspace_it_names() {
    let router = router();
    fixture(&router).await;
    let dataset_id = create_dataset(&router, "Export source").await;

    let here: Vec<String> = start_job_cases(&router, WORKSPACE, &dataset_id)
        .await
        .into_iter()
        .map(|(job_id, _, _)| job_id)
        .collect();
    // Jobs in another workspace of the fixture, and jobs in a workspace opened
    // only when its first job was started: a listing that keyed on nothing but the
    // job id would report these too.
    let there: Vec<String> = start_job_cases(&router, OTHER_WORKSPACE, &dataset_id)
        .await
        .into_iter()
        .map(|(job_id, _, _)| job_id)
        .collect();
    let late: Vec<String> = start_job_cases(&router, LATE_OPENED_WORKSPACE, &dataset_id)
        .await
        .into_iter()
        .map(|(job_id, _, _)| job_id)
        .collect();

    for (case, workspace, expected) in [
        ("the workspace the jobs were started in", WORKSPACE, &here),
        (
            "another workspace with jobs of its own",
            OTHER_WORKSPACE,
            &there,
        ),
        (
            "a workspace opened only by its first job",
            LATE_OPENED_WORKSPACE,
            &late,
        ),
    ] {
        let body = listed(&router, workspace, "maxResults=250").await;
        let reported: BTreeSet<String> = job_ids(&body).into_iter().collect();
        let wanted: BTreeSet<String> = expected.iter().cloned().collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: a listing reports exactly that workspace's jobs, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: a listing that reported everything carries no nextToken, got {body}"
        );
    }

    // A held workspace nobody started a job in is an EMPTY page — absence of jobs is
    // not absence of the workspace.
    let empty = listed(&router, EMPTY_WORKSPACE, "").await;
    assert!(
        summaries(&empty).is_empty() && empty["nextToken"].is_null(),
        "a workspace with no job answers an empty page, got {empty}"
    );

    // A workspace this service holds no record of, in every form of a name neither a
    // `CreateWorkspace` nor a job carries — including ones differing from a held name
    // only in case, since a workspace is named exactly. The workspace the PATH names
    // is resolved before the filter and the paging arguments are read, so a request
    // that is wrong in both ways still reports the resource that is absent.
    for name in ["Plant9", "plant1", "PLANT1", "Plant1-copy"] {
        for query in [
            "",
            "maxResults=1",
            "nextToken=not-hex",
            "filter=COMPLETED",
            "maxResults=250",
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
async fn a_summary_is_the_stored_job_a_read_of_that_job_reports() {
    let router = router();
    fixture(&router).await;
    let dataset_id = create_dataset(&router, "Export source").await;
    let started = start_job_cases(&router, WORKSPACE, &dataset_id).await;

    // (case, how long to wait before the listing, the status then due, whether
    // `completedAt` is reported by then) — the lifecycle every job walks, observed
    // through the listing alone.
    let steps: [(&str, Duration, &str, bool); 3] = [
        (
            "straight after the create",
            Duration::ZERO,
            "SUBMITTED",
            false,
        ),
        (
            "once processing is due",
            EXPORT_JOB_STEP + SLACK,
            "RUNNING",
            false,
        ),
        (
            "once completion is due",
            EXPORT_JOB_STEP + SLACK,
            "COMPLETED",
            true,
        ),
    ];

    for (label, wait, expected_status, completed) in steps {
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let body = listed(&router, WORKSPACE, "maxResults=250").await;
        assert_eq!(
            summaries(&body).len(),
            started.len(),
            "case {label:?}: every started job is listed once, got {body}"
        );

        // A listing REPORTS: taking it again answers the same thing, so nothing the
        // first one did moved a job on.
        let again = listed(&router, WORKSPACE, "maxResults=250").await;
        assert_eq!(
            job_ids(&again),
            job_ids(&body),
            "case {label:?}: two listings of one store must report the same jobs in the same \
             order, got {again} after {body}"
        );

        // The listing is oldest first, which is the order that makes `nextToken`
        // resumable: the instants it reports never decrease along the page.
        let mut previous = f64::MIN;
        for summary in summaries(&body) {
            let started_at = summary["startedAt"].as_f64().unwrap_or(f64::NAN);
            assert!(
                started_at >= previous,
                "case {label:?}: a listing is ordered by startedAt, {started_at} follows \
                 {previous} in {body}"
            );
            previous = started_at;
        }

        for (job_id, job_label, destination) in &started {
            let summary = summaries(&body)
                .iter()
                .find(|summary| summary["jobId"].as_str() == Some(job_id.as_str()))
                .unwrap_or_else(|| panic!("case {label:?}: {job_label} is not listed in {body}"));
            assert_eq!(
                summary["status"].as_str(),
                Some(expected_status),
                "case {label:?} ({job_label}): the status the clock has made due is \
                 {expected_status}, got {summary}"
            );
            assert_eq!(
                summary["destinationS3Uri"].as_str(),
                Some(*destination),
                "case {label:?} ({job_label}): the summary reports the destination the create \
                 named, got {summary}"
            );
            assert_eq!(
                summary["completedAt"].is_null(),
                !completed,
                "case {label:?} ({job_label}): completedAt is reported only once the terminal \
                 status falls due, got {summary}"
            );

            // The same job, read on its own: a summary and a read of one stored job
            // cannot disagree about any member they share.
            let (status, read) = call(&router, "GET", &job_uri(WORKSPACE, job_id), None).await;
            assert_eq!(
                status, 200,
                "case {label:?} ({job_label}): describe got {status} {read}"
            );
            for member in [
                "jobId",
                "status",
                "startedAt",
                "completedAt",
                "destinationS3Uri",
            ] {
                assert_eq!(
                    summary[member], read[member],
                    "case {label:?} ({job_label}): the summary's {member} {} disagrees with the \
                     read's {}",
                    summary[member], read[member]
                );
            }
        }
    }
}

#[tokio::test]
async fn the_filter_reports_only_the_jobs_in_the_status_it_names() {
    let router = router();
    fixture(&router).await;
    let dataset_id = create_dataset(&router, "Export source").await;

    // One batch left to run to its terminal status, then a second batch started: the
    // store then holds jobs in two different statuses at once, so a filter that
    // ignored the status would be caught.
    let settled: BTreeSet<String> = start_job_cases(&router, WORKSPACE, &dataset_id)
        .await
        .into_iter()
        .map(|(job_id, _, _)| job_id)
        .collect();
    tokio::time::sleep(2 * EXPORT_JOB_STEP + SLACK).await;
    let fresh: BTreeSet<String> = start_job_cases(&router, WORKSPACE, &dataset_id)
        .await
        .into_iter()
        .map(|(job_id, _, _)| job_id)
        .collect();
    let every: BTreeSet<String> = settled.union(&fresh).cloned().collect();
    let none: BTreeSet<String> = BTreeSet::new();

    // Rows: every value of the filter enum, at an instant when the first batch is
    // terminal and the second has only just been submitted. `COMPLETED_WITH_ERRORS`
    // and `FAILED` are statuses no job of this emulator reaches, so they report
    // nothing.
    let cases: [(&str, &BTreeSet<String>); 6] = [
        ("ALL", &every),
        ("SUBMITTED", &fresh),
        ("RUNNING", &none),
        ("COMPLETED", &settled),
        ("COMPLETED_WITH_ERRORS", &none),
        ("FAILED", &none),
    ];
    assert_eq!(
        cases.map(|(filter, _)| filter),
        FILTER_VALUES,
        "every value of the filter enum has a row"
    );
    for (filter, expected) in cases {
        let body = listed(&router, WORKSPACE, &format!("filter={filter}&maxResults=250")).await;
        let reported: BTreeSet<String> = job_ids(&body).into_iter().collect();
        assert_eq!(
            &reported, expected,
            "case filter={filter}: unexpected jobs in {body}"
        );
        for summary in summaries(&body) {
            if filter != "ALL" {
                assert_eq!(
                    summary["status"].as_str(),
                    Some(filter),
                    "case filter={filter}: a filtered page reports only that status, got {summary}"
                );
            }
        }
    }

    // "Defaults to ALL": a request that names no filter reports what `ALL` does.
    let unfiltered: BTreeSet<String> = job_ids(&listed(&router, WORKSPACE, "maxResults=250").await)
        .into_iter()
        .collect();
    assert_eq!(
        unfiltered, every,
        "a listing with no filter reports every job, as ALL does"
    );

    // The filter follows the clock, not the call: once the second batch is processing
    // and nothing else has happened, it is `RUNNING` that reports it.
    tokio::time::sleep(EXPORT_JOB_STEP + SLACK).await;
    let running: BTreeSet<String> =
        job_ids(&listed(&router, WORKSPACE, "filter=RUNNING&maxResults=250").await)
            .into_iter()
            .collect();
    assert_eq!(
        running, fresh,
        "case filter=RUNNING: the jobs the clock has started processing"
    );
    let completed: BTreeSet<String> =
        job_ids(&listed(&router, WORKSPACE, "filter=COMPLETED&maxResults=250").await)
            .into_iter()
            .collect();
    assert_eq!(
        completed, settled,
        "case filter=COMPLETED: COMPLETED is terminal, so the first batch stays there"
    );
}

#[tokio::test]
async fn pagination_walks_one_stable_enumeration_whatever_the_page_size() {
    let router = router();
    fixture(&router).await;
    let dataset_id = create_dataset(&router, "Export source").await;
    let started = start_job_cases(&router, WORKSPACE, &dataset_id).await;
    let total = started.len();
    assert!(
        total < DEFAULT_MAX_RESULTS,
        "the fixture has to be shorter than the default page size, got {total}"
    );

    // The whole listing in one page is the order every paged walk has to reproduce.
    let baseline = job_ids(&listed(&router, WORKSPACE, "maxResults=250").await);
    assert_eq!(
        baseline.len(),
        total,
        "the unpaged listing holds every started job"
    );

    // Rows: a page smaller than the listing, sizes that divide it and sizes that do
    // not, exactly the listing, more than it, the published maximum, and no
    // `maxResults` at all.
    let cases: [Option<i64>; 7] = [
        Some(1),
        Some(2),
        Some(3),
        Some(total as i64),
        Some(total as i64 + 1),
        Some(MAX_RESULTS_MAX),
        None,
    ];
    for page_size in cases {
        let case = format!("maxResults={page_size:?}");
        let (walked, pages) = walk(&router, WORKSPACE, page_size, total).await;
        assert_eq!(
            walked, baseline,
            "case {case:?}: the pages joined must be the whole listing once, in one order, \
             got {walked:?}"
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
        Some(hex_token(&baseline[1]).as_str()),
        "the first page's token resumes at the second job, got {first}"
    );

    // A token this listing never issued is refused rather than answered with an empty
    // page: unhexable text, hex of an id no job of this workspace carries, and hex of
    // the id of a job of ANOTHER workspace.
    let elsewhere = start_job(
        &router,
        OTHER_WORKSPACE,
        "s3://sitewise-exports/clips/elsewhere/",
        json!({ "timeseries": [{ "timeSeriesId": DISASSOCIATED_SERIES }] }),
    )
    .await;
    for (case, token) in [
        ("not a token this service issues", "not-hex".to_string()),
        (
            "hex of an unknown id",
            hex_token("99999999-9999-9999-9999-999999999999"),
        ),
        ("hex of another workspace's job", hex_token(&elsewhere)),
    ] {
        let (status, body) = list_raw(&router, WORKSPACE, &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {token}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // `maxResults` carries @range 1..250 (smithy_query query="operation"
    // operation="ListDatasetExportJobs"): each published bound is served and one past
    // it on either side is refused.
    for (case, max_results, served) in [
        ("the range minimum", MAX_RESULTS_MIN, true),
        ("the range maximum", MAX_RESULTS_MAX, true),
        ("one under the minimum", MAX_RESULTS_MIN - 1, false),
        ("one past the maximum", MAX_RESULTS_MAX + 1, false),
    ] {
        let (status, body) =
            list_raw(&router, WORKSPACE, &format!("maxResults={max_results}")).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?} (maxResults={max_results}): an in-range page size must be served, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (maxResults={max_results}): an out-of-range page size must be \
                 refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?} (maxResults={max_results}): unexpected error code in {body}"
            );
        }
    }
}
