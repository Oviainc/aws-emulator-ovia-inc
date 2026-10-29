// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHAT a spent `clientToken` is scoped to, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rule these suites pin: a `clientToken` names the CALL that spent it, not a
//! resource. So
//!
//! * the SAME request under that token is a retry of that call — answered from
//!   what the call left behind, writing nothing further;
//! * ANY other request reusing it belongs to another call: it is refused
//!   `ConflictingOperationException` (409) naming the resource the first call acted
//!   on, and mints nothing — whatever member it differs in;
//! * the same request under a FRESH token is served, so the refusal above is the
//!   token's and not the request's;
//! * the token stays spent after the resource that call made is DELETED: a
//!   different request reusing it is still that refusal, while a repeat of the
//!   call's own request has nothing left to answer from and is served as the fresh
//!   create it now is.
//!
//! `client_token_spent_by_another_request` pins the same rule for the workspace,
//! pipeline and task writes; these suites span what it does not: the
//! workspace-scoped application create, every member of it a second request can
//! differ in, and — for the application and the workspace alike — a token whose
//! resource is gone.
//!
//! The HTTP harness (`call`, `router`, `error_code`), the token helpers
//! (`client_token`, `with_token`, `merge`) and the application and workspace
//! routes are the sibling suites'; this one reuses them rather than growing a
//! second copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::application_lifecycle::{
    create as create_application, create_workspace, describe as describe_application,
};
use crate::asset_lifecycle::{call, error_code, router};
use crate::client_token_idempotency::{client_token, with_token};
use crate::client_token_spent_by_another_request::merge;
use crate::workspace_lifecycle::{
    create as create_workspace_request, default_encryption, describe as describe_workspace,
    expected_arn as workspace_arn, kms_encryption, settle,
};

/// `GET /applications`, as `OPERATIONS.md` reports the route: the listing spans
/// every workspace, so it reports exactly what the creates below left behind.
const APPLICATIONS: &str = "/applications";

/// `POST|GET /workspaces`.
const WORKSPACES: &str = "/workspaces";

/// An Identity Center instance ARN of the form `CreateApplication`'s
/// `idcInstanceArn` admits (`smithy_query(query="operation",
/// operation="CreateApplication")`: target `ARN`,
/// `^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`).
const IDC_INSTANCE: &str = "arn:aws:sso:::instance/ssoins-1111111111111111";

/// A second one, for the row whose requests differ in the instance they name.
const OTHER_IDC_INSTANCE: &str = "arn:aws:sso:::instance/ssoins-2222222222222222";

/// A `CreateApplication` body carrying the members the create requires, plus
/// whatever `extra` states — a description, tags, another instance ARN.
fn application_body(workspace: &str, name: &str, extra: Value) -> Value {
    let mut body = json!({
        "workspaceName": workspace,
        "name": name,
        "idcInstanceArn": IDC_INSTANCE,
    });
    merge(&mut body, extra);
    body
}

/// One row: two DIFFERENT `CreateApplication` requests, differing in exactly one
/// member, and the workspaces they need open.
struct Row {
    case: &'static str,
    /// The seed this row's token is built from: non-whitespace, as
    /// `ClientToken`'s `^\S{36,64}$` requires.
    token_seed: &'static str,
    workspaces: Vec<&'static str>,
    first: Value,
    second: Value,
}

/// The rows, one per member a second create can differ in: the application's own
/// name, the workspace it is created in, the Identity Center instance it names, a
/// description present where the first carried none and present with another
/// value, and tags present where the first carried none, with another value, and
/// with one pair more.
fn rows() -> Vec<Row> {
    let held = "plant-a";
    let other = "plant-b";
    vec![
        Row {
            case: "another name",
            token_seed: "create-application-another-name",
            workspaces: vec![held],
            first: application_body(held, "Boiler", json!({})),
            second: application_body(held, "Chiller", json!({})),
        },
        Row {
            case: "another workspace",
            token_seed: "create-application-another-workspace",
            workspaces: vec![held, other],
            first: application_body(held, "Boiler", json!({})),
            second: application_body(other, "Boiler", json!({})),
        },
        Row {
            case: "another Identity Center instance",
            token_seed: "create-application-another-instance",
            workspaces: vec![held],
            first: application_body(held, "Boiler", json!({})),
            second: application_body(
                held,
                "Boiler",
                json!({ "idcInstanceArn": OTHER_IDC_INSTANCE }),
            ),
        },
        Row {
            case: "a description the first request did not carry",
            token_seed: "create-application-a-description",
            workspaces: vec![held],
            first: application_body(held, "Boiler", json!({})),
            second: application_body(held, "Boiler", json!({ "description": "the north line" })),
        },
        Row {
            case: "another description",
            token_seed: "create-application-another-description",
            workspaces: vec![held],
            first: application_body(held, "Boiler", json!({ "description": "the north line" })),
            second: application_body(held, "Boiler", json!({ "description": "the south line" })),
        },
        Row {
            case: "tags the first request did not carry",
            token_seed: "create-application-tags",
            workspaces: vec![held],
            first: application_body(held, "Boiler", json!({})),
            second: application_body(held, "Boiler", json!({ "tags": { "team": "ops" } })),
        },
        Row {
            case: "another tag value",
            token_seed: "create-application-another-tag-value",
            workspaces: vec![held],
            first: application_body(held, "Boiler", json!({ "tags": { "team": "ops" } })),
            second: application_body(held, "Boiler", json!({ "tags": { "team": "analytics" } })),
        },
        Row {
            case: "one tag more",
            token_seed: "create-application-one-tag-more",
            workspaces: vec![held],
            first: application_body(held, "Boiler", json!({ "tags": { "team": "ops" } })),
            second: application_body(
                held,
                "Boiler",
                json!({ "tags": { "team": "ops", "line": "north" } }),
            ),
        },
    ]
}

/// The string `member` of an answer, which the create's output declares required.
fn member(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries no {member}: {body}"))
        .to_string()
}

/// What two answers to the same `CreateApplication` call have to agree on member
/// for member: everything but the lifecycle status, which the clock moves on its
/// own.
fn identity(body: &Value, case: &str) -> Vec<String> {
    ["id", "arn", "dnsSubdomain", "name"]
        .iter()
        .map(|name| member(body, name, case))
        .collect()
}

/// The ids `ListApplications` reports, sorted — every application the store holds,
/// whatever workspace it was created in.
async fn listed_applications(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "GET", APPLICATIONS, None).await;
    assert_eq!(status, 200, "ListApplications: got {status} {body}");
    let mut ids: Vec<String> = body["applications"]
        .as_array()
        .unwrap_or_else(|| panic!("ListApplications: no applications in {body}"))
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .unwrap_or_else(|| panic!("ListApplications: a summary without an id: {summary}"))
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// The workspace names `ListWorkspaces` reports, sorted.
async fn listed_workspaces(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "GET", WORKSPACES, None).await;
    assert_eq!(status, 200, "ListWorkspaces: got {status} {body}");
    let mut names: Vec<String> = body["workspaceSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("ListWorkspaces: no workspaceSummaries in {body}"))
        .iter()
        .map(|summary| {
            summary["name"]
                .as_str()
                .unwrap_or_else(|| panic!("ListWorkspaces: a summary without a name: {summary}"))
                .to_string()
        })
        .collect();
    names.sort();
    names
}

/// `DeleteApplication` on `(workspace, id)`, which the model answers 204.
async fn delete_application(router: &MultiServiceRouter, workspace: &str, id: &str) {
    let (status, body) = call(
        router,
        "DELETE",
        &format!("/workspaces/{workspace}/applications/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "DeleteApplication {workspace}/{id}: got {status} {body}"
    );
}

/// `DeleteWorkspace` on `name`, carrying no token of its own.
async fn delete_workspace(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(router, "DELETE", &format!("{WORKSPACES}/{name}"), None).await;
    assert_eq!(status, 200, "DeleteWorkspace {name}: got {status} {body}");
}

#[tokio::test]
async fn a_token_one_create_spent_is_refused_to_every_other_create() {
    for row in rows() {
        // A router per row, so the listing below reports exactly what this row's
        // own calls left behind.
        let router = router();
        for workspace in &row.workspaces {
            create_workspace(&router, workspace).await;
        }
        let token = client_token(row.token_seed);

        let (status, first) = create_application(&router, with_token(&row.first, &token)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the first create expected 200, got {status} {first}",
            row.case
        );
        let created_id = member(&first, "id", row.case);
        let created_arn = member(&first, "arn", row.case);

        // The SAME request under that token is a retry of the call above: it is
        // answered from what that call left behind.
        let (status, retried) = create_application(&router, with_token(&row.first, &token)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the retry expected 200, got {status} {retried}",
            row.case
        );
        assert_eq!(
            identity(&retried, row.case),
            identity(&first, row.case),
            "case {:?}: a retry answers as the first call did, got {retried}",
            row.case
        );

        // The row's OTHER request under that token belongs to no call this store
        // served: the modelled conflict, naming the application the first call made.
        let (status, refused) = create_application(&router, with_token(&row.second, &token)).await;
        assert_eq!(
            (status, error_code(&refused)),
            (409, "ConflictingOperationException"),
            "case {:?}: a spent token expected the modelled conflict, got {status} {refused}",
            row.case
        );
        assert_eq!(
            (
                refused["resourceId"].as_str().unwrap_or(""),
                refused["resourceArn"].as_str().unwrap_or(""),
            ),
            (created_id.as_str(), created_arn.as_str()),
            "case {:?}: the conflict names the application the first call made, got {refused}",
            row.case
        );
        let held = listed_applications(&router).await;
        assert_eq!(
            held,
            vec![created_id.clone()],
            "case {:?}: the refused create must mint nothing, the store holds {held:?}",
            row.case
        );

        // The very same second request under a FRESH token is served: what the
        // refusal above answered was the token, not the request.
        let fresh = client_token(&format!("{}-fresh", row.token_seed));
        let (status, served) = create_application(&router, with_token(&row.second, &fresh)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the same request under a fresh token expected 200, got {status} {served}",
            row.case
        );
        let second_id = member(&served, "id", row.case);
        assert_ne!(
            second_id, created_id,
            "case {:?}: a create under a fresh token mints its own application, got {served}",
            row.case
        );
        let mut expected = vec![created_id.clone(), second_id];
        expected.sort();
        let held = listed_applications(&router).await;
        assert_eq!(
            held, expected,
            "case {:?}: expected exactly the two accepted creates, the store holds {held:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_application_token_stays_spent_once_its_application_is_deleted() {
    let router = router();
    let workspace = "plant-a";
    create_workspace(&router, workspace).await;
    let token = client_token("create-application-outlives-its-application");
    let request = application_body(workspace, "Boiler", json!({}));

    let (status, created) = create_application(&router, with_token(&request, &token)).await;
    assert_eq!(status, 200, "the create got {status} {created}");
    let case = "the deleted application";
    let created_id = member(&created, "id", case);
    let created_arn = member(&created, "arn", case);
    delete_application(&router, workspace, &created_id).await;

    // The call that spent the token is still the call that spent it, so a
    // DIFFERENT request reusing it is still the modelled conflict — naming the
    // application that call made, whose ARN carries the id it minted.
    let other = application_body(workspace, "Chiller", json!({}));
    let (status, refused) = create_application(&router, with_token(&other, &token)).await;
    assert_eq!(
        (status, error_code(&refused)),
        (409, "ConflictingOperationException"),
        "case {case:?}: a spent token expected the modelled conflict, got {status} {refused}"
    );
    assert_eq!(
        (
            refused["resourceId"].as_str().unwrap_or(""),
            refused["resourceArn"].as_str().unwrap_or(""),
        ),
        (created_id.as_str(), created_arn.as_str()),
        "case {case:?}: the conflict names the application the token's call made, got {refused}"
    );
    let held = listed_applications(&router).await;
    assert!(
        held.is_empty(),
        "case {case:?}: the refused create must mint nothing, the store holds {held:?}"
    );

    // The token's OWN request has nothing left to answer from — the application it
    // made is gone — so it is served as the fresh create it now is.
    let (status, again) = create_application(&router, with_token(&request, &token)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the call's own request expected 200, got {status} {again}"
    );
    let remade_id = member(&again, "id", case);
    assert_ne!(
        remade_id, created_id,
        "case {case:?}: a fresh create mints an application of its own, got {again}"
    );
    let (status, described) = describe_application(&router, workspace, &remade_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the remade application must read back, got {status} {described}"
    );
    assert_eq!(
        described["name"].as_str().unwrap_or(""),
        "Boiler",
        "case {case:?}: the remade application reports the request's members, got {described}"
    );
}

#[tokio::test]
async fn a_workspace_token_stays_spent_once_its_workspace_is_deleted() {
    let router = router();
    let case = "the deleted workspace";
    let spent_on = "plant-a";
    let token = client_token("create-workspace-outlives-its-workspace");
    let request = json!({
        "workspaceName": spent_on,
        "encryptionConfiguration": default_encryption(),
    });

    let (status, created) = create_workspace_request(&router, with_token(&request, &token)).await;
    assert_eq!(status, 200, "case {case:?}: the create got {status} {created}");
    // A workspace is deleted only once the transition its create owes has fallen
    // due, so the delete below addresses a settled workspace.
    tokio::time::sleep(settle()).await;
    delete_workspace(&router, spent_on).await;

    // A DIFFERENT request reusing the token is the modelled conflict, naming the
    // workspace that call created — by the name it was created under and the ARN
    // that name resolves to.
    let other = json!({
        "workspaceName": "plant-b",
        "encryptionConfiguration": kms_encryption("alias/aws/iotsitewise"),
    });
    let (status, refused) = create_workspace_request(&router, with_token(&other, &token)).await;
    assert_eq!(
        (status, error_code(&refused)),
        (409, "ConflictingOperationException"),
        "case {case:?}: a spent token expected the modelled conflict, got {status} {refused}"
    );
    assert_eq!(
        (
            refused["resourceId"].as_str().unwrap_or(""),
            refused["resourceArn"].as_str().unwrap_or(""),
        ),
        (spent_on, workspace_arn(spent_on).as_str()),
        "case {case:?}: the conflict names the workspace the token's call made, got {refused}"
    );
    let held = listed_workspaces(&router).await;
    assert!(
        held.is_empty(),
        "case {case:?}: the refused create must mint nothing, the store holds {held:?}"
    );

    // The token's OWN request has nothing left to answer from, so it is served as
    // the fresh create it now is, and the workspace reads back.
    let (status, again) = create_workspace_request(&router, with_token(&request, &token)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the call's own request expected 200, got {status} {again}"
    );
    let (status, described) = describe_workspace(&router, spent_on).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the remade workspace must read back, got {status} {described}"
    );
    let held = listed_workspaces(&router).await;
    assert_eq!(
        held,
        vec![spent_on.to_string()],
        "case {case:?}: expected exactly the remade workspace, the store holds {held:?}"
    );
}
