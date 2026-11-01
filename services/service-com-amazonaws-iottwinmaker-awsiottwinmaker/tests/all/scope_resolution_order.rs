// SPDX-License-Identifier: CC0-1.0

//! Which refusal a workspace-scoped request gets when the workspace it names is
//! one this service does not hold AND another member of the same request is bad,
//! end to end over HTTP.
//!
//! RULE under test: a workspace-scoped operation resolves the SCOPE the request
//! names before it judges the request's own members, so a request naming a
//! workspace this service does not hold is refused for THAT workspace — the
//! message naming the id that could not be resolved, with the status and error
//! shape the operation models for an unresolvable scope — whatever else the
//! request carries.
//!
//! The rows differ along WHICH workspace-scoped operation is asked (the
//! workspace-, entity- and sync-job-scoped listings, the two property reads and
//! the graph query), along WHAT ELSE the request carries (nothing, a page size,
//! a `nextToken` no listing of this service minted, a REAL token another listing
//! minted, both at once) and along the id of the workspace that cannot be
//! resolved (a word, and the shortest id the member's pattern admits).
//!
//! The operations that carry a member of their own which can itself be invalid —
//! a query statement outside the grammar, a filter list past the one object the
//! filter union admits — are rows too: the refusal is the scope's and not
//! theirs.
//!
//! The companion case pins the other side, so the rule cannot be satisfied by
//! refusing every request for its workspace: against a workspace this service
//! DOES hold, the bad member is what the refusal names.

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::SYNC_SOURCE_SITEWISE;

use crate::harness::{Emulator, Resp};

/// A workspace this service holds, so the account-level listing has a page to
/// leave over and the companion case has a scope that resolves.
const WORKSPACE: &str = "scopeOrderWs";
/// A second one, for the same reason.
const OTHER_WORKSPACE: &str = "scopeOrderWsTwo";

/// The workspace ids no request below can resolve: a word, and the shortest id
/// `Id`'s pattern admits (`^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$`, so two
/// characters), since the refusal is built from the request and must name
/// whatever id it was given.
const UNRESOLVABLE: [&str; 2] = ["noSuchWs", "ab"];

/// A statement the query language accepts, so nothing but the workspace is wrong
/// with the query rows.
const WELL_FORMED_STATEMENT: &str = "SELECT e FROM EntityGraph MATCH (e)";

/// The message every operation that LOOKS a workspace up reports for one this
/// service does not hold, written out here rather than read from the crate so
/// this asserts the wording instead of restating the implementation.
fn unresolved_workspace(workspace_id: &str) -> String {
    format!("Unable to find workspace for id: {workspace_id}")
}

/// The `message` an error body carries.
fn message(resp: &Resp) -> String {
    resp.member("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One workspace-scoped request: where it is served and what it carries, plus
/// the refusal its operation models for a scope it cannot resolve — a 400
/// `ValidationException` for the operations whose model declares no
/// `ResourceNotFoundException`, a 404 `ResourceNotFoundException` for the ones
/// that do.
struct Scoped {
    /// The operation, for the assertion messages.
    operation: &'static str,
    path: String,
    body: Value,
    status: u16,
    error: &'static str,
}

/// Every workspace-scoped request this service serves that resolves a workspace
/// to answer, addressed to `workspace_id` and otherwise valid.
fn scoped_requests(workspace_id: &str) -> Vec<Scoped> {
    let validation = |operation: &'static str, path: String, body: Value| Scoped {
        operation,
        path,
        body,
        status: 400,
        error: "ValidationException",
    };
    let not_found = |operation: &'static str, path: String, body: Value| Scoped {
        operation,
        path,
        body,
        status: 404,
        error: "ResourceNotFoundException",
    };
    vec![
        validation(
            "ListEntities",
            format!("/workspaces/{workspace_id}/entities-list"),
            json!({}),
        ),
        validation(
            "ListComponentTypes",
            format!("/workspaces/{workspace_id}/component-types-list"),
            json!({}),
        ),
        validation(
            "ListScenes",
            format!("/workspaces/{workspace_id}/scenes-list"),
            json!({}),
        ),
        validation(
            "ListSyncJobs",
            format!("/workspaces/{workspace_id}/sync-jobs-list"),
            json!({}),
        ),
        validation(
            "ListSyncResources",
            format!("/workspaces/{workspace_id}/sync-jobs/{SYNC_SOURCE_SITEWISE}/resources-list"),
            json!({}),
        ),
        validation(
            "ExecuteQuery",
            "/queries/execution".to_string(),
            json!({"workspaceId": workspace_id, "queryStatement": WELL_FORMED_STATEMENT}),
        ),
        not_found(
            "ListComponents",
            format!("/workspaces/{workspace_id}/entities/unit-1/components-list"),
            json!({}),
        ),
        not_found(
            "ListProperties",
            format!("/workspaces/{workspace_id}/properties-list"),
            json!({"entityId": "unit-1", "componentName": "Sensor"}),
        ),
        not_found(
            "GetPropertyValue",
            format!("/workspaces/{workspace_id}/entity-properties/value"),
            json!({
                "entityId": "unit-1",
                "componentName": "Sensor",
                "selectedProperties": ["Temp"],
            }),
        ),
        not_found(
            "GetPropertyValueHistory",
            format!("/workspaces/{workspace_id}/entity-properties/history"),
            json!({
                "entityId": "unit-1",
                "componentName": "Sensor",
                "selectedProperties": ["Temp"],
            }),
        ),
    ]
}

/// What else a request carries besides its scope, each an overlay on the body
/// above: `minted_elsewhere` is a REAL token the account-level listing minted,
/// which is no listing of a workspace's token either.
fn other_members(minted_elsewhere: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("nothingElse", json!({})),
        ("pageSize", json!({"maxResults": 1})),
        ("unmintedToken", json!({"nextToken": "not-a-token"})),
        (
            "tokenMintedElsewhere",
            json!({"nextToken": minted_elsewhere}),
        ),
        (
            "tokenAndPageSize",
            json!({"maxResults": 1, "nextToken": "not-a-token"}),
        ),
    ]
}

/// A token `ListWorkspaces` really minted, cut to one workspace per page so the
/// account-level listing leaves one over.
async fn minted_workspaces_token(emulator: &Emulator) -> String {
    let page = emulator.list_workspaces(json!({"maxResults": 1})).await;
    assert_eq!(
        page.status, 200,
        "ListWorkspaces: expected 200, got {page:?}"
    );
    page.member("nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!("ListWorkspaces left a page over, so it must mint a token: {page:?}")
        })
        .to_string()
}

/// The workspaces the account-level listing pages over and the companion case
/// resolves.
async fn seed(emulator: &Emulator) {
    for workspace_id in [WORKSPACE, OTHER_WORKSPACE] {
        let created = emulator.create_workspace(workspace_id).await;
        assert_eq!(
            created.status, 200,
            "CreateWorkspace {workspace_id:?}: expected 200, got {created:?}"
        );
    }
}

#[tokio::test]
async fn a_request_naming_an_unresolvable_workspace_is_refused_for_that_workspace() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    let minted_elsewhere = minted_workspaces_token(&emulator).await;

    for workspace_id in UNRESOLVABLE {
        for request in scoped_requests(workspace_id) {
            for (members_case, overlay) in other_members(&minted_elsewhere) {
                let case = format!("{workspace_id}/{}/{members_case}", request.operation);
                let mut body = request.body.clone();
                for (member, value) in overlay.as_object().expect("an overlay is an object") {
                    body[member] = value.clone();
                }
                let refused = emulator.call("POST", &request.path, Some(body)).await;

                assert_eq!(
                    (refused.status, refused.error_type.as_deref()),
                    (request.status, Some(request.error)),
                    "case {case:?}: expected {} {}, got {refused:?}",
                    request.status,
                    request.error
                );
                assert_eq!(
                    message(&refused),
                    unresolved_workspace(workspace_id),
                    "case {case:?}: the refusal must name the workspace it could not resolve, \
                     got {refused:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_unresolvable_workspace_outranks_a_member_that_is_itself_invalid() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for workspace_id in UNRESOLVABLE {
        // The query's statement: outside the grammar, so the query itself could
        // not be used either — and the scope is still what the refusal names.
        let refused = emulator
            .execute_query(json!({
                "workspaceId": workspace_id,
                "queryStatement": "THIS IS NOT PARTIQL !!! ###",
            }))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {workspace_id:?}/ExecuteQuery: expected a 400 ValidationException, \
             got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            unresolved_workspace(workspace_id),
            "case {workspace_id:?}/ExecuteQuery: the refusal must name the workspace it could \
             not resolve, got {refused:?}"
        );

        // A listing's filter list: past the one object the filter union admits
        // ("Only one object is accepted as a valid input" —
        // `smithy_query(query="shape",shape="ListComponentTypesFilter")`), so
        // the filters could not be used either.
        let two_filters = json!({
            "filters": [{"namespace": "com.example"}, {"isAbstract": true}],
        });
        let refused = emulator
            .list_component_types(workspace_id, two_filters)
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {workspace_id:?}/ListComponentTypes: expected a 400 ValidationException, \
             got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            unresolved_workspace(workspace_id),
            "case {workspace_id:?}/ListComponentTypes: the refusal must name the workspace it \
             could not resolve, got {refused:?}"
        );
    }
}

#[tokio::test]
async fn a_bad_member_is_what_a_resolvable_workspace_reports() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The token refusal, once the scope stands: the member is what is wrong.
    let cases: Vec<(&str, Resp)> = vec![
        (
            "ExecuteQuery",
            emulator
                .execute_query(json!({
                    "workspaceId": WORKSPACE,
                    "queryStatement": WELL_FORMED_STATEMENT,
                    "nextToken": "not-a-token",
                }))
                .await,
        ),
        (
            "ListComponentTypes",
            emulator
                .list_component_types(WORKSPACE, json!({"nextToken": "not-a-token"}))
                .await,
        ),
        (
            "ListEntities",
            emulator
                .list_entities(WORKSPACE, json!({"nextToken": "not-a-token"}))
                .await,
        ),
    ];
    for (case, refused) in cases {
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected a 400 ValidationException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            "nextToken is not a token this listing minted",
            "case {case:?}: a token no listing minted is what the refusal must name, \
             got {refused:?}"
        );
    }

    // The statement refusal, once the scope stands.
    let refused = emulator
        .execute_query(json!({
            "workspaceId": WORKSPACE,
            "queryStatement": "THIS IS NOT PARTIQL !!! ###",
        }))
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "a statement outside the grammar: expected a 400 ValidationException, got {refused:?}"
    );
    assert!(
        message(&refused).starts_with("Invalid QueryStatement: "),
        "a statement outside the grammar must be what the refusal names, got {refused:?}"
    );

    // And a request with nothing wrong with it is still served.
    let served = emulator
        .execute_query(json!({
            "workspaceId": WORKSPACE,
            "queryStatement": WELL_FORMED_STATEMENT,
        }))
        .await;
    assert_eq!(
        (served.status, served.member("rows")),
        (200, &json!([])),
        "a query over a workspace with no entity selects no row, got {served:?}"
    );
}
