// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise bulk import job slice
//! (`CreateBulkImportJob` -> `DescribeBulkImportJob`), driven over HTTP through
//! the same `MultiServiceRouter` the emulator boots with, so every assertion is
//! about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a created job is PERSISTED and reads back exactly as it was started — its
//!   name, its role ARN, its files in the order and the shape the request listed
//!   them, its job configuration, its ingestion flags and its destination;
//! * a job's `jobStatus` is owed by the CLOCK, not by the read: a job reports
//!   `PENDING` straight after the create, then `RUNNING`, then the terminal
//!   `COMPLETED`, and its `jobLastUpdateDate` is the instant the status the read
//!   reports fell due — so a read never moves a job on and two reads agree;
//! * a job is addressed by its `jobId`, and a `workspaceName` only NARROWS the
//!   read: a job of another workspace, and one created for no workspace at all,
//!   are not reachable through it, and a workspace this store does not hold is
//!   the modelled not-found refusal;
//! * the two members that name a session dataset are one destination, so a
//!   create naming one without the other is the modelled
//!   `InvalidRequestException` and starts NO job, and one naming a workspace, or
//!   a dataset that workspace does not contain, is the modelled not-found
//!   refusal;
//! * a job ingests into a SESSION dataset, the kind `datasetId` documents, so a
//!   destination dataset whose stored kind is present and is another one is the
//!   modelled `InvalidRequestException` and starts NO job, while one the store
//!   holds no kind for is accepted;
//! * a create that ingests into time series OUTSIDE a workspace needs the
//!   account's warm or cold tier enabled, while one that ingests into a session
//!   dataset does not;
//! * a job names at most the documented number of files, past which the create
//!   is the modelled quota refusal and starts no job.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) is the one
//! the asset suite already has, the workspace fixture is the search suite's and
//! the storage-configuration put is that suite's; this suite reuses all three
//! rather than growing copies.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, BULK_IMPORT_JOB_STEP, MAX_FILES_PER_BULK_IMPORT_JOB,
    STORAGE_STATE_DISABLED, STORAGE_STATE_ENABLED, STORAGE_TYPE_MULTI_LAYER,
    STORAGE_TYPE_SITEWISE_DEFAULT,
};

use super::asset_lifecycle::{call, error_code, is_modelled_id, router, router_with_engine};
use super::search_lifecycle::create_workspace;
use super::storage_configuration::{multi_layer_storage, put as put_storage};

/// The workspace a job that ingests into a session dataset is created in.
const WORKSPACE: &str = "plant-floor";

/// A second workspace, so a read scoped to the wrong one can be shown not to
/// resolve.
const OTHER_WORKSPACE: &str = "cold-store";

/// `POST /jobs`, as `OPERATIONS.md` reports the route.
const JOBS_URI: &str = "/jobs";

/// The `jobRoleArn` every create names: an ARN of the form the modelled `ARN`
/// pattern admits (`^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`).
const JOB_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SiteWiseBulkImport";

/// The `ColumnName` values a `Csv` may list. Source:
/// `smithy_query(query="shape", shape="ColumnName")` — written out here as an
/// oracle independent of the crate under test.
const COLUMN_NAMES: [&str; 8] = [
    "ALIAS",
    "ASSET_ID",
    "PROPERTY_ID",
    "DATA_TYPE",
    "TIMESTAMP_SECONDS",
    "TIMESTAMP_NANO_OFFSET",
    "QUALITY",
    "VALUE",
];

/// How long after a step falls due a read is taken, so a run whose scheduler is
/// late cannot observe the previous status.
const SLACK: Duration = Duration::from_millis(250);

/// `GET /jobs/{jobId}`, with the optional `workspaceName` query parameter when
/// the read is scoped to one.
fn job_uri(job_id: &str, workspace: Option<&str>) -> String {
    match workspace {
        None => format!("/jobs/{job_id}"),
        Some(workspace) => format!("/jobs/{job_id}?workspaceName={workspace}"),
    }
}

/// `CreateBulkImportJob` with `body`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", JOBS_URI, Some(body)).await
}

/// `DescribeBulkImportJob`, scoped to `workspace` when the row names one.
async fn describe(
    router: &MultiServiceRouter,
    job_id: &str,
    workspace: Option<&str>,
) -> (u16, Value) {
    call(router, "GET", &job_uri(job_id, workspace), None).await
}

/// How many jobs the store holds, read straight off the store so that a refused
/// create can be shown to have written nothing without depending on the scope or
/// the filter any reader of the API applies.
fn jobs_held(engine: &engine::InMemoryEngine) -> usize {
    let state = service_state(engine).expect("the service store");
    let store = state.lock();
    store.bulk_import_jobs.len()
}

/// One `File`: the bucket and key it is in, plus whichever of the optional
/// members the row varies.
fn file(bucket: &str, key: &str, extras: Value) -> Value {
    let mut value = json!({ "bucket": bucket, "key": key });
    let object = value.as_object_mut().expect("an object file");
    for (member, member_value) in extras.as_object().cloned().unwrap_or_default() {
        object.insert(member, member_value);
    }
    value
}

/// A `TimeInNanos`, as the two members the shape declares.
fn nanos(seconds: i64, offset: i64) -> Value {
    json!({ "timeInSeconds": seconds, "offsetInNanos": offset })
}

/// Turn the account's warm tier on, which is one of the two preconditions a job
/// ingesting into time series outside a workspace has.
async fn enable_warm_tier(router: &MultiServiceRouter) {
    let (status, body) = put_storage(
        router,
        json!({
            "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
            "warmTier": STORAGE_STATE_ENABLED,
        }),
    )
    .await;
    assert_eq!(status, 200, "PutStorageConfiguration got {status} {body}");
}

/// Create a dataset IN `workspace` through `CreateDataset` and answer its id, so
/// a row whose destination is a session dataset names one the store really holds
/// in the workspace the job names.
async fn create_dataset(router: &MultiServiceRouter, name: &str, workspace: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/datasets",
        Some(json!({
            "datasetName": name,
            "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
            "workspaceName": workspace,
        })),
    )
    .await;
    assert_eq!(status, 202, "create dataset {name}: got {status} {body}");
    body["datasetId"]
        .as_str()
        .unwrap_or_else(|| panic!("create dataset {name}: no datasetId in {body}"))
        .to_string()
}

/// Where a job ingests: into a session dataset of a workspace, or into time
/// series outside one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    /// `workspaceName` and `datasetId` together.
    Dataset,
    /// Neither, which is the form the account's tiers are a precondition for.
    Tiered,
}

/// One row of the create-and-read-back table.
struct Case {
    label: &'static str,
    job_name: &'static str,
    /// The `files` the create lists, built so a row can vary their count, their
    /// optional members and the file format each one carries.
    files: fn() -> Value,
    /// The `jobConfiguration` the create carries, if the row names one.
    job_configuration: Option<Value>,
    adaptive_ingestion: Option<bool>,
    delete_files_after_import: Option<bool>,
    destination: Destination,
}

/// The rows: `files` spanning one and several entries, each optional member of a
/// `File` (`versionId`, `alias`, `startTime`, `fileFormat`) present and absent,
/// every member the `FileFormat` union declares (source:
/// `smithy_query(query="shape", shape="FileFormat")` — csv | parquet | mp4 |
/// annotation), a `jobConfiguration` present and absent, both ingestion flags at
/// each value and absent, and both destinations a job may have.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one-plain-file-into-the-tiers",
            job_name: "Historical import",
            files: || json!([file("sitewise-import", "turbines/2026/01.csv", json!({}))]),
            job_configuration: None,
            adaptive_ingestion: None,
            delete_files_after_import: None,
            destination: Destination::Tiered,
        },
        Case {
            label: "one-csv-file-with-every-column",
            job_name: "csv_import:2026/01",
            files: || {
                json!([file(
                    "sitewise-import",
                    "turbines/2026/01.csv",
                    json!({
                        "versionId": "3HL4kqtJlcpXroDTDmjVBH40Nrjfkd",
                        "alias": "/company/windfarm/3/turbine/7/temperature",
                        "startTime": nanos(1_760_000_000, 250),
                        "fileFormat": { "csv": { "columnNames": COLUMN_NAMES } },
                    })
                )])
            },
            job_configuration: Some(json!({
                "fileFormat": { "csv": { "columnNames": [COLUMN_NAMES[1], COLUMN_NAMES[7]] } }
            })),
            adaptive_ingestion: Some(true),
            delete_files_after_import: Some(false),
            destination: Destination::Tiered,
        },
        Case {
            label: "several-files-mixing-every-file-format",
            job_name: "Mixed formats",
            files: || {
                json!([
                    file(
                        "sitewise-import",
                        "turbines/parquet/part-0",
                        json!({ "fileFormat": { "parquet": {} } })
                    ),
                    file(
                        "sitewise-import",
                        "turbines/video/clip.mp4",
                        json!({
                            "fileFormat": { "mp4": {} },
                            "startTime": nanos(1_700_000_001, 0),
                        })
                    ),
                    file(
                        "sitewise-import",
                        "turbines/annotations/labels.json",
                        json!({ "fileFormat": { "annotation": {} } })
                    ),
                    file(
                        "sitewise-import",
                        "turbines/2026/02.csv",
                        json!({ "fileFormat": { "csv": { "columnNames": [COLUMN_NAMES[0]] } } })
                    ),
                ])
            },
            job_configuration: Some(json!({ "fileFormat": { "parquet": {} } })),
            adaptive_ingestion: Some(false),
            delete_files_after_import: Some(true),
            destination: Destination::Tiered,
        },
        Case {
            label: "into-a-session-dataset",
            job_name: "Session import",
            files: || {
                json!([file(
                    "sw-import",
                    "sessions/2026/02.csv",
                    json!({ "fileFormat": { "csv": { "columnNames": COLUMN_NAMES } } })
                )])
            },
            job_configuration: None,
            adaptive_ingestion: Some(true),
            delete_files_after_import: None,
            destination: Destination::Dataset,
        },
        Case {
            label: "the-largest-file-list-the-quota-admits",
            job_name: "Bulk of a hundred files",
            files: || {
                Value::Array(
                    (0..MAX_FILES_PER_BULK_IMPORT_JOB)
                        .map(|at| {
                            file(
                                "sitewise-import",
                                &format!("turbines/part-{at}.csv"),
                                json!({}),
                            )
                        })
                        .collect(),
                )
            },
            job_configuration: None,
            adaptive_ingestion: None,
            delete_files_after_import: None,
            destination: Destination::Tiered,
        },
    ]
}

/// The body a create sends for `case`, with the destination's members filled in
/// from the dataset this suite created.
fn body_of(case: &Case, dataset_id: &str) -> Value {
    let mut body = json!({
        "jobName": case.job_name,
        "jobRoleArn": JOB_ROLE_ARN,
        "files": (case.files)(),
        "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some(configuration) = case.job_configuration.clone() {
        object.insert("jobConfiguration".to_string(), configuration);
    }
    if let Some(adaptive) = case.adaptive_ingestion {
        object.insert("adaptiveIngestion".to_string(), json!(adaptive));
    }
    if let Some(delete) = case.delete_files_after_import {
        object.insert("deleteFilesAfterImport".to_string(), json!(delete));
    }
    if case.destination == Destination::Dataset {
        object.insert("workspaceName".to_string(), json!(WORKSPACE));
        object.insert("datasetId".to_string(), json!(dataset_id));
    }
    body
}

#[tokio::test]
async fn a_created_job_reads_back_through_describe_for_every_request_shape() {
    let router = router();
    enable_warm_tier(&router).await;
    create_workspace(&router, WORKSPACE).await;
    let dataset_id = create_dataset(&router, "Import target", WORKSPACE).await;

    for case in cases() {
        let label = case.label;
        let body = body_of(&case, &dataset_id);
        let (status, created) = create(&router, body.clone()).await;
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
            created["jobName"].as_str().unwrap_or(""),
            case.job_name,
            "case {label:?}: a create answers the name it was given, got {created}"
        );
        assert_eq!(
            created["jobStatus"].as_str().unwrap_or(""),
            "PENDING",
            "case {label:?}: a job is PENDING until the clock starts it, got {created}"
        );

        // The scope a dataset job is read through is its own workspace; a job
        // ingesting into the tiers is read account-wide.
        let scope = match case.destination {
            Destination::Dataset => Some(WORKSPACE),
            Destination::Tiered => None,
        };
        let (status, described) = describe(&router, job_id, scope).await;
        assert_eq!(
            status, 200,
            "case {label:?}: DescribeBulkImportJob expected 200, got {status} {described}"
        );
        assert_eq!(
            described["jobId"].as_str().unwrap_or(""),
            job_id,
            "case {label:?}: a read must report the job it was asked for, got {described}"
        );
        assert_eq!(
            described["jobName"].as_str().unwrap_or(""),
            case.job_name,
            "case {label:?}: unexpected jobName in {described}"
        );
        assert_eq!(
            described["jobRoleArn"].as_str().unwrap_or(""),
            JOB_ROLE_ARN,
            "case {label:?}: unexpected jobRoleArn in {described}"
        );
        assert_eq!(
            described["files"], body["files"],
            "case {label:?}: a read must report the files the create carried, got {described}"
        );
        assert_eq!(
            described["errorReportLocation"], body["errorReportLocation"],
            "case {label:?}: unexpected errorReportLocation in {described}"
        );
        assert_eq!(
            described["jobConfiguration"], body["jobConfiguration"],
            "case {label:?}: unexpected jobConfiguration in {described}"
        );
        assert_eq!(
            described["adaptiveIngestion"], body["adaptiveIngestion"],
            "case {label:?}: unexpected adaptiveIngestion in {described}"
        );
        assert_eq!(
            described["deleteFilesAfterImport"], body["deleteFilesAfterImport"],
            "case {label:?}: unexpected deleteFilesAfterImport in {described}"
        );
        assert_eq!(
            described["datasetId"], body["datasetId"],
            "case {label:?}: unexpected datasetId in {described}"
        );
        assert_eq!(
            described["workspaceName"], body["workspaceName"],
            "case {label:?}: unexpected workspaceName in {described}"
        );
        assert_eq!(
            described["jobStatus"].as_str().unwrap_or(""),
            "PENDING",
            "case {label:?}: a job reports PENDING until the clock starts it, got {described}"
        );

        let created_at = described["jobCreationDate"].as_f64().unwrap_or(0.0);
        assert!(
            created_at > 0.0,
            "case {label:?}: jobCreationDate must be a real instant, got {described}"
        );
        assert_eq!(
            described["jobLastUpdateDate"].as_f64().unwrap_or(0.0),
            created_at,
            "case {label:?}: nothing has updated a fresh job, so its last update is its \
             creation, got {described}"
        );

        // A job is identified by its id alone, so the unscoped read answers the
        // same job the scoped one did.
        let (status, unscoped) = describe(&router, job_id, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the unscoped read got {status} {unscoped}"
        );
        assert_eq!(
            unscoped["jobId"].as_str().unwrap_or(""),
            job_id,
            "case {label:?}: unexpected jobId in {unscoped}"
        );
    }
}

#[tokio::test]
async fn the_status_a_job_reports_is_the_one_the_clock_has_made_due() {
    let router = router();
    enable_warm_tier(&router).await;
    let (status, created) = create(
        &router,
        json!({
            "jobName": "Clock walk",
            "jobRoleArn": JOB_ROLE_ARN,
            "files": [file("sitewise-import", "turbines/2026/01.csv", json!({}))],
            "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
        }),
    )
    .await;
    assert_eq!(status, 202, "the create got {status} {created}");
    let job_id = created["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("no jobId in {created}"))
        .to_string();

    // (case, how long to wait before the read, the status then due, how many
    // steps of the lifecycle have fallen due by then) — the lifecycle every job
    // walks, read through the API alone.
    let steps: [(&str, Duration, &str, f64); 3] = [
        ("straight after the create", Duration::ZERO, "PENDING", 0.0),
        ("once processing is due", BULK_IMPORT_JOB_STEP + SLACK, "RUNNING", 1.0),
        ("once completion is due", BULK_IMPORT_JOB_STEP + SLACK, "COMPLETED", 2.0),
    ];

    let mut creation_date = None;
    for (label, wait, expected, steps_due) in steps {
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let (status, body) = describe(&router, &job_id, None).await;
        assert_eq!(status, 200, "case {label:?}: expected 200, got {status} {body}");
        assert_eq!(
            body["jobStatus"].as_str().unwrap_or(""),
            expected,
            "case {label:?}: expected jobStatus {expected:?}, got {body}"
        );

        // `jobCreationDate` is the instant the create landed, so it never moves.
        let reported_creation = body["jobCreationDate"].as_f64().unwrap_or(0.0);
        match creation_date {
            None => creation_date = Some(reported_creation),
            Some(first) => assert_eq!(
                reported_creation, first,
                "case {label:?}: jobCreationDate must not move, got {body}"
            ),
        }

        // `jobLastUpdateDate` is the instant the status the read reports fell
        // due: the creation itself while nothing has moved the job, and one step
        // per transition after that.
        let elapsed = body["jobLastUpdateDate"].as_f64().unwrap_or(0.0) - reported_creation;
        let due = steps_due * BULK_IMPORT_JOB_STEP.as_secs_f64();
        assert!(
            (elapsed - due).abs() < 0.001,
            "case {label:?}: jobLastUpdateDate must be {due}s after jobCreationDate, \
             got {elapsed}s in {body}"
        );

        // A read is a read: taking it again answers the same thing, so nothing
        // the first read did moved the job on.
        let (status, again) = describe(&router, &job_id, None).await;
        assert_eq!(status, 200, "case {label:?}: the second read got {status} {again}");
        assert_eq!(
            again, body,
            "case {label:?}: two reads of one job must agree, got {again} after {body}"
        );
    }

    // COMPLETED is terminal: the status stays there however long the job is left.
    tokio::time::sleep(BULK_IMPORT_JOB_STEP * 3).await;
    let (status, settled) = describe(&router, &job_id, None).await;
    assert_eq!(status, 200, "the read after the job settled got {status} {settled}");
    assert_eq!(
        settled["jobStatus"].as_str().unwrap_or(""),
        "COMPLETED",
        "COMPLETED is terminal, got {settled}"
    );
}

#[tokio::test]
async fn a_workspace_scope_only_narrows_the_read() {
    // CAVEAT: the model documents `workspaceName` only as "The name of the
    // workspace", so the NARROWING the rows below pin is this emulator's reading
    // of the member — a scope the caller named has to bear on the answer. If a
    // recording ever shows AWS answering a job addressed by id regardless of the
    // scope, the rows that expect a 404 here change with the handler.
    let router = router();
    enable_warm_tier(&router).await;
    create_workspace(&router, WORKSPACE).await;
    create_workspace(&router, OTHER_WORKSPACE).await;
    let dataset_id = create_dataset(&router, "Import target", WORKSPACE).await;

    let dataset_job = start(
        &router,
        json!({
            "jobName": "Session import",
            "jobRoleArn": JOB_ROLE_ARN,
            "files": [file("sitewise-import", "sessions/01.csv", json!({}))],
            "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
            "workspaceName": WORKSPACE,
            "datasetId": dataset_id,
        }),
    )
    .await;
    let tiered_job = start(
        &router,
        json!({
            "jobName": "Tiered import",
            "jobRoleArn": JOB_ROLE_ARN,
            "files": [file("sitewise-import", "turbines/01.csv", json!({}))],
            "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
        }),
    )
    .await;
    let unminted = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    // (case, the job the read addresses, the scope it names, whether the job
    // answers, the identifier the refusal names).
    let rows: Vec<(&str, &str, Option<&str>, bool, &str)> = vec![
        ("a dataset job read account-wide", &dataset_job, None, true, ""),
        (
            "a dataset job read through its own workspace",
            &dataset_job,
            Some(WORKSPACE),
            true,
            "",
        ),
        (
            "a dataset job read through another workspace",
            &dataset_job,
            Some(OTHER_WORKSPACE),
            false,
            &dataset_job,
        ),
        ("a tiered job read account-wide", &tiered_job, None, true, ""),
        (
            "a tiered job, which belongs to no workspace, read through one",
            &tiered_job,
            Some(WORKSPACE),
            false,
            &tiered_job,
        ),
        (
            "a scope this store holds no workspace for",
            &dataset_job,
            Some("no-such-workspace"),
            false,
            "no-such-workspace",
        ),
        ("an id no create minted", unminted, None, false, unminted),
        (
            "an id no create minted, read through a workspace",
            unminted,
            Some(WORKSPACE),
            false,
            unminted,
        ),
    ];

    for (label, job_id, scope, found, named) in rows {
        let (status, body) = describe(&router, job_id, scope).await;
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
            let message = body["message"].as_str().unwrap_or("");
            assert!(
                message.contains(named),
                "case {label:?}: the refusal must name {named:?}, got {body}"
            );
        }
    }
}

/// Start a job with `body`, asserting the create was served, and answer its id.
async fn start(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = create(router, body).await;
    assert_eq!(status, 202, "the create got {status} {created}");
    created["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("no jobId in {created}"))
        .to_string()
}

#[tokio::test]
async fn a_create_the_store_cannot_serve_starts_no_job() {
    let (router, engine) = router_with_engine();
    enable_warm_tier(&router).await;
    create_workspace(&router, WORKSPACE).await;
    create_workspace(&router, OTHER_WORKSPACE).await;
    let dataset_id = create_dataset(&router, "Import target", WORKSPACE).await;
    // A dataset of ANOTHER workspace, and one no workspace contains, so the rows
    // below can show the dataset lookup is scoped to the workspace the job names.
    let other_dataset_id = create_dataset(&router, "Other target", OTHER_WORKSPACE).await;
    let workspace_less_dataset_id = {
        let (status, body) = call(
            &router,
            "POST",
            "/datasets",
            Some(json!({
                "datasetName": "Unscoped target",
                "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
            })),
        )
        .await;
        assert_eq!(status, 202, "create dataset: got {status} {body}");
        body["datasetId"]
            .as_str()
            .unwrap_or_else(|| panic!("no datasetId in {body}"))
            .to_string()
    };

    let one_file = json!([file("sitewise-import", "turbines/01.csv", json!({}))]);
    let base = json!({
        "jobName": "Refused import",
        "jobRoleArn": JOB_ROLE_ARN,
        "files": one_file,
        "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
    });
    /// Build a create body from `base` with `patch`'s members added.
    fn with(base: &Value, patch: Value) -> Value {
        let mut body = base.clone();
        let object = body.as_object_mut().expect("an object body");
        for (member, value) in patch.as_object().cloned().unwrap_or_default() {
            object.insert(member, value);
        }
        body
    }

    // Files one past the documented quota, and the shortest list that breaks it.
    let too_many_files = Value::Array(
        (0..=MAX_FILES_PER_BULK_IMPORT_JOB)
            .map(|at| file("sitewise-import", &format!("turbines/part-{at}.csv"), json!({})))
            .collect(),
    );

    // (case, the body, the status it is refused with, the error code it carries).
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a workspace with no dataset beside it",
            with(&base, json!({ "workspaceName": WORKSPACE })),
            400,
            "InvalidRequestException",
        ),
        (
            "a dataset with no workspace beside it",
            with(&base, json!({ "datasetId": dataset_id })),
            400,
            "InvalidRequestException",
        ),
        (
            "a workspace this store does not hold",
            with(
                &base,
                json!({ "workspaceName": "no-such-workspace", "datasetId": dataset_id }),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a dataset this store does not hold",
            with(
                &base,
                json!({
                    "workspaceName": WORKSPACE,
                    "datasetId": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                }),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a dataset another workspace contains",
            with(
                &base,
                json!({ "workspaceName": WORKSPACE, "datasetId": other_dataset_id }),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a dataset no workspace contains",
            with(
                &base,
                json!({ "workspaceName": WORKSPACE, "datasetId": workspace_less_dataset_id }),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "one file past the documented quota",
            with(&base, json!({ "files": too_many_files })),
            410,
            "LimitExceededException",
        ),
    ];

    for (label, body, expected_status, expected_code) in rows {
        let held = jobs_held(&engine);
        let (status, answer) = create(&router, body).await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: expected {expected_status}, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            expected_code,
            "case {label:?}: unexpected error code in {answer}"
        );
        assert_eq!(
            jobs_held(&engine),
            held,
            "case {label:?}: a refused create must start no job"
        );
    }

    // The quota's own boundary is accepted, so the refusal above is the quota and
    // not an off-by-one.
    let at_the_quota = Value::Array(
        (0..MAX_FILES_PER_BULK_IMPORT_JOB)
            .map(|at| file("sitewise-import", &format!("turbines/part-{at}.csv"), json!({})))
            .collect(),
    );
    let held = jobs_held(&engine);
    let (status, answer) = create(&router, with(&base, json!({ "files": at_the_quota }))).await;
    assert_eq!(
        status, 202,
        "a job naming exactly {MAX_FILES_PER_BULK_IMPORT_JOB} files is served, got {status} \
         {answer}"
    );
    assert_eq!(
        jobs_held(&engine),
        held + 1,
        "a served create starts exactly one job"
    );
}

#[tokio::test]
async fn ingesting_outside_a_workspace_needs_the_account_s_warm_or_cold_tier() {
    // (case, the storage configuration the account is put into — `None` for an
    // account no put has configured — whether a job outside a workspace is then
    // served).
    let rows: Vec<(&str, Option<Value>, bool)> = vec![
        ("an account nothing has configured", None, false),
        (
            "the hot tier alone",
            Some(json!({ "storageType": STORAGE_TYPE_SITEWISE_DEFAULT })),
            false,
        ),
        (
            "the hot tier with the warm tier turned off",
            Some(json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "warmTier": STORAGE_STATE_DISABLED,
            })),
            false,
        ),
        (
            "the warm tier enabled",
            Some(json!({
                "storageType": STORAGE_TYPE_SITEWISE_DEFAULT,
                "warmTier": STORAGE_STATE_ENABLED,
            })),
            true,
        ),
        (
            "the cold tier, which multi-layer storage is",
            Some(json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": multi_layer_storage("sitewise-cold", "SiteWiseCold"),
            })),
            true,
        ),
        (
            "both tiers",
            Some(json!({
                "storageType": STORAGE_TYPE_MULTI_LAYER,
                "multiLayerStorage": multi_layer_storage("sitewise-cold", "SiteWiseCold"),
                "warmTier": STORAGE_STATE_ENABLED,
            })),
            true,
        ),
    ];

    for (label, configuration, served) in rows {
        // One account per row: the configuration is account-wide, so a row must
        // not read the one the row before it put.
        let router = router();
        if let Some(configuration) = configuration {
            let (status, body) = put_storage(&router, configuration).await;
            assert_eq!(
                status, 200,
                "case {label:?}: PutStorageConfiguration got {status} {body}"
            );
        }

        let body = json!({
            "jobName": "Tiered import",
            "jobRoleArn": JOB_ROLE_ARN,
            "files": [file("sitewise-import", "turbines/01.csv", json!({}))],
            "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
        });
        let (status, answer) = create(&router, body).await;
        if served {
            assert_eq!(
                status, 202,
                "case {label:?}: expected 202, got {status} {answer}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: expected 400, got {status} {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "InvalidRequestException",
                "case {label:?}: unexpected error code in {answer}"
            );
        }

        // The precondition is about the tiers, not about every job: a job that
        // ingests into a session dataset of a workspace is served whatever the
        // account's storage configuration is.
        create_workspace(&router, WORKSPACE).await;
        let dataset_id = create_dataset(&router, "Import target", WORKSPACE).await;
        let (status, answer) = create(
            &router,
            json!({
                "jobName": "Session import",
                "jobRoleArn": JOB_ROLE_ARN,
                "files": [file("sitewise-import", "sessions/01.csv", json!({}))],
                "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
                "workspaceName": WORKSPACE,
                "datasetId": dataset_id,
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {label:?}: a job into a session dataset needs no tier, got {status} {answer}"
        );
    }
}

/// Every value `DatasetTypeEnum` admits. Source: `smithy_query(query="shape",
/// shape="DatasetTypeEnum")` — SESSION | CURATED | EXTERNAL — written out here as
/// an oracle independent of the crate under test, so no kind the enum carries is
/// left out of the table below.
const DATASET_TYPES: [&str; 3] = ["SESSION", "CURATED", "EXTERNAL"];

#[tokio::test]
async fn the_destination_dataset_is_held_to_the_kind_the_member_documents() {
    let (router, engine) = router_with_engine();
    create_workspace(&router, WORKSPACE).await;

    // (case, the `datasetType` the destination dataset carries, whether the
    // dataset is created INSIDE the workspace the job names, the status the create
    // is answered with, the error code it carries when refused).
    //
    // Every kind `DATASET_TYPES` lists has a row: an EXTERNAL dataset is one that
    // "connects to an external datasource", so `CreateDataset` refuses to put one
    // in a workspace at all and the only EXTERNAL destination a store can hold is
    // one no workspace contains — which the workspace-scoped lookup reports as the
    // absence it is, ahead of the kind. A dataset with no stored kind states
    // nothing about which side of the rule it falls on, so it is not refused.
    let rows: Vec<(&str, Option<&str>, bool, u16, &str)> = vec![
        (
            "the SESSION kind the member documents",
            Some(DATASET_TYPES[0]),
            true,
            202,
            "",
        ),
        (
            "a CURATED dataset of the workspace the job names",
            Some(DATASET_TYPES[1]),
            true,
            400,
            "InvalidRequestException",
        ),
        (
            "an EXTERNAL dataset, which no workspace contains",
            Some(DATASET_TYPES[2]),
            false,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a dataset the store holds no kind for",
            None,
            true,
            202,
            "",
        ),
    ];

    for (index, (label, dataset_type, in_workspace, expected_status, expected_code)) in
        rows.into_iter().enumerate()
    {
        let dataset_id = super::dataset_data_segments::create_dataset(
            &router,
            &format!("Destination {index}"),
            dataset_type,
            in_workspace.then_some(WORKSPACE),
        )
        .await;

        let held = jobs_held(&engine);
        let (status, answer) = create(
            &router,
            json!({
                "jobName": format!("Kind import {index}"),
                "jobRoleArn": JOB_ROLE_ARN,
                "files": [file("sitewise-import", "turbines/01.csv", json!({}))],
                "errorReportLocation": { "bucket": "sitewise-errors", "prefix": "imports/" },
                "workspaceName": WORKSPACE,
                "datasetId": dataset_id,
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: expected {expected_status}, got {status} {answer}"
        );

        if expected_status == 202 {
            // A served create starts exactly one job, and the destination it was
            // started for reads back through `DescribeBulkImportJob`.
            assert_eq!(
                jobs_held(&engine),
                held + 1,
                "case {label:?}: a served create starts exactly one job"
            );
            let job_id = answer["jobId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: no jobId in {answer}"));
            let (status, described) = describe(&router, job_id, Some(WORKSPACE)).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the read back got {status} {described}"
            );
            assert_eq!(
                described["datasetId"].as_str().unwrap_or(""),
                dataset_id,
                "case {label:?}: unexpected datasetId in {described}"
            );
            assert_eq!(
                described["workspaceName"].as_str().unwrap_or(""),
                WORKSPACE,
                "case {label:?}: unexpected workspaceName in {described}"
            );
            continue;
        }

        assert_eq!(
            error_code(&answer),
            expected_code,
            "case {label:?}: unexpected error code in {answer}"
        );
        assert_eq!(
            jobs_held(&engine),
            held,
            "case {label:?}: a refused create must start no job"
        );
        let message = answer["message"].as_str().unwrap_or("");
        assert!(
            message.contains(&dataset_id),
            "case {label:?}: the refusal must name the dataset it addressed, got {answer}"
        );
        if expected_status == 400 {
            let held_kind = dataset_type.unwrap_or_default();
            assert!(
                message.contains(DATASET_TYPES[0]) && message.contains(held_kind),
                "case {label:?}: the refusal must name the kind required and the kind held, \
                 got {answer}"
            );
        }
    }
}
