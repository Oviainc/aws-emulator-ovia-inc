// SPDX-License-Identifier: CC0-1.0

//! The two READS of the IoT SiteWise dataset family — the single read
//! `DescribeDataset` and the listing `ListDatasets` — agree about which resource
//! a request that is wrong in more than one way reports as absent.
//!
//! The rule these tests pin: an operation whose request names a `workspaceName`
//! resolves that name through the workspace registry BEFORE the resource scoped
//! to it, so a request naming a workspace this service holds no record of is
//! refused with the modelled `ResourceNotFoundException` (HTTP 404) stating the
//! WORKSPACE as the absent resource — whatever else about the same request is
//! also absent (a dataset id nothing minted, a `datasetVersion` no write
//! produced, a dataset that lives outside every workspace) — and the resource
//! nested in the workspace reports its own absence only once the workspace
//! resolves. A listing is bound by the same rule: an absent workspace is that
//! refusal rather than an empty page, which would report absence as success.
//!
//! Where the sibling `workspace_scoped_absence` suite spans the OPERATIONS that
//! name their workspace outside the request path, this suite spans the other
//! dimensions of the same rule over the dataset reads: which dataset the request
//! addresses (one held inside the workspace, one held outside every workspace,
//! and an id nothing minted), which `datasetVersion` it asks for, and whether the
//! read is the single read or the listing.
//!
//! The expected refusal strings are written out BY HAND as an independent oracle:
//! this suite never calls the crate's message helpers, so a handler that drifts
//! to a template of its own cannot agree with itself.
//!
//! Everything is driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a client
//! sees.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, router};
use crate::dataset_data_segments::create_dataset;
use crate::hierarchies_etags_and_refusals::error_message;
use crate::support::ensure_workspace;

/// The workspace this suite opens, so a row can be wrong only in what it names
/// beside the workspace.
const HELD: &str = "reading-room";

/// A second workspace this suite opens: the dataset of `HELD` addressed under it
/// is wrong in neither the workspace nor the id's FORM, only in which workspace
/// holds the dataset.
const OTHER_HELD: &str = "writing-room";

/// An `ID` the modelled form admits (len 36..36, lower-case hex groups, and not
/// the identifier the pattern excludes) that no dataset answers to.
///
/// Source: `smithy_query(query="operation", operation="DescribeDataset")`, whose
/// `datasetId` is `len 36..36`, pattern
/// `^(?!00000000-0000-0000-0000-000000000000)[0-9a-f]{8}-…$`.
const UNKNOWN_DATASET: &str = "aaaaaaaa-1111-2222-3333-444444444444";

/// The resource KINDS this service words its absences with, as the wire reports
/// them.
const WORKSPACE_KIND: &str = "Workspace";
const DATASET_KIND: &str = "Dataset";

/// The `datasetVersion` a dataset carries when nothing has rewritten it since the
/// create.
const CREATED_VERSION: &str = "1";

/// The `sourceType` every dataset of this suite is created with, which the
/// listing requires (`smithy_query(query="operation", operation="ListDatasets")`
/// marks `sourceType` required).
const SOURCE_TYPE: &str = "SITEWISE";

/// How this service states an absence, written out by hand.
fn absent(kind: &str, identifier: &str) -> String {
    format!("{kind} with ID {identifier} not found")
}

/// Workspace names no `CreateWorkspace` of this suite opens, each inside the
/// modelled `WorkspaceName` form.
///
/// Source: `smithy_query(query="shape", shape="WorkspaceName")` — `len 1..64`,
/// pattern `^[a-zA-Z0-9_-]+$`: the shortest name it admits, a mixed-case name
/// carrying both punctuation marks it allows, and one at the published maximum
/// length.
fn ghost_workspaces() -> Vec<String> {
    vec!["g".to_string(), "Ghost_1-east".to_string(), "g".repeat(64)]
}

/// `datasetVersion` values spanning the modelled `Version` form, plus the request
/// that asks for no particular version.
///
/// Source: `smithy_query(query="operation", operation="DescribeDataset")` —
/// `datasetVersion` is `len 1..10`, pattern `^(0|([1-9]{1}\d*))$`: the lowest
/// value it admits, the version a create mints, one no write of this suite
/// produced, and the longest form inside its length bound.
const VERSIONS: [Option<&str>; 5] = [None, Some("0"), Some("1"), Some("3"), Some("9999999999")];

/// The versions that name no write of a freshly created dataset.
const UNHELD_VERSIONS: [&str; 3] = ["0", "3", "9999999999"];

/// `GET /datasets/{datasetId}` with the optional query members a row names.
fn describe_uri(dataset: &str, workspace: Option<&str>, version: Option<&str>) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(name) = workspace {
        query.push(format!("workspaceName={name}"));
    }
    if let Some(version) = version {
        query.push(format!("datasetVersion={version}"));
    }
    if query.is_empty() {
        format!("/datasets/{dataset}")
    } else {
        format!("/datasets/{dataset}?{}", query.join("&"))
    }
}

/// `GET /datasets` scoped to `workspace`, carrying the required `sourceType`.
fn list_uri(workspace: Option<&str>) -> String {
    match workspace {
        Some(name) => format!("/datasets?sourceType={SOURCE_TYPE}&workspaceName={name}"),
        None => format!("/datasets?sourceType={SOURCE_TYPE}"),
    }
}

/// The members of a described dataset that no read may change, as a row a later
/// read is compared against.
fn stable_projection(described: &Value) -> Vec<String> {
    [
        "datasetId",
        "datasetArn",
        "datasetName",
        "datasetVersion",
        "workspaceName",
        "datasetCreationDate",
        "datasetLastUpdateDate",
    ]
    .iter()
    .map(|member| format!("{member}={}", described[*member]))
    .collect()
}

/// The two datasets every row below is driven against, and the ids they were
/// minted with.
struct Fixture {
    /// A dataset created INSIDE `HELD`.
    inside: String,
    /// A dataset created outside every workspace — the model's rule on
    /// `CreateDataset`'s `workspaceName` ("Omit this field for datasets that
    /// connect to an external datasource") makes that an `EXTERNAL` dataset.
    outside: String,
}

/// Open the workspaces and create the two datasets.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    ensure_workspace(router, HELD).await;
    ensure_workspace(router, OTHER_HELD).await;
    Fixture {
        inside: create_dataset(router, "Reading room session", Some("SESSION"), Some(HELD)).await,
        outside: create_dataset(router, "External connection", Some("EXTERNAL"), None).await,
    }
}

/// A workspace this service does not hold is the absence BOTH reads report, for
/// every dataset a request can address beside it and every version it can ask
/// for.
#[tokio::test]
async fn a_workspace_this_service_does_not_hold_is_the_absence_both_reads_report() {
    let router = router();
    let fixture = fixture(&router).await;

    let targets: [(&str, &str); 3] = [
        ("a dataset held by the workspace this suite opened", &fixture.inside),
        ("a dataset held outside every workspace", &fixture.outside),
        ("an id no create minted", UNKNOWN_DATASET),
    ];

    for ghost in ghost_workspaces() {
        let expected = absent(WORKSPACE_KIND, &ghost);

        for (label, dataset) in targets {
            for version in VERSIONS {
                let case = format!(
                    "DescribeDataset on the unheld workspace {ghost:?} addressing {label} with \
                     datasetVersion {version:?}"
                );
                let (status, answer) =
                    call(&router, "GET", &describe_uri(dataset, Some(&ghost), version), None).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected 404, got {status} {answer}"
                );
                assert_eq!(
                    error_code(&answer),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {answer}"
                );
                assert_eq!(
                    error_message(&answer),
                    expected,
                    "case {case:?}: expected the refusal to name the workspace \
                     ({expected:?}), got {answer}"
                );
            }
        }

        // The listing over the same absent workspace answers the same refusal,
        // not an empty page.
        let case = format!("ListDatasets on the unheld workspace {ghost:?}");
        let (status, answer) = call(&router, "GET", &list_uri(Some(&ghost)), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer}"
        );
        assert_eq!(
            error_message(&answer),
            expected,
            "case {case:?}: the two reads must report the same absent resource \
             ({expected:?}), got {answer}"
        );
    }
}

/// Once the workspace resolves, the dataset and the version it asks for report
/// their own absence — and a request that names them all correctly is served.
#[tokio::test]
async fn a_held_workspace_leaves_the_dataset_and_the_version_to_report_themselves() {
    let router = router();
    let fixture = fixture(&router).await;

    // The dataset of `HELD`, read through its own workspace and without naming
    // one: both name the dataset this request addresses.
    for workspace in [None, Some(HELD)] {
        for version in [None, Some(CREATED_VERSION)] {
            let case = format!(
                "DescribeDataset of the held dataset with workspaceName {workspace:?} and \
                 datasetVersion {version:?}"
            );
            let (status, described) = call(
                &router,
                "GET",
                &describe_uri(&fixture.inside, workspace, version),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {described}"
            );
            assert_eq!(
                described["datasetId"].as_str().unwrap_or_default(),
                fixture.inside,
                "case {case:?}: expected datasetId {}, got {described}",
                fixture.inside
            );
            assert_eq!(
                described["workspaceName"].as_str().unwrap_or_default(),
                HELD,
                "case {case:?}: expected workspaceName {HELD}, got {described}"
            );
            assert_eq!(
                described["datasetVersion"].as_str().unwrap_or_default(),
                CREATED_VERSION,
                "case {case:?}: expected datasetVersion {CREATED_VERSION}, got {described}"
            );
        }
    }

    // Rows that are wrong only BEHIND a workspace this service holds: an id
    // nothing minted, a version no write produced, and the dataset that lives
    // outside every workspace addressed under a workspace that does exist.
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for version in VERSIONS {
        rows.push((
            format!("an id no create minted, datasetVersion {version:?}"),
            describe_uri(UNKNOWN_DATASET, Some(HELD), version),
            UNKNOWN_DATASET.to_string(),
        ));
        rows.push((
            format!(
                "the dataset of another workspace, datasetVersion {version:?}"
            ),
            describe_uri(&fixture.inside, Some(OTHER_HELD), version),
            fixture.inside.clone(),
        ));
        rows.push((
            format!("a dataset outside every workspace, datasetVersion {version:?}"),
            describe_uri(&fixture.outside, Some(HELD), version),
            fixture.outside.clone(),
        ));
    }
    for version in UNHELD_VERSIONS {
        rows.push((
            format!("the held dataset at the unheld datasetVersion {version:?}"),
            describe_uri(&fixture.inside, Some(HELD), Some(version)),
            fixture.inside.clone(),
        ));
    }

    for (label, uri, named) in rows {
        let expected = absent(DATASET_KIND, &named);
        let (status, answer) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {answer}"
        );
        assert_eq!(
            error_message(&answer),
            expected,
            "case {label:?}: with the workspace held the refusal must name the dataset \
             ({expected:?}), got {answer}"
        );
    }

    // The listing of a workspace this service holds is served, and reports the
    // one dataset created inside it.
    let (status, listed) = call(&router, "GET", &list_uri(Some(HELD)), None).await;
    assert_eq!(
        status, 200,
        "ListDatasets of the held workspace: expected 200, got {status} {listed}"
    );
    let ids: Vec<&str> = listed["datasetSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["id"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec![fixture.inside.as_str()],
        "ListDatasets of the held workspace: expected exactly the dataset created in it, got \
         {listed}"
    );

    // The dataset outside every workspace reads back carrying no `workspaceName`
    // of its own.
    let (status, described) = call(
        &router,
        "GET",
        &describe_uri(&fixture.outside, None, None),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeDataset of the dataset outside every workspace: expected 200, got {status} \
         {described}"
    );
    assert!(
        described["workspaceName"].is_null(),
        "DescribeDataset of the dataset outside every workspace: expected no workspaceName, got \
         {described}"
    );
}

/// Every refusal above is a READ that refused: none of them removed, moved or
/// rewrote the dataset it named, so the two datasets report the same thing
/// afterwards as before.
#[tokio::test]
async fn the_refusals_leave_both_datasets_exactly_as_they_were() {
    let router = router();
    let fixture = fixture(&router).await;

    let (_, before_inside) = call(
        &router,
        "GET",
        &describe_uri(&fixture.inside, Some(HELD), None),
        None,
    )
    .await;
    let (_, before_outside) = call(
        &router,
        "GET",
        &describe_uri(&fixture.outside, None, None),
        None,
    )
    .await;

    for ghost in ghost_workspaces() {
        for dataset in [fixture.inside.as_str(), fixture.outside.as_str(), UNKNOWN_DATASET] {
            for version in VERSIONS {
                call(
                    &router,
                    "GET",
                    &describe_uri(dataset, Some(&ghost), version),
                    None,
                )
                .await;
            }
        }
        call(&router, "GET", &list_uri(Some(&ghost)), None).await;
    }
    for version in UNHELD_VERSIONS {
        call(
            &router,
            "GET",
            &describe_uri(&fixture.inside, Some(HELD), Some(version)),
            None,
        )
        .await;
    }

    let (status, after_inside) = call(
        &router,
        "GET",
        &describe_uri(&fixture.inside, Some(HELD), None),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the dataset of the held workspace after every refusal: got {status} {after_inside}"
    );
    assert_eq!(
        stable_projection(&after_inside),
        stable_projection(&before_inside),
        "the dataset of the held workspace changed across the refusals: before \
         {before_inside}, after {after_inside}"
    );

    let (status, after_outside) = call(
        &router,
        "GET",
        &describe_uri(&fixture.outside, None, None),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the dataset outside every workspace after every refusal: got {status} {after_outside}"
    );
    assert_eq!(
        stable_projection(&after_outside),
        stable_projection(&before_outside),
        "the dataset outside every workspace changed across the refusals: before \
         {before_outside}, after {after_outside}"
    );
}
