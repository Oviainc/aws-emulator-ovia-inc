// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the `clientToken` the workspace, pipeline and task
//! writes take, driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a real
//! client sees.
//!
//! The rule these suites pin: a `clientToken` is spent by ONE request of an
//! operation. A repeat of the SAME request under that token is a RETRY of that
//! call — answered from what the first call left behind, writing nothing
//! further. A DIFFERENT request reusing the token is refused
//! `ConflictingOperationException` (409) naming the resource the first call
//! acted on, and changes nothing: it mints no second resource and performs no
//! second write. The token names the CALL that spent it, so a request
//! addressing another resource entirely is still that refusal.
//!
//! The rows span what the rule generalises over: the operation (the two
//! workspace-scoped creates, the workspace create, the workspace update) and
//! the member the second request differs in (the resource's own name, the
//! workspace it is created in, a description, a payload — the DAG a pipeline
//! carries, the configuration a task carries — and a tag).
//!
//! The HTTP harness (`call`, `router`, `error_code`) is `asset_lifecycle`'s, the
//! token helpers (`client_token`, `with_token`) are
//! `client_token_idempotency`'s, and the workspace / pipeline / task routes and
//! body builders are their own suites': reused rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, router};
use crate::client_token_idempotency::{client_token, with_token};
use crate::pipeline_lifecycle::{node, pipelines_uri};
use crate::task_lifecycle::{minimal_container, tasks_uri};
use crate::workspace_lifecycle::{create_settled, default_encryption, settle};

/// `POST|GET /workspaces`, as `OPERATIONS.md` reports the route.
const WORKSPACES_URI: &str = "/workspaces";

/// The resource kind a row mints, and how the API reports one of them.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Pipeline,
    Task,
    Workspace,
}

impl Kind {
    /// The member a listing of this kind reports its summaries under.
    fn summaries(self) -> &'static str {
        match self {
            Kind::Pipeline => "pipelineSummaries",
            Kind::Task => "taskSummaries",
            Kind::Workspace => "workspaceSummaries",
        }
    }

    /// The member one of those summaries carries its name under —
    /// `WorkspaceSummary` spells it `name`, the other two spell it out.
    fn name_member(self) -> &'static str {
        match self {
            Kind::Pipeline => "pipelineName",
            Kind::Task => "taskName",
            Kind::Workspace => "name",
        }
    }

    /// The member the create's answer carries the minted resource's name under —
    /// what a conflict with that resource reports as `resourceId`.
    fn id_member(self) -> &'static str {
        match self {
            Kind::Pipeline => "pipelineName",
            Kind::Task => "taskName",
            Kind::Workspace => "workspaceName",
        }
    }

    /// The member that answer carries the minted ARN under.
    fn arn_member(self) -> &'static str {
        match self {
            Kind::Pipeline => "pipelineArn",
            Kind::Task => "taskArn",
            Kind::Workspace => "workspaceArn",
        }
    }

    /// The member that answer carries the resource's lifecycle status under.
    fn status_member(self) -> &'static str {
        match self {
            Kind::Pipeline | Kind::Task => "status",
            Kind::Workspace => "workspaceStatus",
        }
    }
}

/// One row: a create that spends a token, the DIFFERENT create that reuses it,
/// and what the store must report once that second create has been refused.
struct Row {
    case: &'static str,
    /// The seed this row's token is built from: non-whitespace, as
    /// `ClientToken`'s `^\S{36,64}$` requires.
    token_seed: &'static str,
    kind: Kind,
    /// The workspaces the row needs open before the first create lands.
    workspaces: Vec<&'static str>,
    /// Where the first create goes, and the body it carries (without a token).
    path: String,
    body: Value,
    /// Where the DIFFERENT create goes — another workspace, for the row that
    /// differs in the workspace it addresses — and the body it carries.
    other_path: String,
    other_body: Value,
    /// Every listing that must still report exactly the names given: its path,
    /// and those names.
    listings: Vec<(String, Vec<&'static str>)>,
}

/// A pipeline create body: a DAG of `nodes`, plus whatever `extra` carries.
fn pipeline_body(name: &str, nodes: Value, extra: Value) -> Value {
    let mut body = json!({ "pipelineName": name, "computations": nodes });
    merge(&mut body, extra);
    body
}

/// A task create body carrying the members `CreateTask` requires, plus `extra`.
fn task_body(name: &str, processing_unit: &str, extra: Value) -> Value {
    let mut body = json!({
        "taskName": name,
        "taskConfiguration": minimal_container("GENERIC_COMPUTE_PROCESSING", processing_unit),
    });
    merge(&mut body, extra);
    body
}

/// A workspace create body, plus `extra`.
fn workspace_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "workspaceName": name,
        "encryptionConfiguration": default_encryption(),
    });
    merge(&mut body, extra);
    body
}

/// Fold the members of `extra` into `body`.
pub(crate) fn merge(body: &mut Value, extra: Value) {
    let object = body.as_object_mut().expect("an object body");
    for (member, value) in extra.as_object().expect("an object of extra members") {
        object.insert(member.clone(), value.clone());
    }
}

/// The rows, one per (operation, differing member) pair.
fn rows() -> Vec<Row> {
    let held = "plant-a";
    let other = "plant-b";
    vec![
        Row {
            case: "CreatePipeline, another pipelineName",
            token_seed: "create-pipeline-another-name",
            kind: Kind::Pipeline,
            workspaces: vec![held],
            path: pipelines_uri(held),
            body: pipeline_body("p1", json!([]), json!({})),
            other_path: pipelines_uri(held),
            other_body: pipeline_body("p2", json!([]), json!({})),
            listings: vec![(pipelines_uri(held), vec!["p1"])],
        },
        Row {
            case: "CreatePipeline, another workspace",
            token_seed: "create-pipeline-another-workspace",
            kind: Kind::Pipeline,
            workspaces: vec![held, other],
            path: pipelines_uri(held),
            body: pipeline_body("shared-name", json!([]), json!({})),
            other_path: pipelines_uri(other),
            other_body: pipeline_body("shared-name", json!([]), json!({})),
            listings: vec![
                (pipelines_uri(held), vec!["shared-name"]),
                (pipelines_uri(other), vec![]),
            ],
        },
        Row {
            case: "CreatePipeline, another DAG",
            token_seed: "create-pipeline-another-dag",
            kind: Kind::Pipeline,
            workspaces: vec![held],
            path: pipelines_uri(held),
            body: pipeline_body(
                "dag",
                json!([node("extract", "extract-task", &[])]),
                json!({ "description": "one node" }),
            ),
            other_path: pipelines_uri(held),
            other_body: pipeline_body(
                "dag",
                json!([
                    node("extract", "extract-task", &[]),
                    node("load", "load-task", &["extract"]),
                ]),
                json!({ "description": "one node" }),
            ),
            listings: vec![(pipelines_uri(held), vec!["dag"])],
        },
        Row {
            case: "CreatePipeline, another environment",
            token_seed: "create-pipeline-another-environment",
            kind: Kind::Pipeline,
            workspaces: vec![held],
            path: pipelines_uri(held),
            body: pipeline_body(
                "env",
                json!([]),
                json!({ "environmentVariables": { "LOG_LEVEL": "debug", "SHARD": "2" } }),
            ),
            other_path: pipelines_uri(held),
            other_body: pipeline_body(
                "env",
                json!([]),
                json!({ "environmentVariables": { "LOG_LEVEL": "info", "SHARD": "2" } }),
            ),
            listings: vec![(pipelines_uri(held), vec!["env"])],
        },
        Row {
            case: "CreateTask, another taskName",
            token_seed: "create-task-another-name",
            kind: Kind::Task,
            workspaces: vec![held],
            path: tasks_uri(held),
            body: task_body("t1", "UNITS_2", json!({})),
            other_path: tasks_uri(held),
            other_body: task_body("t2", "UNITS_2", json!({})),
            listings: vec![(tasks_uri(held), vec!["t1"])],
        },
        Row {
            case: "CreateTask, another configuration",
            token_seed: "create-task-another-configuration",
            kind: Kind::Task,
            workspaces: vec![held],
            path: tasks_uri(held),
            body: task_body("sized", "UNITS_2", json!({})),
            other_path: tasks_uri(held),
            other_body: task_body("sized", "UNITS_4", json!({})),
            listings: vec![(tasks_uri(held), vec!["sized"])],
        },
        Row {
            case: "CreateTask, another tag",
            token_seed: "create-task-another-tag",
            kind: Kind::Task,
            workspaces: vec![held],
            path: tasks_uri(held),
            body: task_body("tagged", "UNITS_2", json!({ "tags": { "team": "ops" } })),
            other_path: tasks_uri(held),
            other_body: task_body(
                "tagged",
                "UNITS_2",
                json!({ "tags": { "team": "analytics" } }),
            ),
            listings: vec![(tasks_uri(held), vec!["tagged"])],
        },
        Row {
            case: "CreateWorkspace, another workspaceName",
            token_seed: "create-workspace-another-name",
            kind: Kind::Workspace,
            workspaces: vec![],
            path: WORKSPACES_URI.to_string(),
            body: workspace_body("w1", json!({})),
            other_path: WORKSPACES_URI.to_string(),
            other_body: workspace_body("w2", json!({})),
            listings: vec![(WORKSPACES_URI.to_string(), vec!["w1"])],
        },
        Row {
            case: "CreateWorkspace, another description",
            token_seed: "create-workspace-another-description",
            kind: Kind::Workspace,
            workspaces: vec![],
            path: WORKSPACES_URI.to_string(),
            body: workspace_body(
                "described",
                json!({ "workspaceDescription": "the north line" }),
            ),
            other_path: WORKSPACES_URI.to_string(),
            other_body: workspace_body(
                "described",
                json!({ "workspaceDescription": "the south line" }),
            ),
            listings: vec![(WORKSPACES_URI.to_string(), vec!["described"])],
        },
    ]
}

/// The names a listing at `path` reports for `kind`.
async fn listed(router: &MultiServiceRouter, path: &str, kind: Kind) -> Vec<String> {
    let (status, body) = call(router, "GET", path, None).await;
    assert_eq!(status, 200, "list {path}: got {status} {body}");
    body[kind.summaries()]
        .as_array()
        .unwrap_or_else(|| panic!("list {path}: no {} in {body}", kind.summaries()))
        .iter()
        .map(|summary| {
            summary[kind.name_member()]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "list {path}: a summary without a {}: {summary}",
                        kind.name_member()
                    )
                })
                .to_string()
        })
        .collect()
}

/// `body` without its lifecycle status, which the clock moves on its own: what
/// two answers to the same call have to agree on member for member.
fn without_status(body: &Value, kind: Kind) -> Value {
    let mut stripped = body.clone();
    stripped
        .as_object_mut()
        .expect("an object answer")
        .remove(kind.status_member());
    stripped
}

#[tokio::test]
async fn a_token_one_create_spent_refuses_a_different_create_and_mints_nothing() {
    for row in rows() {
        // A router per row, so the listings below report exactly what this row's
        // own calls left behind.
        let router = router();
        for workspace in &row.workspaces {
            crate::support::ensure_workspace(&router, workspace).await;
        }
        let token = client_token(row.token_seed);

        let (status, first) = call(
            &router,
            "POST",
            &row.path,
            Some(with_token(&row.body, &token)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the first create expected 200, got {status} {first}",
            row.case
        );
        let created_name = member(&first, row.kind.id_member(), row.case);
        let created_arn = member(&first, row.kind.arn_member(), row.case);

        // The SAME request under that token is a retry of the call above: it is
        // answered from what that call left behind.
        let (status, retried) = call(
            &router,
            "POST",
            &row.path,
            Some(with_token(&row.body, &token)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the retry expected 200, got {status} {retried}",
            row.case
        );
        assert_eq!(
            without_status(&retried, row.kind),
            without_status(&first, row.kind),
            "case {:?}: a retry answers as the first call did, got {retried}",
            row.case
        );

        // A DIFFERENT request reusing the spent token is the modelled conflict,
        // naming the resource the first call acted on.
        let (status, refused) = call(
            &router,
            "POST",
            &row.other_path,
            Some(with_token(&row.other_body, &token)),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused)),
            (409, "ConflictingOperationException"),
            "case {:?}: a spent token expected the modelled conflict, got {status} {refused}",
            row.case
        );
        assert_eq!(
            refused["resourceId"].as_str().unwrap_or(""),
            created_name,
            "case {:?}: the conflict names the resource the first call acted on, got {refused}",
            row.case
        );
        assert_eq!(
            refused["resourceArn"].as_str().unwrap_or(""),
            created_arn,
            "case {:?}: the conflict names that resource's ARN, got {refused}",
            row.case
        );

        // The refused create persisted nothing: every listing reports exactly what
        // the accepted call made.
        for (path, expected) in &row.listings {
            let held = listed(&router, path, row.kind).await;
            assert_eq!(
                held, *expected,
                "case {:?}: the refused create must mint nothing, {path} holds {held:?}",
                row.case
            );
        }
    }
}

/// The string `member` of an answer, which the create's output declares
/// required.
fn member(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries no {member}: {body}"))
        .to_string()
}

/// `UpdateWorkspace` on `name` with `body`.
async fn update_workspace(router: &MultiServiceRouter, name: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("{WORKSPACES_URI}/{name}"),
        Some(body),
    )
    .await
}

/// `DescribeWorkspace` on `name`.
async fn describe_workspace(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("{WORKSPACES_URI}/{name}"), None).await;
    assert_eq!(status, 200, "describe {name}: got {status} {body}");
    body
}

#[tokio::test]
async fn a_token_one_update_spent_refuses_a_different_update_and_writes_nothing() {
    let router = router();
    let updated = "updated";
    let untouched = "untouched";
    create_settled(&router, updated, default_encryption()).await;
    create_settled(&router, untouched, default_encryption()).await;

    let token = client_token("update-workspace-token-spent");
    let request = json!({ "workspaceDescription": "the north line", "clientToken": token });
    let (status, accepted) = update_workspace(&router, updated, request.clone()).await;
    assert_eq!(status, 200, "the first update got {status} {accepted}");
    assert_eq!(
        accepted["workspaceStatus"]["state"].as_str().unwrap_or(""),
        "UPDATING",
        "case {:?}: an accepted update answers UPDATING, got {accepted}",
        "the first update"
    );
    tokio::time::sleep(settle()).await;
    let settled = describe_workspace(&router, updated).await;

    // The retry of that very request performs no further action.
    let (status, retried) = update_workspace(&router, updated, request.clone()).await;
    assert_eq!(status, 200, "the retry got {status} {retried}");
    let after_retry = describe_workspace(&router, updated).await;
    assert_eq!(
        after_retry, settled,
        "case {:?}: a retry writes nothing further, got {after_retry}",
        "the retry"
    );

    // Each row is a DIFFERENT request reusing the spent token: one addressing
    // ANOTHER workspace, and one carrying another description on the same
    // workspace. Both are the modelled conflict naming the workspace the spent
    // token's call updated, and both leave the store exactly as it was.
    let rows = [
        (
            "another workspace",
            untouched,
            json!({ "workspaceDescription": "the north line", "clientToken": token }),
        ),
        (
            "another description",
            updated,
            json!({ "workspaceDescription": "the south line", "clientToken": token }),
        ),
    ];
    for (case, addressed, body) in rows {
        let (status, refused) = update_workspace(&router, addressed, body).await;
        assert_eq!(
            (status, error_code(&refused)),
            (409, "ConflictingOperationException"),
            "case {case:?}: a spent token expected the modelled conflict, got {status} {refused}"
        );
        assert_eq!(
            refused["resourceId"].as_str().unwrap_or(""),
            updated,
            "case {case:?}: the conflict names the workspace the first call updated, got {refused}"
        );
        assert_eq!(
            refused["resourceArn"].as_str().unwrap_or(""),
            crate::workspace_lifecycle::expected_arn(updated),
            "case {case:?}: the conflict names that workspace's ARN, got {refused}"
        );
    }

    let after_refusals = describe_workspace(&router, updated).await;
    assert_eq!(
        after_refusals, settled,
        "a refused update must leave the workspace it addressed unchanged, got {after_refusals}"
    );
    let spared = describe_workspace(&router, untouched).await;
    assert_eq!(
        spared["workspaceDescription"],
        Value::Null,
        "a refused update must write nothing to the workspace it addressed, got {spared}"
    );
    assert_eq!(
        spared["workspaceStatus"]["state"].as_str().unwrap_or(""),
        "ACTIVE",
        "a refused update must not move the workspace it addressed, got {spared}"
    );
}
