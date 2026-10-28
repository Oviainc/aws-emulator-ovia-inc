// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the rule every create that puts the resource it mints
//! INSIDE a workspace follows, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: a create whose request names the workspace its new
//! resource belongs to resolves that name through the workspace registry
//! `CreateWorkspace` writes, so a name no workspace holds is the modelled
//! `ResourceNotFoundException` (HTTP 404) naming the workspace and stores nothing,
//! and the very same request is accepted — and reads back — as soon as the
//! workspace is open.
//!
//! The rows span the creates of this service that name a workspace, whether the
//! name rides the path (`CreatePipeline`, `CreateTask`,
//! `CreateDatasetExportJob`) or the body (`CreateDataset`, `CreateApplication`),
//! crossed with the forms `WorkspaceName` admits (smithy_query query="operation"
//! operation="CreatePipeline": `@length(min: 1, max: 64)`, pattern
//! `^[a-zA-Z0-9_-]+$`) — each published bound of that length and a name mixing
//! every character class the pattern allows.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`) is the one the
//! asset suite already has, the workspace step is `support`'s and the container
//! task configuration is the task suite's; this suite reuses them rather than
//! growing a second copy.

use serde_json::{json, Value};

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    not_found_message, service_state, RESOURCE_WORKSPACE,
};

use crate::asset_lifecycle::{call, error_code, router_with_engine};
use crate::support::ensure_workspace;
use crate::task_lifecycle::{minimal_container, PROCESSING_TYPES, PROCESSING_UNITS};

/// A workspace this store really holds, open before every refusal below: what a
/// refusal is about is the workspace the request names, not an empty registry.
const BYSTANDER: &str = "bystander-workspace";

/// A `TimeSeriesId` of the single-id form, which a `ProcessingInput` names
/// without the store having to hold a dataset.
const SERIES: &str = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";

/// One create that puts the resource it mints inside the workspace its request
/// names.
struct Target {
    label: &'static str,
    /// The route the create addresses for `workspace`.
    uri: fn(&str) -> String,
    /// The body the create sends to mint `name` inside `workspace`.
    body: fn(&str, &str) -> Value,
    /// The status the model declares for an accepted create.
    success: u16,
    /// The route a read of what the create minted addresses, off the workspace
    /// and the create's own answer.
    read_uri: fn(&str, &Value) -> String,
}

/// Read a string member off a response body, or `""`.
fn member(body: &Value, name: &str) -> String {
    body[name].as_str().unwrap_or_default().to_string()
}

/// Every create of this service whose request names the workspace its new
/// resource belongs to, one row each, with the read that proves an accepted one
/// persisted.
///
/// `CreatePipeline`, `CreateTask` and `CreateDatasetExportJob` take the name as
/// an `@httpLabel` on `/workspaces/{workspaceName}/…`; `CreateDataset` and
/// `CreateApplication` carry it in the body (`POST /datasets`,
/// `POST /applications`), so the rule is about what the request NAMES rather
/// than about where the name rides.
fn targets() -> Vec<Target> {
    vec![
        Target {
            label: "CreatePipeline",
            uri: |workspace| format!("/workspaces/{workspace}/pipelines"),
            body: |_, name| json!({ "pipelineName": name, "computations": [] }),
            success: 200,
            read_uri: |workspace, created| {
                format!(
                    "/workspaces/{workspace}/pipelines/{}",
                    member(created, "pipelineName")
                )
            },
        },
        Target {
            label: "CreateTask",
            uri: |workspace| format!("/workspaces/{workspace}/tasks"),
            body: |_, name| {
                json!({
                    "taskName": name,
                    "taskConfiguration": minimal_container(
                        PROCESSING_TYPES[0],
                        PROCESSING_UNITS[0],
                    ),
                })
            },
            success: 200,
            read_uri: |workspace, created| {
                format!(
                    "/workspaces/{workspace}/tasks/{}",
                    member(created, "taskName")
                )
            },
        },
        Target {
            label: "CreateDatasetExportJob",
            uri: |workspace| format!("/workspaces/{workspace}/dataset-export-jobs"),
            body: |_, name| {
                json!({
                    "destinationS3Uri": format!("s3://sitewise-exports/{name}/"),
                    "errorReportLocation": { "s3Uri": "s3://sitewise-exports/errors/" },
                    "input": { "timeseries": [{ "timeSeriesId": SERIES }] },
                })
            },
            success: 202,
            read_uri: |workspace, created| {
                format!(
                    "/workspaces/{workspace}/dataset-export-jobs/{}",
                    member(created, "jobId")
                )
            },
        },
        Target {
            label: "CreateDataset",
            uri: |_| "/datasets".to_string(),
            body: |workspace, name| {
                json!({
                    "datasetName": name,
                    // "Required for session and curated datasets": a dataset of
                    // this type is one that lives in a workspace.
                    "datasetType": "SESSION",
                    "workspaceName": workspace,
                    "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
                })
            },
            success: 202,
            read_uri: |workspace, created| {
                format!(
                    "/datasets/{}?workspaceName={workspace}",
                    member(created, "datasetId")
                )
            },
        },
        Target {
            label: "CreateApplication",
            uri: |_| "/applications".to_string(),
            body: |workspace, name| {
                json!({
                    "workspaceName": workspace,
                    "name": name,
                    "idcInstanceArn":
                        "arn:aws:sso::123456789012:instance/ssoins-1111111111111111",
                })
            },
            success: 200,
            read_uri: |workspace, created| {
                format!(
                    "/workspaces/{workspace}/applications/{}",
                    member(created, "id")
                )
            },
        },
    ]
}

/// The workspace names the rows are run over: each published bound of
/// `WorkspaceName`'s `@length(min: 1, max: 64)`, a name mixing every character
/// class its pattern `^[a-zA-Z0-9_-]+$` admits, and an all-digit name.
fn workspace_names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest name the length admits", "w".to_string()),
        ("the longest name the length admits", "w".repeat(64)),
        (
            "every character class the pattern admits",
            "Plant_7-North".to_string(),
        ),
        ("an all-digit name", "0123456789".to_string()),
    ]
}

/// How many resources of each workspace-scoped kind the store holds: the only way
/// to see that a refused create wrote nothing at all, since a refused create
/// leaves no id to read back.
fn held(engine: &engine::InMemoryEngine) -> [usize; 5] {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let store = state.lock();
    [
        store.pipelines.len(),
        store.tasks.len(),
        store.dataset_export_jobs.len(),
        store.datasets.len(),
        store.applications.len(),
    ]
}

#[tokio::test]
async fn a_create_naming_a_workspace_the_registry_does_not_hold_is_the_modelled_not_found() {
    let (router, engine) = router_with_engine();
    ensure_workspace(&router, BYSTANDER).await;
    let before = held(&engine);

    for target in targets() {
        for (at, (name_case, workspace)) in workspace_names().iter().enumerate() {
            let case = format!("{} / {name_case}", target.label);
            let name = format!("probe-{at}");
            let (status, refused) = call(
                &router,
                "POST",
                &(target.uri)(workspace),
                Some((target.body)(workspace, &name)),
            )
            .await;

            assert_eq!(
                (status, error_code(&refused)),
                (404, "ResourceNotFoundException"),
                "case {case:?}: a create naming a workspace no create opened must be the \
                 modelled not-found refusal, got {status} {refused}"
            );
            assert_eq!(
                refused["message"].as_str().unwrap_or(""),
                not_found_message(RESOURCE_WORKSPACE, workspace),
                "case {case:?}: the refusal must name the workspace that is absent, \
                 got {refused}"
            );
            assert_eq!(
                held(&engine),
                before,
                "case {case:?}: a refused create must store nothing, the store now holds {:?}",
                held(&engine)
            );
        }
    }
}

#[tokio::test]
async fn the_same_create_is_accepted_once_the_workspace_is_open_and_reads_back() {
    for target in targets() {
        // A router per target, so a workspace one target opened is not already
        // there for the next: the names these rows run over are the published
        // bounds of `WorkspaceName`, so they cannot be made unique instead.
        let (router, engine) = router_with_engine();
        for (at, (name_case, workspace)) in workspace_names().iter().enumerate() {
            let case = format!("{} / {name_case}", target.label);
            let name = format!("probe-{at}");
            let request = (target.body)(workspace, &name);

            // The same request, before and after `CreateWorkspace` puts the name
            // in the registry: what changes the answer is the workspace and
            // nothing in the request.
            let (status, refused) = call(
                &router,
                "POST",
                &(target.uri)(workspace),
                Some(request.clone()),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the not-found refusal before the workspace is open, \
                 got {status} {refused}"
            );

            let before = held(&engine);
            ensure_workspace(&router, workspace).await;
            let (status, created) = call(
                &router,
                "POST",
                &(target.uri)(workspace),
                Some(request.clone()),
            )
            .await;
            assert_eq!(
                status,
                target.success,
                "case {case:?}: an open workspace must take the very same create, \
                 got {status} {created}"
            );
            assert_ne!(
                held(&engine),
                before,
                "case {case:?}: an accepted create must store the resource it minted, \
                 the store holds {:?}",
                held(&engine)
            );

            // And what it minted is really persisted: it reads back inside the
            // workspace it was created in.
            let read_uri = (target.read_uri)(workspace, &created);
            let (status, read) = call(&router, "GET", &read_uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the created resource must read back at {read_uri}, \
                 got {status} {read}"
            );
        }
    }
}
