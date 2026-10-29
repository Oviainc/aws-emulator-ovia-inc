// SPDX-License-Identifier: CC0-1.0

//! The same outside-in resolution the sibling `workspace_scoped_path_resolution`
//! suite pins for `/workspaces/{name}/…` routes, for the operations that name
//! their workspace in a QUERY parameter or in the request BODY instead of in an
//! `@httpLabel`: the dataset family (`smithy_query query="operation"` on
//! DescribeDataset, UpdateDataset, DeleteDataset, ListDatasetDataSegments,
//! ListDatasetDataSegmentRelationships and the three batch data-segment
//! operations, where `workspaceName` is an `@httpQuery` or a body member).
//!
//! The rule: an operation whose request names a workspace resolves that
//! workspace through the workspace registry BEFORE the resource it scopes to it,
//! so a request naming a workspace this service does not hold is refused as the
//! WORKSPACE being absent, and the dataset reports its own absence only once the
//! workspace resolves. The refusal is the modelled `ResourceNotFoundException`
//! (HTTP 404) stating the resource KIND and the identifier the request supplied.
//!
//! The rows span that whole class — the read, the update, the delete, both
//! listings and all three batch calls — rather than the one request an issue
//! happened to record, and each row is driven against several workspace names
//! spanning the modelled `WorkspaceName` form.
//!
//! The expected strings are written out BY HAND as an independent oracle: this
//! suite never calls the crate's message helpers, so a handler that drifts to a
//! template of its own cannot agree with itself.
//!
//! Everything is driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a client
//! sees.

use serde_json::{json, Value};

use crate::asset_lifecycle::{call, error_code, router};
use crate::dataset_data_segments::create_dataset;
use crate::hierarchies_etags_and_refusals::error_message;
use crate::support::ensure_workspace;

/// The workspace this suite opens, so a row can be wrong only in the dataset it
/// scopes to it.
const HELD: &str = "segment-shop";

/// An `ID` the modelled form admits (len 36..36, lower-case hex groups, and not
/// the identifier the pattern excludes) that no dataset answers to.
const UNKNOWN_DATASET: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// A second one, for the entry member that names the OTHER dataset of a pair.
const OTHER_UNKNOWN_DATASET: &str = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";

/// A `TimeSeriesId` (len 36..73) no data stream answers to.
const UNKNOWN_SERIES: &str = "eeeeeeee-ffff-aaaa-bbbb-cccccccccccc";

/// The resource KINDS this service words its absences with, as the wire reports
/// them.
const WORKSPACE_KIND: &str = "Workspace";
const DATASET_KIND: &str = "Dataset";

/// Workspace names no `CreateWorkspace` of this suite opens, each inside the
/// modelled `WorkspaceName` (len 1..64, pattern `^[a-zA-Z0-9_-]+$`): the shortest
/// name it admits, a mixed-case name with the punctuation it allows, and one at
/// the published maximum length.
const UNKNOWN_WORKSPACES: [&str; 3] = [
    "q",
    "Shop_9-ghost",
    "uuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuuu",
];

/// How this service states an absence, written out by hand.
fn absent(kind: &str, identifier: &str) -> String {
    format!("{kind} with ID {identifier} not found")
}

/// A `TimeInNanos` on the wire, at a second inside the modelled range
/// (`timeInSeconds` `@range(min: 1, max: 9223372036854774)`).
fn instant(seconds: i64) -> Value {
    json!({ "timeInSeconds": seconds, "offsetInNanos": 0 })
}

/// A `DatasetSource` carrying the two members the shape requires, each a value of
/// its own enum (`DatasetSourceType`, `DatasetSourceFormat`).
fn dataset_source() -> Value {
    json!({ "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" })
}

/// One row of the table: an operation that names its workspace outside the path.
struct Case {
    label: &'static str,
    method: &'static str,
    /// The request's URI, for the workspace the row is driven against and the
    /// dataset id it addresses.
    uri: fn(&str, &str) -> String,
    /// The request's body, for that workspace and dataset id; `None` where the
    /// operation takes no body.
    body: fn(&str, &str) -> Option<Value>,
}

/// The rows: every operation of this service whose `workspaceName` is an
/// `@httpQuery` member or a body member and which addresses a dataset scoped to
/// it.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "DescribeDataset",
            method: "GET",
            uri: |workspace, dataset| format!("/datasets/{dataset}?workspaceName={workspace}"),
            body: |_, _| None,
        },
        Case {
            label: "UpdateDataset",
            method: "PUT",
            uri: |_, dataset| format!("/datasets/{dataset}"),
            body: |workspace, _| {
                Some(json!({
                    "workspaceName": workspace,
                    "datasetName": "A write no dataset takes",
                    "datasetSource": dataset_source(),
                }))
            },
        },
        Case {
            label: "DeleteDataset",
            method: "DELETE",
            uri: |workspace, dataset| format!("/datasets/{dataset}?workspaceName={workspace}"),
            body: |_, _| None,
        },
        Case {
            label: "ListDatasetDataSegments",
            method: "GET",
            uri: |workspace, dataset| {
                format!("/datasets/{dataset}/data-segments?workspaceName={workspace}")
            },
            body: |_, _| None,
        },
        Case {
            label: "ListDatasetDataSegmentRelationships",
            method: "GET",
            uri: |workspace, dataset| {
                format!("/datasets/{dataset}/data-segment-relationships?workspaceName={workspace}")
            },
            body: |_, _| None,
        },
        Case {
            label: "BatchDeleteDatasetDataSegments",
            method: "POST",
            uri: |_, dataset| format!("/datasets/{dataset}/data-segments/batch-delete"),
            body: |workspace, _| {
                Some(json!({
                    "workspaceName": workspace,
                    "deleteDataSegmentEntries": [{
                        "timeSeriesId": OTHER_UNKNOWN_DATASET,
                        "startTimestamp": instant(1_700_000_000),
                        "endTimestamp": instant(1_700_000_100),
                    }],
                }))
            },
        },
        Case {
            label: "BatchAssociateDataSegmentsToDataset",
            method: "POST",
            uri: |_, dataset| format!("/datasets/{dataset}/data-segments/associate"),
            body: |workspace, _| {
                Some(json!({
                    "workspaceName": workspace,
                    "associateDataSegmentEntries": [{
                        "sourceDatasetId": OTHER_UNKNOWN_DATASET,
                        "timeSeriesId": UNKNOWN_SERIES,
                        "startTimestamp": instant(1_700_000_000),
                        "endTimestamp": instant(1_700_000_100),
                    }],
                }))
            },
        },
        Case {
            label: "BatchDisassociateDataSegmentsFromDataset",
            method: "POST",
            uri: |_, dataset| format!("/datasets/{dataset}/data-segments/disassociate"),
            body: |workspace, _| {
                Some(json!({
                    "workspaceName": workspace,
                    "disassociateDataSegmentEntries": [{
                        "sourceDatasetId": OTHER_UNKNOWN_DATASET,
                        "timeSeriesId": UNKNOWN_SERIES,
                        "startTimestamp": instant(1_700_000_000),
                        "endTimestamp": instant(1_700_000_100),
                    }],
                }))
            },
        },
    ]
}

#[tokio::test]
async fn a_request_naming_an_unheld_workspace_reports_the_workspace() {
    let router = router();
    ensure_workspace(&router, HELD).await;
    // A dataset the store DOES hold, in the held workspace: a row naming it under
    // an unheld workspace is wrong only about the workspace, so the refusal cannot
    // be passing for the dataset's own absence.
    let held_dataset = create_dataset(&router, "Shop session", Some("SESSION"), Some(HELD)).await;

    for case in cases() {
        for workspace in UNKNOWN_WORKSPACES {
            for dataset in [held_dataset.as_str(), UNKNOWN_DATASET] {
                let uri = (case.uri)(workspace, dataset);
                let body = (case.body)(workspace, dataset);
                let (status, answer) = call(&router, case.method, &uri, body).await;
                assert_eq!(
                    status, 404,
                    "case {} on the unheld workspace {workspace:?} with dataset {dataset}: a \
                     request naming a workspace this service does not hold is refused, got \
                     {status} {answer}",
                    case.label
                );
                assert_eq!(
                    error_code(&answer),
                    "ResourceNotFoundException",
                    "case {} on the unheld workspace {workspace:?} with dataset {dataset}: \
                     unexpected error code in {answer}",
                    case.label
                );
                assert_eq!(
                    error_message(&answer),
                    absent(WORKSPACE_KIND, workspace),
                    "case {} on the unheld workspace {workspace:?} with dataset {dataset}: the \
                     refusal must name the WORKSPACE as the absent resource, got {answer}",
                    case.label
                );
            }
        }
    }

    // None of those refusals removed the dataset they named.
    let (status, described) = call(
        &router,
        "GET",
        &format!("/datasets/{held_dataset}?workspaceName={HELD}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "a refused request must leave the dataset it named, got {status} {described}"
    );
}

#[tokio::test]
async fn a_held_workspace_leaves_the_dataset_to_report_its_own_absence() {
    let router = router();
    ensure_workspace(&router, HELD).await;

    for case in cases() {
        let uri = (case.uri)(HELD, UNKNOWN_DATASET);
        let body = (case.body)(HELD, UNKNOWN_DATASET);
        let (status, answer) = call(&router, case.method, &uri, body).await;
        assert_eq!(
            status, 404,
            "case {}: a request naming a held workspace and a dataset it does not hold is \
             refused, got {status} {answer}",
            case.label
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {}: unexpected error code in {answer}",
            case.label
        );
        assert_eq!(
            error_message(&answer),
            absent(DATASET_KIND, UNKNOWN_DATASET),
            "case {}: with the workspace held, the refusal must name the dataset, got {answer}",
            case.label
        );
    }
}

/// The resolution the two suites above pin does not stand in the way of a request
/// that names a workspace this service holds AND a dataset that workspace holds:
/// the dataset created here reads back through `DescribeDataset` scoped by its own
/// workspace, and its segment listing is served as the empty page it is.
#[tokio::test]
async fn a_held_dataset_of_a_held_workspace_still_reads_back() {
    let router = router();
    let dataset = create_dataset(&router, "Shop curated", Some("CURATED"), Some(HELD)).await;

    let (status, described) = call(
        &router,
        "GET",
        &format!("/datasets/{dataset}?workspaceName={HELD}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DescribeDataset: got {status} {described}");
    assert_eq!(
        described["datasetId"].as_str().unwrap_or_default(),
        dataset,
        "DescribeDataset: expected datasetId {dataset}, got {described}"
    );
    assert_eq!(
        described["workspaceName"].as_str().unwrap_or_default(),
        HELD,
        "DescribeDataset: expected workspaceName {HELD}, got {described}"
    );

    let (status, listed) = call(
        &router,
        "GET",
        &format!("/datasets/{dataset}/data-segments?workspaceName={HELD}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListDatasetDataSegments: got {status} {listed}"
    );
    assert_eq!(
        listed["dataSegments"].as_array().map(Vec::len),
        Some(0),
        "ListDatasetDataSegments: a dataset holding no segment lists none, got {listed}"
    );
}
