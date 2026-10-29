// SPDX-License-Identifier: CC0-1.0

//! Every operation whose route nests a JOB inside `/workspaces/{workspaceName}`
//! resolves the path's resources OUTSIDE IN, so the modelled 404 names the
//! OUTERMOST resource the request got wrong: the workspace when this service
//! holds no workspace of that name, and only then the job inside it.
//!
//! The rows span that whole class of job routes — both job kinds the service has
//! (dataset export jobs and enrichment jobs) and every verb reaching one of them
//! (the read of one job, the listing of them, the create that starts one, and the
//! cancel) — rather than the one read an issue happened to record. The workspace
//! names no workspace answers to vary along the modelled `WorkspaceName`
//! constraint (len 1..64, pattern `^[a-zA-Z0-9_-]+$`, from
//! `smithy_query query="shape" shape="WorkspaceName"`), including both published
//! length bounds, so the expected message is built from what the row sent rather
//! than from one recorded value.
//!
//! The expected strings are written out BY HAND as an independent oracle: this
//! suite never calls the crate's message helpers, so a handler that drifts to a
//! template of its own cannot agree with itself.
//!
//! Everything is driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a client
//! sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, router};
use super::hierarchies_etags_and_refusals::error_message;

/// The workspace the fixture opens and starts both kinds of job in, so a row can
/// be wrong only INSIDE it.
const WORKSPACE: &str = "job-resolution";

/// A second workspace the fixture opens and leaves EMPTY, so a row addressing the
/// first workspace's job through it is wrong in the job alone.
const NEIGHBOUR: &str = "job-resolution-neighbour";

/// Workspace names no workspace answers to, each inside the modelled
/// `WorkspaceName` (len 1..64, pattern `^[a-zA-Z0-9_-]+$`): a name at the
/// published MINIMUM length, a hyphenated name, a mixed-case name with an
/// underscore and digits, and one at the published MAXIMUM length.
const UNKNOWN_WORKSPACES: [&str; 4] = [
    "z",
    "no-such-workspace",
    "Absent_Workspace_42",
    "wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww",
];

/// A `DatasetExportJobId` (len 36..36, lower-case hex groups) no export job
/// holds.
const UNKNOWN_EXPORT_JOB: &str = "11111111-2222-3333-4444-555555555555";

/// An `EnrichmentJobId` of the same form — and not the all-zero id its pattern
/// excludes — that no enrichment job holds.
const UNKNOWN_ENRICHMENT_JOB: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The resource KINDS this service words its absences with, as the wire reports
/// them.
const WORKSPACE_KIND: &str = "Workspace";
const EXPORT_JOB_KIND: &str = "Dataset Export Job";
const ENRICHMENT_JOB_KIND: &str = "Enrichment Job";

/// An S3 URI of the form `S3Uri` admits
/// (`^s3://[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]/.+$`).
const DESTINATION: &str = "s3://sitewise-exports/clips/";

/// The same member's error-report form.
const ERROR_REPORT: &str = "s3://sitewise-exports/errors/";

/// One row: the case name, the HTTP method, the URI, the request body, and the
/// absence the refusal must state.
type Row = (&'static str, &'static str, String, Option<Value>, String);

/// How this service states an absence, written out by hand.
fn absent(kind: &str, identifier: &str) -> String {
    format!("{kind} with ID {identifier} not found")
}

/// What the fixture leaves behind: the dataset both job kinds are started over,
/// and the id of each job it started in [`WORKSPACE`].
struct Fixture {
    export_job: String,
    enrichment_job: String,
}

/// A `CreateDatasetExportJob` body whose `ProcessingInput` names a time series by
/// alias, so no row depends on a dataset being resolved.
fn export_body() -> Value {
    json!({
        "destinationS3Uri": DESTINATION,
        "errorReportLocation": { "s3Uri": ERROR_REPORT },
        "input": { "timeseries": [{ "propertyAlias": "/plant/line-1/rate" }] },
    })
}

/// A `CreateEnrichmentJob` body analysing `dataset_id` over a non-empty window,
/// as `EnrichmentJobConfiguration`'s `eventDetection` member states it.
fn enrichment_body(dataset_id: &str) -> Value {
    json!({
        "jobConfiguration": {
            "eventDetection": {
                "datasetId": dataset_id,
                "trimSettings": {
                    "startTime": { "timeInSeconds": 1_700_000_000, "offsetInNanos": 0 },
                    "endTime": { "timeInSeconds": 1_700_003_600, "offsetInNanos": 0 },
                },
            }
        }
    })
}

/// Two open workspaces, one holding a dataset export job and an enrichment job
/// and the other holding none, so every row below is wrong in exactly the place
/// it says.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    crate::support::ensure_workspace(router, WORKSPACE).await;
    crate::support::ensure_workspace(router, NEIGHBOUR).await;

    let (status, dataset) = call(
        router,
        "POST",
        "/datasets",
        Some(json!({
            "datasetName": "Job resolution source",
            "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
        })),
    )
    .await;
    assert_eq!(status, 202, "create dataset: got {status} {dataset}");
    let dataset_id = dataset["datasetId"]
        .as_str()
        .unwrap_or_else(|| panic!("create dataset: no datasetId in {dataset}"))
        .to_string();

    let (status, export) = call(
        router,
        "POST",
        &format!("/workspaces/{WORKSPACE}/dataset-export-jobs"),
        Some(export_body()),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateDatasetExportJob: got {status} {export}"
    );
    let export_job = export["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDatasetExportJob: no jobId in {export}"))
        .to_string();

    let (status, enrichment) = call(
        router,
        "POST",
        &format!("/workspaces/{WORKSPACE}/enrichment-jobs"),
        Some(enrichment_body(&dataset_id)),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateEnrichmentJob: got {status} {enrichment}"
    );
    let enrichment_job = enrichment["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateEnrichmentJob: no jobId in {enrichment}"))
        .to_string();

    Fixture {
        export_job,
        enrichment_job,
    }
}

/// Drive `rows` and assert each is the modelled 404 stating the absence the row
/// expects.
async fn assert_rows(router: &MultiServiceRouter, rows: Vec<Row>) {
    for (case, method, uri, body, expected) in rows {
        let (status, answered) = call(router, method, &uri, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            expected,
            "case {case:?}: expected {expected:?}, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_workspace_this_service_does_not_hold_is_the_absence_every_job_route_reports() {
    let router = router();
    let held = fixture(&router).await;

    // Each row names a workspace the store holds no record of. Whether the job id
    // on the path is one a create really minted changes nothing: the workspace is
    // the outermost resource the request got wrong, so it is the one the refusal
    // names. The unknown workspace names rotate through the rows, so the expected
    // message is built from the identifier the row sent.
    let [shortest, hyphenated, mixed, longest] = UNKNOWN_WORKSPACES;
    let export = held.export_job.as_str();
    let enrichment = held.enrichment_job.as_str();
    let rows: Vec<Row> = vec![
        (
            "DescribeDatasetExportJob, a job id a create really minted",
            "GET",
            format!("/workspaces/{hyphenated}/dataset-export-jobs/{export}"),
            None,
            absent(WORKSPACE_KIND, hyphenated),
        ),
        (
            "DescribeDatasetExportJob, wrong in the workspace and the job",
            "GET",
            format!("/workspaces/{mixed}/dataset-export-jobs/{UNKNOWN_EXPORT_JOB}"),
            None,
            absent(WORKSPACE_KIND, mixed),
        ),
        (
            "DescribeDatasetExportJob, a workspace name at the modelled maximum length",
            "GET",
            format!("/workspaces/{longest}/dataset-export-jobs/{export}"),
            None,
            absent(WORKSPACE_KIND, longest),
        ),
        (
            "DescribeDatasetExportJob, a workspace name at the modelled minimum length",
            "GET",
            format!("/workspaces/{shortest}/dataset-export-jobs/{export}"),
            None,
            absent(WORKSPACE_KIND, shortest),
        ),
        (
            "ListDatasetExportJobs",
            "GET",
            format!("/workspaces/{hyphenated}/dataset-export-jobs"),
            None,
            absent(WORKSPACE_KIND, hyphenated),
        ),
        (
            "ListDatasetExportJobs, with a filter and a page size",
            "GET",
            format!("/workspaces/{mixed}/dataset-export-jobs?filter=ALL&maxResults=1"),
            None,
            absent(WORKSPACE_KIND, mixed),
        ),
        (
            "CreateDatasetExportJob",
            "POST",
            format!("/workspaces/{longest}/dataset-export-jobs"),
            Some(export_body()),
            absent(WORKSPACE_KIND, longest),
        ),
        (
            "DescribeEnrichmentJob, a job id a create really minted",
            "GET",
            format!("/workspaces/{shortest}/enrichment-jobs/{enrichment}"),
            None,
            absent(WORKSPACE_KIND, shortest),
        ),
        (
            "DescribeEnrichmentJob, wrong in the workspace and the job",
            "GET",
            format!("/workspaces/{hyphenated}/enrichment-jobs/{UNKNOWN_ENRICHMENT_JOB}"),
            None,
            absent(WORKSPACE_KIND, hyphenated),
        ),
        (
            "ListEnrichmentJobs",
            "GET",
            format!("/workspaces/{mixed}/enrichment-jobs"),
            None,
            absent(WORKSPACE_KIND, mixed),
        ),
        (
            "CancelEnrichmentJob, a job id a create really minted",
            "POST",
            format!("/workspaces/{longest}/enrichment-jobs/{enrichment}/cancel"),
            Some(json!({})),
            absent(WORKSPACE_KIND, longest),
        ),
        (
            "CancelEnrichmentJob, wrong in the workspace and the job",
            "POST",
            format!("/workspaces/{shortest}/enrichment-jobs/{UNKNOWN_ENRICHMENT_JOB}/cancel"),
            Some(json!({})),
            absent(WORKSPACE_KIND, shortest),
        ),
    ];
    assert_rows(&router, rows).await;
}

#[tokio::test]
async fn a_workspace_that_resolves_refuses_the_job_inside_it_the_path_names() {
    let router = router();
    let held = fixture(&router).await;
    let export = held.export_job.as_str();
    let enrichment = held.enrichment_job.as_str();

    // Each row names a workspace the fixture OPENED, so the outermost resource the
    // request gets wrong is the job inside it: an id no create ever minted, or a
    // job of another workspace, which the workspace the path names does not hold.
    let rows: Vec<Row> = vec![
        (
            "DescribeDatasetExportJob, an id no create minted",
            "GET",
            format!("/workspaces/{WORKSPACE}/dataset-export-jobs/{UNKNOWN_EXPORT_JOB}"),
            None,
            absent(EXPORT_JOB_KIND, UNKNOWN_EXPORT_JOB),
        ),
        (
            "DescribeDatasetExportJob, a job of another workspace",
            "GET",
            format!("/workspaces/{NEIGHBOUR}/dataset-export-jobs/{export}"),
            None,
            absent(EXPORT_JOB_KIND, export),
        ),
        (
            "DescribeEnrichmentJob, an id no create minted",
            "GET",
            format!("/workspaces/{WORKSPACE}/enrichment-jobs/{UNKNOWN_ENRICHMENT_JOB}"),
            None,
            absent(ENRICHMENT_JOB_KIND, UNKNOWN_ENRICHMENT_JOB),
        ),
        (
            "DescribeEnrichmentJob, a job of another workspace",
            "GET",
            format!("/workspaces/{NEIGHBOUR}/enrichment-jobs/{enrichment}"),
            None,
            absent(ENRICHMENT_JOB_KIND, enrichment),
        ),
        (
            "CancelEnrichmentJob, an id no create minted",
            "POST",
            format!("/workspaces/{WORKSPACE}/enrichment-jobs/{UNKNOWN_ENRICHMENT_JOB}/cancel"),
            Some(json!({})),
            absent(ENRICHMENT_JOB_KIND, UNKNOWN_ENRICHMENT_JOB),
        ),
        (
            "CancelEnrichmentJob, a job of another workspace",
            "POST",
            format!("/workspaces/{NEIGHBOUR}/enrichment-jobs/{enrichment}/cancel"),
            Some(json!({})),
            absent(ENRICHMENT_JOB_KIND, enrichment),
        ),
    ];
    assert_rows(&router, rows).await;
}

#[tokio::test]
async fn resolving_the_workspace_first_still_serves_every_job_path_the_store_holds() {
    let router = router();
    let held = fixture(&router).await;
    let export = held.export_job.as_str();
    let enrichment = held.enrichment_job.as_str();

    // Resolving the workspace first refuses nothing that resolves: each row is the
    // same route as a refusal above, with every resource on it present. The reads
    // report the job the path names, and the listings report the jobs the fixture
    // started in the workspace they name — and none of the other workspace's.
    let (status, body) = call(
        &router,
        "GET",
        &format!("/workspaces/{WORKSPACE}/dataset-export-jobs/{export}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeDatasetExportJob on its own path: got {status} {body}"
    );
    assert_eq!(
        body["jobId"].as_str(),
        Some(export),
        "DescribeDatasetExportJob reports the job it was asked for, expected {export}, got {body}"
    );
    assert_eq!(
        body["workspaceName"].as_str(),
        Some(WORKSPACE),
        "DescribeDatasetExportJob reports its workspace, expected {WORKSPACE}, got {body}"
    );

    let (status, body) = call(
        &router,
        "GET",
        &format!("/workspaces/{WORKSPACE}/enrichment-jobs/{enrichment}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeEnrichmentJob on its own path: got {status} {body}"
    );
    assert_eq!(
        body["jobId"].as_str(),
        Some(enrichment),
        "DescribeEnrichmentJob reports the job it was asked for, expected {enrichment}, got {body}"
    );

    // (case, uri, the items member, the job the listing must report)
    let listings: [(&str, String, &str, &str); 4] = [
        (
            "ListDatasetExportJobs in the workspace holding one",
            format!("/workspaces/{WORKSPACE}/dataset-export-jobs"),
            "jobs",
            export,
        ),
        (
            "ListEnrichmentJobs in the workspace holding one",
            format!("/workspaces/{WORKSPACE}/enrichment-jobs"),
            "jobs",
            enrichment,
        ),
        (
            "ListDatasetExportJobs in the empty workspace",
            format!("/workspaces/{NEIGHBOUR}/dataset-export-jobs"),
            "jobs",
            "",
        ),
        (
            "ListEnrichmentJobs in the empty workspace",
            format!("/workspaces/{NEIGHBOUR}/enrichment-jobs"),
            "jobs",
            "",
        ),
    ];
    for (case, uri, member, expected) in listings {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let reported: Vec<&str> = body[member]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no {member} list in {body}"))
            .iter()
            .map(|summary| summary["jobId"].as_str().unwrap_or(""))
            .collect();
        let wanted: Vec<&str> = if expected.is_empty() {
            Vec::new()
        } else {
            vec![expected]
        };
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected {wanted:?}, got {body}"
        );
    }
}
