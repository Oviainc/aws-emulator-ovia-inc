// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteWorkspace`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a delete removes EXACTLY the workspace the request names, together with
//!   everything keyed off it that is addressable only inside it (the queries and
//!   searches it ran), and answers that workspace's lifecycle state — so a later
//!   read of it resolves to nothing, its ARN stops carrying tags, and every other
//!   workspace, with its own queries and searches, is left alone;
//! * a delete is refused while the workspace still contains a resource a caller
//!   addresses on its own — a dataset, a pipeline, a task or an application — and
//!   the refused call leaves the workspace and its contents exactly as they were;
//!   once those contents are deleted, the same delete is accepted;
//! * a name that resolves to no stored workspace — never created, or already
//!   deleted — is the modelled 404 naming the identifier the REQUEST supplied, and
//!   writes nothing.
//!
//! The rows differ along what the rules generalise over: the workspace's name at
//! both ends of its modelled length, both `EncryptionType` values, a description
//! and tags present or absent, how many queries and searches ran in it, whether
//! the delete carries a `clientToken`, whether the workspace had settled the
//! transition its create owes, and — for the refusal — which kind of content it
//! still holds and how many.
//!
//! The HTTP harness (`call`, `error_code`, `router_with_engine`) is the one the
//! asset suite already has, and the workspace helpers are the ones the workspace
//! lifecycle suite already has; this suite reuses both rather than growing a
//! second copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::asset_lifecycle::{call, error_code, router_with_engine};
use crate::workspace_lifecycle::{
    create as create_workspace, default_encryption, describe as describe_workspace, expected_arn,
    kms_encryption, settle, state,
};

/// The workspace every suite keeps beside the one it deletes, so each assertion
/// about what a delete removed is measured against what it had to leave alone.
const NEIGHBOUR: &str = "the-workspace-that-stays";

/// `DELETE /workspaces/{workspaceName}`, with `clientToken` on the query string
/// as the model binds it (smithy_query query="operation"
/// operation="DeleteWorkspace").
async fn delete_workspace(
    router: &MultiServiceRouter,
    name: &str,
    client_token: Option<&str>,
) -> (u16, Value) {
    let uri = match client_token {
        Some(token) => format!("/workspaces/{name}?clientToken={token}"),
        None => format!("/workspaces/{name}"),
    };
    call(router, "DELETE", &uri, None).await
}

/// `ListTagsForResource` on `arn` — the reader that proves a deleted workspace's
/// ARN stops resolving.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags?resourceArn={arn}"), None).await
}

/// `StartQuery` in `workspace`, and the `queryId` it minted.
async fn start_query(router: &MultiServiceRouter, workspace: &str, statement: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/queries"),
        Some(json!({ "queryStatement": statement })),
    )
    .await;
    assert_eq!(
        status, 202,
        "StartQuery in {workspace}: got {status} {body}"
    );
    body["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQuery in {workspace}: no queryId in {body}"))
        .to_string()
}

/// `DescribeQuery` on a query of `workspace`.
async fn describe_query(
    router: &MultiServiceRouter,
    workspace: &str,
    query_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/workspaces/{workspace}/queries/{query_id}"),
        None,
    )
    .await
}

/// `StartSearch` in `workspace`, and the `searchId` it minted.
async fn start_search(router: &MultiServiceRouter, workspace: &str, statement: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/searches"),
        Some(json!({ "queryStatement": statement })),
    )
    .await;
    assert_eq!(
        status, 202,
        "StartSearch in {workspace}: got {status} {body}"
    );
    body["searchId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartSearch in {workspace}: no searchId in {body}"))
        .to_string()
}

/// `DescribeSearch` on a search of `workspace`.
async fn describe_search(
    router: &MultiServiceRouter,
    workspace: &str,
    search_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/workspaces/{workspace}/searches/{search_id}"),
        None,
    )
    .await
}

/// `CreateDataset` for a `CURATED` dataset of `workspace` — a dataset that is
/// "created in a workspace", so the workspace CONTAINS it.
async fn create_dataset(router: &MultiServiceRouter, workspace: &str, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/datasets",
        Some(json!({
            "datasetName": name,
            "datasetType": "CURATED",
            "workspaceName": workspace,
            "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateDataset {name} in {workspace}: got {status} {body}"
    );
    body["datasetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDataset {name}: no datasetId in {body}"))
        .to_string()
}

/// `CreatePipeline` in `workspace`, with no compute nodes at all — the smallest
/// pipeline the model admits, so the row is about the pipeline existing and
/// nothing else.
async fn create_pipeline(router: &MultiServiceRouter, workspace: &str, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/pipelines"),
        Some(json!({ "pipelineName": name, "computations": [] })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePipeline {name} in {workspace}: got {status} {body}"
    );
}

/// `DeletePipeline` in `workspace`.
async fn delete_pipeline(router: &MultiServiceRouter, workspace: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/workspaces/{workspace}/pipelines/{name}"),
        None,
    )
    .await
}

/// `CreateTask` in `workspace`, carrying only the members
/// `ContainerTaskConfiguration` requires.
async fn create_task(router: &MultiServiceRouter, workspace: &str, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/workspaces/{workspace}/tasks"),
        Some(json!({
            "taskName": name,
            "taskConfiguration": {
                "containerTaskConfiguration": {
                    "ecrUri": "123456789012.dkr.ecr.us-west-2.amazonaws.com/analytics:v1",
                    "taskExecutionRole": "arn:aws:iam::123456789012:role/SiteWiseTaskRole",
                    "processingType": "GENERIC_COMPUTE_PROCESSING",
                    "processingUnit": "UNITS_2",
                }
            },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateTask {name} in {workspace}: got {status} {body}"
    );
}

/// `CreateApplication` in `workspace`.
async fn create_application(router: &MultiServiceRouter, workspace: &str, name: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/applications",
        Some(json!({
            "workspaceName": workspace,
            "name": name,
            "idcInstanceArn": "arn:aws:sso:::instance/ssoins-1111111111111111",
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateApplication {name} in {workspace}: got {status} {body}"
    );
}

/// A `DescribeWorkspace` body without the lifecycle `workspaceStatus` the clock
/// moves on its own, so two reads taken at different instants are comparable.
fn without_status(body: &Value) -> Value {
    let mut copy = body.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("workspaceStatus");
    }
    copy
}

/// The names of every workspace the store holds, sorted.
fn stored_workspace_names(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let mut names: Vec<String> = state.lock().workspaces.keys().cloned().collect();
    names.sort();
    names
}

/// How many queries the store holds for `workspace` — the records a workspace's
/// delete has to take with it, which no reader enumerates.
fn queries_held(engine: &engine::InMemoryEngine, workspace: &str) -> usize {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let store = state.lock();
    store
        .queries
        .keys()
        .filter(|(held_in, _)| held_in == workspace)
        .count()
}

/// How many searches the store holds for `workspace`. See [`queries_held`].
fn searches_held(engine: &engine::InMemoryEngine, workspace: &str) -> usize {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let store = state.lock();
    store
        .searches
        .keys()
        .filter(|(held_in, _)| held_in == workspace)
        .count()
}

/// How many datasets, pipelines, tasks and applications the store holds in all —
/// what proves a refused delete removed none of them.
fn contents_held(engine: &engine::InMemoryEngine) -> (usize, usize, usize, usize) {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let store = state.lock();
    (
        store.datasets.len(),
        store.pipelines.len(),
        store.tasks.len(),
        store.applications.len(),
    )
}

/// One row of the delete table.
struct DeleteCase {
    label: &'static str,
    /// The workspace the row deletes. `WorkspaceName` is len 1..64, pattern
    /// `^[a-zA-Z0-9_-]+$` (smithy_query query="operation"
    /// operation="DeleteWorkspace"), so the rows span both ends of that length.
    name: String,
    description: Option<&'static str>,
    tags: Option<Value>,
    encryption: Value,
    /// How many queries ran in the workspace before the delete.
    queries: usize,
    /// How many searches ran in it.
    searches: usize,
    /// The `clientToken` the delete carries, if any: `ClientToken` is len 36..64,
    /// pattern `^\S{36,64}$`.
    client_token: Option<&'static str>,
    /// Whether the row waits for the transition the create owes before deleting,
    /// so the table covers a workspace deleted while it is still settling its
    /// create and one deleted after it has settled.
    settled: bool,
}

/// The rows.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "shortest-name-nothing-ran-in-it-still-creating",
            name: "w".to_string(),
            description: None,
            tags: None,
            encryption: default_encryption(),
            queries: 0,
            searches: 0,
            client_token: None,
            settled: false,
        },
        DeleteCase {
            label: "described-tagged-settled-one-query-and-one-search",
            name: "plant-floor".to_string(),
            description: Some("the north line"),
            tags: Some(json!({ "line": "north", "env": "test" })),
            encryption: default_encryption(),
            queries: 1,
            searches: 1,
            client_token: Some("11111111-2222-3333-4444-555555555555"),
            settled: true,
        },
        DeleteCase {
            label: "customer-managed-key-with-several-queries-and-searches",
            name: "Keyed_Workspace-2".to_string(),
            description: Some("under a key of its own"),
            tags: Some(json!({ "owner": "ops" })),
            encryption: kms_encryption("1234abcd-12ab-34cd-56ef-1234567890ab"),
            queries: 3,
            searches: 2,
            client_token: None,
            settled: true,
        },
        DeleteCase {
            label: "longest-name-with-a-client-token-while-still-creating",
            name: "w".repeat(64),
            description: None,
            tags: Some(json!({ "only": "one" })),
            encryption: default_encryption(),
            queries: 1,
            searches: 0,
            client_token: Some("0123456789abcdef0123456789abcdef0123"),
            settled: false,
        },
    ]
}

/// The create body a row sends.
fn create_body(case: &DeleteCase) -> Value {
    let mut body = json!({
        "workspaceName": case.name,
        "encryptionConfiguration": case.encryption,
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some(description) = case.description {
        object.insert("workspaceDescription".to_string(), json!(description));
    }
    if let Some(tags) = case.tags.as_ref() {
        object.insert("tags".to_string(), tags.clone());
    }
    body
}

#[tokio::test]
async fn a_delete_removes_the_named_workspace_with_what_ran_in_it_and_nothing_else() {
    for case in delete_cases() {
        let (router, engine) = router_with_engine();

        let (status, created) = create_workspace(&router, create_body(&case)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateWorkspace got {status} {created}",
            case.label
        );
        let (status, neighbour_created) = create_workspace(
            &router,
            json!({
                "workspaceName": NEIGHBOUR,
                "encryptionConfiguration": default_encryption(),
                "tags": { "keep": "me" },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the neighbour's create got {status} {neighbour_created}",
            case.label
        );

        // What ran in each workspace: the records a delete has to take with the
        // workspace they belong to, and leave alone for the one it does not name.
        let mut doomed_queries = Vec::new();
        for at in 0..case.queries {
            doomed_queries
                .push(start_query(&router, &case.name, "SELECT asset_name FROM asset").await);
            assert_eq!(
                doomed_queries.len(),
                at + 1,
                "case {:?}: every query must mint an id",
                case.label
            );
        }
        let mut doomed_searches = Vec::new();
        for _ in 0..case.searches {
            doomed_searches.push(start_search(&router, &case.name, "boiler").await);
        }
        let neighbour_query =
            start_query(&router, NEIGHBOUR, "SELECT asset_name FROM asset").await;
        let neighbour_search = start_search(&router, NEIGHBOUR, "turbine").await;

        if case.settled {
            tokio::time::sleep(settle()).await;
        }

        // The workspace reads back before the delete, so what the delete removed is
        // measured against what was really stored.
        let (status, before) = describe_workspace(&router, &case.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeWorkspace before the delete got {status} {before}",
            case.label
        );
        let arn = expected_arn(&case.name);
        assert_eq!(
            before["workspaceArn"], json!(arn),
            "case {:?}: unexpected workspaceArn in {before}",
            case.label
        );
        let (status, tags_before) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the workspace's ARN must resolve before the delete, \
             got {status} {tags_before}",
            case.label
        );
        let neighbour_before = describe_workspace(&router, NEIGHBOUR).await.1;

        let (status, answer) = delete_workspace(&router, &case.name, case.client_token).await;
        assert_eq!(
            status, 200,
            "case {:?}: DeleteWorkspace expected 200, got {status} {answer}",
            case.label
        );
        // "The status of the workspace after the deletion request, which is
        // DELETING when the operation returns."
        assert_eq!(
            state(&answer), "DELETING",
            "case {:?}: a delete reports DELETING, got {answer}",
            case.label
        );

        // The workspace is gone from its reader and from the store.
        let (status, after) = describe_workspace(&router, &case.name).await;
        assert_eq!(
            (status, error_code(&after)),
            (404, "ResourceNotFoundException"),
            "case {:?}: DescribeWorkspace after the delete must be a modelled 404, \
             got {status} {after}",
            case.label
        );
        assert!(
            after["message"]
                .as_str()
                .unwrap_or_default()
                .contains(&case.name),
            "case {:?}: the refusal names the workspace the request addressed, got {after}",
            case.label
        );
        assert_eq!(
            stored_workspace_names(&engine),
            vec![NEIGHBOUR.to_string()],
            "case {:?}: the store must hold only the neighbour workspace",
            case.label
        );

        // Its ARN stops resolving: the tags it carried went with the record.
        let (status, tags_after) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, error_code(&tags_after)),
            (404, "ResourceNotFoundException"),
            "case {:?}: the deleted workspace's ARN must stop resolving, \
             got {status} {tags_after}",
            case.label
        );

        // Everything that ran in it went with it, and nothing that ran elsewhere did.
        assert_eq!(
            (
                queries_held(&engine, &case.name),
                searches_held(&engine, &case.name)
            ),
            (0, 0),
            "case {:?}: the delete must take the workspace's queries and searches with it",
            case.label
        );
        assert_eq!(
            (
                queries_held(&engine, NEIGHBOUR),
                searches_held(&engine, NEIGHBOUR)
            ),
            (1, 1),
            "case {:?}: the neighbour's query and search must survive",
            case.label
        );
        for query in &doomed_queries {
            let (status, body) = describe_query(&router, &case.name, query).await;
            assert_eq!(
                (status, error_code(&body)),
                (404, "ResourceNotFoundException"),
                "case {:?}: query {query} of the deleted workspace must be gone, \
                 got {status} {body}",
                case.label
            );
        }
        for search in &doomed_searches {
            let (status, body) = describe_search(&router, &case.name, search).await;
            assert_eq!(
                (status, error_code(&body)),
                (404, "ResourceNotFoundException"),
                "case {:?}: search {search} of the deleted workspace must be gone, \
                 got {status} {body}",
                case.label
            );
        }
        let (status, body) = describe_query(&router, NEIGHBOUR, &neighbour_query).await;
        assert_eq!(
            status, 200,
            "case {:?}: the neighbour's query must still read back, got {status} {body}",
            case.label
        );
        let (status, body) = describe_search(&router, NEIGHBOUR, &neighbour_search).await;
        assert_eq!(
            status, 200,
            "case {:?}: the neighbour's search must still read back, got {status} {body}",
            case.label
        );

        // The neighbour workspace itself is untouched.
        let (status, neighbour_after) = describe_workspace(&router, NEIGHBOUR).await;
        assert_eq!(
            status, 200,
            "case {:?}: the neighbour workspace expected 200, got {status} {neighbour_after}",
            case.label
        );
        assert_eq!(
            without_status(&neighbour_after),
            without_status(&neighbour_before),
            "case {:?}: the delete must leave the neighbour workspace exactly as it was, \
             got {neighbour_after}",
            case.label
        );
    }
}

/// One row of the still-has-contents table: how many of each kind the workspace
/// holds, and the contents the refusal must name.
struct ContentCase {
    label: &'static str,
    datasets: usize,
    pipelines: usize,
    tasks: usize,
    applications: usize,
    /// The contents the refusal states, built here as an INDEPENDENT oracle: the
    /// kinds the service names in its refusals, in the order a workspace's
    /// contents are reported, each with how many of it the workspace holds.
    expected: &'static str,
}

/// The rows: each kind a workspace contains on its own, one of them in the
/// plural, and every kind at once.
fn content_cases() -> Vec<ContentCase> {
    vec![
        ContentCase {
            label: "one-dataset",
            datasets: 1,
            pipelines: 0,
            tasks: 0,
            applications: 0,
            expected: "1 Dataset",
        },
        ContentCase {
            label: "one-pipeline",
            datasets: 0,
            pipelines: 1,
            tasks: 0,
            applications: 0,
            expected: "1 Pipeline",
        },
        ContentCase {
            label: "two-tasks",
            datasets: 0,
            pipelines: 0,
            tasks: 2,
            applications: 0,
            expected: "2 Tasks",
        },
        ContentCase {
            label: "one-application",
            datasets: 0,
            pipelines: 0,
            tasks: 0,
            applications: 1,
            expected: "1 Application",
        },
        ContentCase {
            label: "every-kind-at-once",
            datasets: 2,
            pipelines: 1,
            tasks: 1,
            applications: 3,
            expected: "2 Datasets, 1 Pipeline, 1 Task, 3 Applications",
        },
    ]
}

/// Fill `workspace` with the contents `case` names.
async fn fill(router: &MultiServiceRouter, workspace: &str, case: &ContentCase) {
    for at in 0..case.datasets {
        create_dataset(router, workspace, &format!("dataset-{at}")).await;
    }
    for at in 0..case.pipelines {
        create_pipeline(router, workspace, &format!("pipeline-{at}")).await;
    }
    for at in 0..case.tasks {
        create_task(router, workspace, &format!("task-{at}")).await;
    }
    for at in 0..case.applications {
        create_application(router, workspace, &format!("application-{at}")).await;
    }
}

#[tokio::test]
async fn a_delete_is_refused_while_the_workspace_still_contains_resources() {
    const DOOMED: &str = "the-full-workspace";

    for case in content_cases() {
        let (router, engine) = router_with_engine();
        let (status, created) = create_workspace(
            &router,
            json!({
                "workspaceName": DOOMED,
                "encryptionConfiguration": default_encryption(),
                "tags": { "stage": "test" },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateWorkspace got {status} {created}",
            case.label
        );
        fill(&router, DOOMED, &case).await;
        // The contents are created inside the workspace, so the refusal below is
        // about what the workspace holds and not about the clock: every row waits
        // for the transition the create owes.
        tokio::time::sleep(settle()).await;

        let before = describe_workspace(&router, DOOMED).await.1;
        let held_before = contents_held(&engine);

        let (status, refused) = delete_workspace(&router, DOOMED, None).await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "InvalidRequestException"),
            "case {:?}: a delete of a workspace that still has contents must be the \
             modelled 400, got {status} {refused}",
            case.label
        );
        let message = refused["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(case.expected),
            "case {:?}: the refusal must name {:?}, got {refused}",
            case.label,
            case.expected
        );
        assert!(
            message.contains(DOOMED),
            "case {:?}: the refusal must name the workspace it refuses, got {refused}",
            case.label
        );

        // The refused call wrote nothing: the workspace and every one of its
        // contents stand exactly as they did.
        let (status, after) = describe_workspace(&router, DOOMED).await;
        assert_eq!(
            status, 200,
            "case {:?}: the workspace must survive a refused delete, got {status} {after}",
            case.label
        );
        assert_eq!(
            without_status(&after),
            without_status(&before),
            "case {:?}: a refused delete must leave the workspace exactly as it was, \
             got {after}",
            case.label
        );
        assert_eq!(
            stored_workspace_names(&engine),
            vec![DOOMED.to_string()],
            "case {:?}: a refused delete must leave the workspace in the store",
            case.label
        );
        assert_eq!(
            contents_held(&engine),
            held_before,
            "case {:?}: a refused delete must remove none of the contents",
            case.label
        );
    }
}

#[tokio::test]
async fn a_workspace_whose_contents_have_been_deleted_can_then_be_deleted() {
    const DOOMED: &str = "emptied-workspace";

    let (router, engine) = router_with_engine();
    let (status, created) = create_workspace(
        &router,
        json!({
            "workspaceName": DOOMED,
            "encryptionConfiguration": default_encryption(),
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateWorkspace got {status} {created}");

    let pipelines = ["first-pipeline", "second-pipeline"];
    for pipeline in pipelines {
        create_pipeline(&router, DOOMED, pipeline).await;
    }
    tokio::time::sleep(settle()).await;

    // Both pipelines stand, so the delete is refused and names them.
    let (status, refused) = delete_workspace(&router, DOOMED, None).await;
    assert_eq!(
        (status, error_code(&refused)),
        (400, "InvalidRequestException"),
        "a workspace holding two pipelines must refuse the delete, got {status} {refused}"
    );
    assert!(
        refused["message"]
            .as_str()
            .unwrap_or_default()
            .contains("2 Pipelines"),
        "the refusal must name what the workspace holds, got {refused}"
    );

    // One of the two deleted: the workspace still holds the other, so the delete
    // is still refused — and now names one pipeline rather than two.
    let (status, deleted) = delete_pipeline(&router, DOOMED, pipelines[0]).await;
    assert_eq!(
        status, 200,
        "DeletePipeline {} got {status} {deleted}",
        pipelines[0]
    );
    let (status, refused) = delete_workspace(&router, DOOMED, None).await;
    assert_eq!(
        (status, error_code(&refused)),
        (400, "InvalidRequestException"),
        "a workspace holding one pipeline must still refuse the delete, got {status} {refused}"
    );
    assert!(
        refused["message"]
            .as_str()
            .unwrap_or_default()
            .contains("1 Pipeline"),
        "the refusal must name what the workspace still holds, got {refused}"
    );

    // Both gone: the same delete is accepted, and the workspace stops resolving.
    let (status, deleted) = delete_pipeline(&router, DOOMED, pipelines[1]).await;
    assert_eq!(
        status, 200,
        "DeletePipeline {} got {status} {deleted}",
        pipelines[1]
    );
    let (status, answer) = delete_workspace(&router, DOOMED, None).await;
    assert_eq!(
        status, 200,
        "an emptied workspace must be deletable, got {status} {answer}"
    );
    assert_eq!(
        state(&answer), "DELETING",
        "the accepted delete reports DELETING, got {answer}"
    );
    let (status, after) = describe_workspace(&router, DOOMED).await;
    assert_eq!(
        (status, error_code(&after)),
        (404, "ResourceNotFoundException"),
        "the deleted workspace must stop resolving, got {status} {after}"
    );
    assert!(
        stored_workspace_names(&engine).is_empty(),
        "the store must hold no workspace at all, got {:?}",
        stored_workspace_names(&engine)
    );
}

#[tokio::test]
async fn a_delete_of_a_workspace_this_service_does_not_hold_is_a_modelled_not_found() {
    const HELD: &str = "Plant1";

    let (router, engine) = router_with_engine();
    let (status, created) = create_workspace(
        &router,
        json!({
            "workspaceName": HELD,
            "encryptionConfiguration": default_encryption(),
            "tags": { "keep": "me" },
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateWorkspace got {status} {created}");
    let query = start_query(&router, HELD, "SELECT asset_name FROM asset").await;
    let before = describe_workspace(&router, HELD).await.1;

    // A delete that lands first, so the SECOND delete of that name is a row of
    // this table: a delete carrying no token, or one of its own, is judged on the
    // state it finds — only a retry carrying the token the first delete SPENT
    // replays that call's answer.
    let (status, created) = create_workspace(
        &router,
        json!({
            "workspaceName": "twice-deleted",
            "encryptionConfiguration": default_encryption(),
        }),
    )
    .await;
    assert_eq!(status, 200, "the second create got {status} {created}");
    let (status, answer) = delete_workspace(&router, "twice-deleted", None).await;
    assert_eq!(status, 200, "the first delete got {status} {answer}");

    // Names no create ever minted — including one that differs from the held name
    // only in case, since a workspace is named exactly — and the name the delete
    // above already removed.
    let probes = [
        ("never-created", "never-created"),
        ("the-held-name-in-another-case", "plant1"),
        ("the-shortest-name", "z"),
        ("the-longest-name", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"),
        ("already-deleted", "twice-deleted"),
    ];

    for (label, name) in probes {
        assert_ne!(name, HELD, "case {label:?}: the probe must not be the held name");
        for token in [None, Some("3f2a1b0c-9d8e-7f6a-5b4c-3d2e1f0a9b8c-tok")] {
            let (status, refused) = delete_workspace(&router, name, token).await;
            assert_eq!(
                (status, error_code(&refused)),
                (404, "ResourceNotFoundException"),
                "case {label:?} (token {token:?}): a delete of an absent workspace must be \
                 a modelled 404, got {status} {refused}"
            );
            let message = refused["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(name) && !message.contains(HELD),
                "case {label:?}: the refusal must name the requested name only, got {refused}"
            );
        }
    }

    // Every refusal wrote nothing: the held workspace and what ran in it are
    // exactly as they were.
    let (status, after) = describe_workspace(&router, HELD).await;
    assert_eq!(status, 200, "the held workspace got {status} {after}");
    assert_eq!(
        without_status(&after),
        without_status(&before),
        "a refused delete must leave the held workspace exactly as it was, got {after}"
    );
    assert_eq!(
        stored_workspace_names(&engine),
        vec![HELD.to_string()],
        "the store must hold exactly the one workspace that was never deleted"
    );
    let (status, body) = describe_query(&router, HELD, &query).await;
    assert_eq!(
        status, 200,
        "a refused delete must leave the held workspace's query readable, got {status} {body}"
    );
}
