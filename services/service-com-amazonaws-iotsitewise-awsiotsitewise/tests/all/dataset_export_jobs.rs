// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise dataset export job slice
//! (`CreateDatasetExportJob` -> `DescribeDatasetExportJob`), driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a created job is PERSISTED and reads back exactly as it was started — its
//!   destination, its error report location and the processing input the create
//!   carried, whatever shape that input took;
//! * a job's `status` is owed by the CLOCK, not by the read: a job reports
//!   `SUBMITTED` straight after the create, then `RUNNING`, then the terminal
//!   `COMPLETED`, and reports no `completedAt` until that terminal status falls
//!   due — so a read never moves a job on and two reads of one job agree;
//! * a job is addressed by `(workspaceName, jobId)`, so a job of one workspace is
//!   not reachable through another, and a job no workspace holds is the modelled
//!   not-found refusal;
//! * a create whose input breaks a rule the input shapes state is the modelled
//!   `InvalidRequestException` and starts NO job, and one naming a dataset the
//!   store does not hold is the modelled not-found refusal;
//! * a retried create carrying the same `clientToken` replays the job the first
//!   one started rather than starting a second.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) is the one
//! the asset suite already has; this suite reuses it rather than growing a second.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, EXPORT_JOB_STEP};

use super::asset_lifecycle::{call, error_code, is_modelled_id, router, router_with_engine};

/// The workspace every suite below works in unless it is about workspace scoping.
const WORKSPACE: &str = "plant-floor";

/// `POST /workspaces/{workspaceName}/dataset-export-jobs`, as `OPERATIONS.md`
/// reports the route.
fn jobs_uri(workspace: &str) -> String {
    format!("/workspaces/{workspace}/dataset-export-jobs")
}

/// `GET /workspaces/{workspaceName}/dataset-export-jobs/{jobId}`.
fn job_uri(workspace: &str, job_id: &str) -> String {
    format!("/workspaces/{workspace}/dataset-export-jobs/{job_id}")
}

/// An S3 URI of the form `S3Uri`'s pattern admits
/// (`^s3://[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]/.+$`).
const DESTINATION: &str = "s3://sitewise-exports/clips/";

/// The same member's error-report form, so a response reporting one in place of
/// the other cannot pass.
const ERROR_REPORT: &str = "s3://sitewise-exports/errors/";

/// The `DatasetExportJobStatus` values, from the enum itself (smithy_query
/// query="shape" shape="DatasetExportJobStatus").
const JOB_STATUSES: [&str; 5] = [
    "SUBMITTED",
    "RUNNING",
    "COMPLETED",
    "COMPLETED_WITH_ERRORS",
    "FAILED",
];

/// The `ExportDataType` values a `DatasetItem` may name a subset of (smithy_query
/// query="shape" shape="ExportDataType").
const EXPORT_DATA_TYPES: [&str; 3] = ["VIDEO", "TELEMETRY", "ANNOTATION"];

/// How long after a step falls due a read is taken, so a run whose scheduler is
/// late cannot observe the previous status.
const SLACK: Duration = Duration::from_millis(250);

/// A `TimeSeriesId` of the associated form, which this service builds as
/// `<assetId>.<propertyId>` — the longest the modelled length (36..73) admits.
const ASSOCIATED_SERIES: &str =
    "11111111-2222-3333-4444-555555555555.66666666-7777-8888-9999-aaaaaaaaaaaa";

/// A `TimeSeriesId` of the shortest form that length admits: a single id, which
/// is how this service names a stream no asset property owns.
const DISASSOCIATED_SERIES: &str = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";

/// A `TimeInNanos`, as the two members the shape declares.
fn nanos(seconds: i64, offset: i64) -> Value {
    json!({ "timeInSeconds": seconds, "offsetInNanos": offset })
}

/// A `trimSettings` spanning `[start, end)`.
fn trim(start: Value, end: Value) -> Value {
    json!({ "startTime": start, "endTime": end })
}

/// `CreateDatasetExportJob` with `body`.
async fn create(router: &MultiServiceRouter, workspace: &str, body: Value) -> (u16, Value) {
    call(router, "POST", &jobs_uri(workspace), Some(body)).await
}

/// `DescribeDatasetExportJob`.
async fn describe(router: &MultiServiceRouter, workspace: &str, job_id: &str) -> (u16, Value) {
    call(router, "GET", &job_uri(workspace, job_id), None).await
}

/// The body a create sends: the destination, the error report location, the
/// processing input, and `clientToken` only when the row carries one.
fn create_body(input: &Value, token: Option<&str>) -> Value {
    let mut body = json!({
        "destinationS3Uri": DESTINATION,
        "errorReportLocation": { "s3Uri": ERROR_REPORT },
        "input": input.clone(),
    });
    if let Some(token) = token {
        body.as_object_mut()
            .expect("an object body")
            .insert("clientToken".to_string(), json!(token));
    }
    body
}

/// Create a dataset through `CreateDataset` and answer its id, so the rows whose
/// `ProcessingInput` names a dataset name one the store really holds.
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

/// Start a job with `input` and answer its id.
async fn start_job(router: &MultiServiceRouter, workspace: &str, input: &Value) -> String {
    let (status, body) = create(router, workspace, create_body(input, None)).await;
    assert_eq!(status, 202, "the create got {status} {body}");
    body["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("no jobId in {body}"))
        .to_string()
}

/// One row of the create-and-read-back table: the processing input a create
/// sends, built from the id of the dataset this suite created.
struct InputCase {
    label: &'static str,
    input: fn(&str) -> Value,
    client_token: Option<&'static str>,
}

/// The rows: `ProcessingInput` spanning both members the union declares — a
/// `timeseries` list of one and of several items, identified by `timeSeriesId` of
/// each length the type admits and by `propertyAlias`, with and without
/// `trimSettings` and `formatSettings`, and a `dataset` item with no options, with
/// a trim range, and with one and with every `ExportDataType` — crossed with a
/// present / absent `clientToken`.
fn input_cases() -> Vec<InputCase> {
    vec![
        InputCase {
            label: "one-timeseries-by-the-longest-id",
            input: |_| json!({ "timeseries": [{ "timeSeriesId": ASSOCIATED_SERIES }] }),
            client_token: None,
        },
        InputCase {
            label: "one-timeseries-by-the-shortest-id",
            input: |_| json!({ "timeseries": [{ "timeSeriesId": DISASSOCIATED_SERIES }] }),
            client_token: Some("11111111-1111-1111-1111-111111111111"),
        },
        InputCase {
            label: "one-timeseries-by-alias-trimmed-and-formatted",
            input: |_| {
                json!({
                    "timeseries": [{
                        "propertyAlias": "/company/windfarm/3/turbine/7/temperature",
                        "trimSettings": trim(nanos(1_760_000_000, 0), nanos(1_760_003_600, 0)),
                        "formatSettings": {
                            "framesPerSecond": 30,
                            "widthInPixels": 1_920,
                            "heightInPixels": 1_080,
                        },
                    }]
                })
            },
            client_token: None,
        },
        InputCase {
            label: "several-timeseries-mixing-ids-and-aliases",
            input: |_| {
                json!({
                    "timeseries": [
                        { "timeSeriesId": ASSOCIATED_SERIES },
                        {
                            "propertyAlias": "/company/windfarm/3/turbine/7/vibration",
                            "trimSettings": trim(
                                nanos(1_760_000_000, 1),
                                nanos(1_760_000_000, 2),
                            ),
                        },
                        {
                            "timeSeriesId": DISASSOCIATED_SERIES,
                            "formatSettings": { "framesPerSecond": 1 },
                        },
                    ]
                })
            },
            client_token: Some("22222222-2222-2222-2222-222222222222"),
        },
        InputCase {
            label: "dataset-with-no-options",
            input: |dataset_id| json!({ "dataset": { "datasetId": dataset_id } }),
            client_token: None,
        },
        InputCase {
            label: "dataset-trimmed-to-one-data-type",
            input: |dataset_id| {
                json!({
                    "dataset": {
                        "datasetId": dataset_id,
                        "trimSettings": trim(nanos(1_700_000_000, 0), nanos(1_700_000_001, 0)),
                        "exportDataTypes": [EXPORT_DATA_TYPES[1]],
                    }
                })
            },
            client_token: Some("33333333-3333-3333-3333-333333333333"),
        },
        InputCase {
            label: "dataset-with-every-data-type",
            input: |dataset_id| {
                json!({
                    "dataset": {
                        "datasetId": dataset_id,
                        "exportDataTypes": EXPORT_DATA_TYPES,
                    }
                })
            },
            client_token: None,
        },
    ]
}

#[tokio::test]
async fn a_started_job_reads_back_through_describe_for_every_processing_input_shape() {
    let router = router();
    let dataset_id = create_dataset(&router, "Export source").await;

    for case in input_cases() {
        let label = case.label;
        let input = (case.input)(&dataset_id);
        let (status, created) =
            create(&router, WORKSPACE, create_body(&input, case.client_token)).await;
        assert_eq!(
            status, 202,
            "case {label:?}: expected 202, got {status} {created}"
        );
        let job_id = created["jobId"].as_str().unwrap_or("");
        assert!(
            is_modelled_id(job_id),
            "case {label:?}: jobId must have the modelled form, got {created}"
        );
        assert_eq!(
            created["workspaceName"].as_str().unwrap_or(""),
            WORKSPACE,
            "case {label:?}: unexpected workspaceName in {created}"
        );

        let (status, described) = describe(&router, WORKSPACE, job_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeDatasetExportJob expected 200, got {status} {described}"
        );
        assert_eq!(
            described["jobId"].as_str().unwrap_or(""),
            job_id,
            "case {label:?}: a read must report the job it was asked for, got {described}"
        );
        assert_eq!(
            described["workspaceName"].as_str().unwrap_or(""),
            WORKSPACE,
            "case {label:?}: unexpected workspaceName in {described}"
        );
        assert_eq!(
            described["destinationS3Uri"].as_str().unwrap_or(""),
            DESTINATION,
            "case {label:?}: unexpected destinationS3Uri in {described}"
        );
        assert_eq!(
            described["errorReportLocation"],
            json!({ "s3Uri": ERROR_REPORT }),
            "case {label:?}: unexpected errorReportLocation in {described}"
        );
        assert_eq!(
            described["input"], input,
            "case {label:?}: a read must report the processing input the create carried, \
             got {described}"
        );
        assert_eq!(
            described["status"].as_str().unwrap_or(""),
            JOB_STATUSES[0],
            "case {label:?}: a job reports {:?} until the clock starts it, got {described}",
            JOB_STATUSES[0]
        );
        assert!(
            described.get("completedAt").is_none(),
            "case {label:?}: a job that has not completed reports no completedAt, got {described}"
        );
        assert!(
            described["startedAt"].as_f64().unwrap_or(0.0) > 0.0,
            "case {label:?}: startedAt must be a real instant, got {described}"
        );
    }
}

#[tokio::test]
async fn the_status_a_job_reports_is_the_one_the_clock_has_made_due() {
    let router = router();
    let input = json!({ "timeseries": [{ "timeSeriesId": ASSOCIATED_SERIES }] });
    let job_id = start_job(&router, WORKSPACE, &input).await;

    // (case, how long to wait before the read, the status then due, whether
    // `completedAt` is reported by then) — the lifecycle every job walks, read
    // through the API alone.
    let steps: [(&str, Duration, &str, bool); 3] = [
        ("straight after the create", Duration::ZERO, "SUBMITTED", false),
        ("once processing is due", EXPORT_JOB_STEP + SLACK, "RUNNING", false),
        ("once completion is due", EXPORT_JOB_STEP + SLACK, "COMPLETED", true),
    ];

    let mut started_at = None;
    for (label, wait, expected, completed) in steps {
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let (status, body) = describe(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["status"].as_str().unwrap_or(""),
            expected,
            "case {label:?}: expected status {expected:?}, got {body}"
        );

        let reported_start = body["startedAt"].as_f64().unwrap_or(0.0);
        match started_at {
            None => started_at = Some(reported_start),
            Some(first) => assert_eq!(
                reported_start, first,
                "case {label:?}: startedAt must not move as the job runs, got {body}"
            ),
        }

        match (completed, body["completedAt"].as_f64()) {
            (false, reported) => assert!(
                reported.is_none(),
                "case {label:?}: a job still running reports no completedAt, got {body}"
            ),
            (true, Some(completed_at)) => {
                let elapsed = completed_at - reported_start;
                assert!(
                    elapsed >= 2.0 * EXPORT_JOB_STEP.as_secs_f64(),
                    "case {label:?}: completedAt must be when the terminal status fell due, \
                     {elapsed}s after startedAt in {body}"
                );
            }
            (true, None) => panic!("case {label:?}: a completed job reports completedAt, got {body}"),
        }

        // A read is a read: taking it again answers the same thing, so nothing the
        // first read did moved the job on.
        let (status, again) = describe(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the second read got {status} {again}"
        );
        assert_eq!(
            again["status"].as_str().unwrap_or(""),
            expected,
            "case {label:?}: two reads of one job must agree, got {again} after {body}"
        );
    }

    // COMPLETED is terminal: the status stays there however long the job is left.
    tokio::time::sleep(EXPORT_JOB_STEP * 3).await;
    let (status, settled) = describe(&router, WORKSPACE, &job_id).await;
    assert_eq!(status, 200, "the read after the job settled got {status} {settled}");
    assert_eq!(
        settled["status"].as_str().unwrap_or(""),
        "COMPLETED",
        "COMPLETED is terminal, got {settled}"
    );
}

#[tokio::test]
async fn a_job_is_addressed_within_the_workspace_that_holds_it() {
    let router = router();
    let input = json!({ "timeseries": [{ "propertyAlias": "/plant/line-1/rate" }] });

    let here = start_job(&router, WORKSPACE, &input).await;
    let elsewhere = start_job(&router, "other-floor", &input).await;
    assert_ne!(
        here, elsewhere,
        "two creates must start two jobs, got {here} twice"
    );

    // (case, workspace, job, found) — a job resolves in its OWN workspace only,
    // and an id no create ever minted resolves nowhere.
    let unknown = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    let rows: [(&str, &str, &str, bool); 5] = [
        ("its own workspace", WORKSPACE, here.as_str(), true),
        ("the other workspace's own", "other-floor", elsewhere.as_str(), true),
        ("another workspace", "other-floor", here.as_str(), false),
        ("a workspace no job lives in", "empty-floor", here.as_str(), false),
        ("an id no create minted", WORKSPACE, unknown, false),
    ];

    for (label, workspace, job_id, found) in rows {
        let (status, body) = describe(&router, workspace, job_id).await;
        if found {
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status} {body}"
            );
            assert_eq!(
                body["jobId"].as_str().unwrap_or(""),
                job_id,
                "case {label:?}: unexpected jobId in {body}"
            );
            assert_eq!(
                body["workspaceName"].as_str().unwrap_or(""),
                workspace,
                "case {label:?}: unexpected workspaceName in {body}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {label:?}: expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {label:?}: unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_input_that_breaks_a_rule_the_shapes_state_starts_no_job() {
    let (router, engine) = router_with_engine();
    let dataset_id = create_dataset(&router, "Export source").await;

    // (case, input, expected status, expected error code). The rules: a
    // `TimeseriesItem` names "exactly one of timeSeriesId or propertyAlias", a
    // `TrimSettings` end time "must be greater than startTime", and a `DatasetItem`
    // names a dataset this service holds.
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "both identifiers on one item",
            json!({
                "timeseries": [{
                    "timeSeriesId": ASSOCIATED_SERIES,
                    "propertyAlias": "/plant/line-1/rate",
                }]
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "neither identifier on one item",
            json!({ "timeseries": [{ "formatSettings": { "framesPerSecond": 5 } }] }),
            400,
            "InvalidRequestException",
        ),
        (
            "both identifiers on a later item",
            json!({
                "timeseries": [
                    { "timeSeriesId": ASSOCIATED_SERIES },
                    { "timeSeriesId": DISASSOCIATED_SERIES, "propertyAlias": "/plant/line-1/rate" },
                ]
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a timeseries trim range that ends where it starts",
            json!({
                "timeseries": [{
                    "timeSeriesId": ASSOCIATED_SERIES,
                    "trimSettings": trim(nanos(1_760_000_000, 7), nanos(1_760_000_000, 7)),
                }]
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a timeseries trim range that ends a nanosecond before it starts",
            json!({
                "timeseries": [{
                    "propertyAlias": "/plant/line-1/rate",
                    "trimSettings": trim(nanos(1_760_000_000, 7), nanos(1_760_000_000, 6)),
                }]
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a dataset trim range that ends before it starts",
            json!({
                "dataset": {
                    "datasetId": dataset_id,
                    "trimSettings": trim(nanos(1_760_000_100, 0), nanos(1_760_000_000, 0)),
                }
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a dataset no store holds",
            json!({ "dataset": { "datasetId": "ffffffff-ffff-ffff-ffff-ffffffffffff" } }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (label, input, expected_status, expected_code) in rows {
        let (status, body) = create(&router, WORKSPACE, create_body(&input, None)).await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {label:?}: unexpected error code in {body}"
        );

        // A refused create writes NOTHING. The store is the only place that shows
        // it: no reader enumerates a workspace's jobs yet.
        let held = {
            let state = service_state(&*engine).expect("the service store");
            let store = state.lock();
            store.dataset_export_jobs.len()
        };
        assert_eq!(
            held, 0,
            "case {label:?}: a refused create must start no job, the store holds {held}"
        );
    }

    // The same range one nanosecond wider is accepted, so the refusals above are
    // about the rule and not about trimming at all.
    let input = json!({
        "timeseries": [{
            "timeSeriesId": ASSOCIATED_SERIES,
            "trimSettings": trim(nanos(1_760_000_000, 7), nanos(1_760_000_000, 8)),
        }]
    });
    let (status, accepted) = create(&router, WORKSPACE, create_body(&input, None)).await;
    assert_eq!(
        status, 202,
        "the narrowest accepted trim range got {status} {accepted}"
    );
}

#[tokio::test]
async fn a_retried_create_carrying_the_same_client_token_replays_the_first_job() {
    let (router, engine) = router_with_engine();
    let input = json!({ "timeseries": [{ "timeSeriesId": ASSOCIATED_SERIES }] });
    let token = "44444444-4444-4444-4444-444444444444";
    let other_token = "55555555-5555-5555-5555-555555555555";

    let (status, first) = create(&router, WORKSPACE, create_body(&input, Some(token))).await;
    assert_eq!(status, 202, "the first create got {status} {first}");
    let job_id = first["jobId"].as_str().unwrap_or("").to_string();

    // (case, workspace, token, whether the answer must be the first job's id)
    let rows: [(&str, &str, Option<&str>, bool); 4] = [
        ("the same token again", WORKSPACE, Some(token), true),
        ("the same token once more", WORKSPACE, Some(token), true),
        ("another token", WORKSPACE, Some(other_token), false),
        ("no token at all", WORKSPACE, None, false),
    ];

    for (label, workspace, retry_token, replayed) in rows {
        let (status, body) = create(&router, workspace, create_body(&input, retry_token)).await;
        assert_eq!(
            status, 202,
            "case {label:?}: expected 202, got {status} {body}"
        );
        let answered = body["jobId"].as_str().unwrap_or("");
        if replayed {
            assert_eq!(
                answered, job_id,
                "case {label:?}: a retry must replay the job the first create started, got {body}"
            );
        } else {
            assert_ne!(
                answered, job_id,
                "case {label:?}: a create that is not a retry must start a job of its own, \
                 got {body}"
            );
        }
        let (status, described) = describe(&router, workspace, answered).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the answered job must read back, got {status} {described}"
        );
    }

    // Three jobs: the one the token started, and the two creates that were not
    // retries of it.
    let held = {
        let state = service_state(&*engine).expect("the service store");
        let store = state.lock();
        store.dataset_export_jobs.len()
    };
    assert_eq!(
        held, 3,
        "the two replays must have written nothing, the store holds {held}"
    );
}
