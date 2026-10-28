// SPDX-License-Identifier: CC0-1.0

//! What a `clientToken` already spent on a DIFFERENT request means for the Monitor
//! operations whose answer carries nothing of the call that spent it, driven over
//! HTTP through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these rows pin: a `clientToken` suppresses re-execution only for a
//! REPEAT of the request it was spent on. Where an operation's answer reports no
//! member of that earlier call — an empty body, or a per-entry report rebuilt from
//! nothing the store kept — a success can state only that THIS request was carried
//! out, so a request differing in any member it is recognised by is a different
//! call and must be carried out: a read of the resource afterwards reports what it
//! asked for, never what the earlier call left behind.
//!
//! The rows span what the rule generalises over: the operation kind (update,
//! delete, batch association, batch disassociation), the resource kind (project,
//! dashboard, access policy, a project's asset membership), the member the second
//! request differs in (a name, an optional description absent from the first call,
//! a definition, an enum permission, the id in the path, the contents of a list),
//! how the token travels (in the payload, on the query string) and the answer
//! shape (an empty 200, an empty 204, a 200 carrying a per-entry list).
//!
//! The HTTP harness (`call`, `error_code`, `router_with_engine`, `create_model`,
//! `create_asset`), the portal create and the token helpers (`with_token`,
//! `token_for`, `carrying`) are the ones the sibling suites define; all are reused
//! rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::access_policy_lifecycle::create_portal;
use crate::asset_lifecycle::{call, create_asset, create_model, error_code, router_with_engine};
use crate::client_token_replay::{carrying, token_for};

/// What a read of the addressed resource must report at one step of a row.
#[derive(Debug, Clone)]
enum Reads {
    /// The read answers, reporting each of these members.
    Reports {
        read_uri: String,
        members: Vec<(&'static str, Value)>,
    },
    /// The read is the modelled not-found: the resource was removed.
    Gone { read_uri: String },
    /// The project's asset membership, which is a SET, holds exactly these ids.
    Assets {
        read_uri: String,
        asset_ids: Vec<String>,
    },
}

/// One row: a request served under a token, and a DIFFERENT request of the same
/// operation sent under that same, now spent, token.
struct Row {
    case: &'static str,
    method: &'static str,
    first_uri: String,
    first_body: Option<Value>,
    second_uri: String,
    second_body: Option<Value>,
    /// Whether `clientToken` travels on the query string (`@httpQuery`) rather
    /// than in the payload.
    token_in_query: bool,
    /// The status the operation models for a served call.
    status: u16,
    /// What a read reports once the FIRST request has been served — and still
    /// reports after a retry of it, which writes nothing.
    after_first: Reads,
    /// What a read reports once the DIFFERING request has been served.
    after_second: Reads,
}

/// Assert `expected` of the store, as the API reports it.
async fn assert_reads(router: &MultiServiceRouter, case: &str, step: &str, expected: &Reads) {
    match expected {
        Reads::Reports { read_uri, members } => {
            let (status, read) = call(router, "GET", read_uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?} {step}: the read got {status} {read}"
            );
            for (member, value) in members {
                assert_eq!(
                    &read[*member], value,
                    "case {case:?} {step}: expected {member} {value}, got {read}"
                );
            }
        }
        Reads::Gone { read_uri } => {
            let (status, read) = call(router, "GET", read_uri, None).await;
            assert_eq!(
                (status, error_code(&read)),
                (404, "ResourceNotFoundException"),
                "case {case:?} {step}: expected the modelled not-found, got {status} {read}"
            );
        }
        Reads::Assets {
            read_uri,
            asset_ids,
        } => {
            let (status, read) = call(router, "GET", read_uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?} {step}: the read got {status} {read}"
            );
            // The membership is a SET, so the comparison is order-independent.
            let mut reported: Vec<String> = read["assetIds"]
                .as_array()
                .map(|ids| {
                    ids.iter()
                        .filter_map(|id| id.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            reported.sort();
            let mut wanted = asset_ids.clone();
            wanted.sort();
            assert_eq!(
                reported, wanted,
                "case {case:?} {step}: expected the membership {wanted:?}, got {reported:?} \
                 from {read}"
            );
        }
    }
}

/// Create a project in `portal_id` and return its id.
async fn project(router: &MultiServiceRouter, portal_id: &str, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/projects",
        Some(json!({ "portalId": portal_id, "projectName": name })),
    )
    .await;
    assert_eq!(status, 201, "CreateProject {name:?}: got {status} {body}");
    body["projectId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateProject {name:?}: no projectId in {body}"))
        .to_string()
}

/// Create a dashboard in `project_id` and return its id.
async fn dashboard(router: &MultiServiceRouter, project_id: &str, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/dashboards",
        Some(json!({
            "projectId": project_id,
            "dashboardName": name,
            "dashboardDefinition": "{\"widgets\":[]}",
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateDashboard {name:?}: got {status} {body}");
    body["dashboardId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDashboard {name:?}: no dashboardId in {body}"))
        .to_string()
}

/// Create a portal-scoped access policy for the user `user_id` and return its id.
async fn access_policy(router: &MultiServiceRouter, portal_id: &str, user_id: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/access-policies",
        Some(json!({
            "accessPolicyIdentity": { "user": { "id": user_id } },
            "accessPolicyResource": { "portal": { "id": portal_id } },
            "accessPolicyPermission": "VIEWER",
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateAccessPolicy {user_id:?}: got {status} {body}"
    );
    body["accessPolicyId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAccessPolicy {user_id:?}: no accessPolicyId in {body}"))
        .to_string()
}

/// Associate `asset_ids` with `project_id` under no token at all, so the
/// membership a row starts from is set up by a call of its own.
async fn associate(router: &MultiServiceRouter, project_id: &str, asset_ids: &[&str]) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/projects/{project_id}/assets/associate"),
        Some(json!({ "assetIds": asset_ids })),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchAssociateProjectAssets {asset_ids:?}: got {status} {body}"
    );
}

/// The rows, each with the resources it addresses created first. A row never
/// addresses a resource another row removes.
async fn rows(router: &MultiServiceRouter, portal_id: &str) -> Vec<Row> {
    let renamed = project(router, portal_id, "BeforeTheRename").await;
    let described = project(router, portal_id, "BeforeTheDescription").await;
    let dashboard_home = project(router, portal_id, "DashboardHome").await;
    let edited = dashboard(router, &dashboard_home, "BeforeTheEdit").await;
    let first_dashboard = dashboard(router, &dashboard_home, "FirstDoomedDashboard").await;
    let second_dashboard = dashboard(router, &dashboard_home, "SecondDoomedDashboard").await;
    let first_doomed_project = project(router, portal_id, "FirstDoomedProject").await;
    let second_doomed_project = project(router, portal_id, "SecondDoomedProject").await;
    let promoted = access_policy(router, portal_id, "u-0123456789abcdef").await;
    let first_doomed_policy = access_policy(router, portal_id, "u-1111111111111111").await;
    let second_doomed_policy = access_policy(router, portal_id, "u-2222222222222222").await;

    let model_id = create_model(router, "SpentTokenModel", json!([])).await;
    let first_asset = create_asset(router, "SpentTokenAssetOne", &model_id).await;
    let second_asset = create_asset(router, "SpentTokenAssetTwo", &model_id).await;
    let growing = project(router, portal_id, "GrowingMembership").await;
    let shrinking = project(router, portal_id, "ShrinkingMembership").await;
    associate(router, &shrinking, &[&first_asset, &second_asset]).await;

    let identity = json!({ "user": { "id": "u-0123456789abcdef" } });
    let resource = json!({ "portal": { "id": portal_id } });
    let widget_definition = "{\"widgets\":[{\"type\":\"sc-line-chart\"}]}";

    vec![
        Row {
            case: "UpdateProject differing in the project name",
            method: "PUT",
            first_uri: format!("/projects/{renamed}"),
            first_body: Some(json!({ "projectName": "FirstUpdate" })),
            second_uri: format!("/projects/{renamed}"),
            second_body: Some(json!({ "projectName": "SecondUpdate" })),
            token_in_query: false,
            status: 200,
            after_first: Reads::Reports {
                read_uri: format!("/projects/{renamed}"),
                members: vec![("projectName", json!("FirstUpdate"))],
            },
            after_second: Reads::Reports {
                read_uri: format!("/projects/{renamed}"),
                members: vec![("projectName", json!("SecondUpdate"))],
            },
        },
        Row {
            case: "UpdateProject differing in the optional description",
            method: "PUT",
            first_uri: format!("/projects/{described}"),
            first_body: Some(json!({ "projectName": "Described" })),
            second_uri: format!("/projects/{described}"),
            second_body: Some(json!({
                "projectName": "Described",
                "projectDescription": "a description the first call did not carry",
            })),
            token_in_query: false,
            status: 200,
            after_first: Reads::Reports {
                read_uri: format!("/projects/{described}"),
                // The first update carried no description, so the record holds none.
                members: vec![("projectDescription", json!(null))],
            },
            after_second: Reads::Reports {
                read_uri: format!("/projects/{described}"),
                members: vec![(
                    "projectDescription",
                    json!("a description the first call did not carry"),
                )],
            },
        },
        Row {
            case: "UpdateDashboard differing in the name and the definition",
            method: "PUT",
            first_uri: format!("/dashboards/{edited}"),
            first_body: Some(json!({
                "dashboardName": "FirstUpdate",
                "dashboardDefinition": "{\"widgets\":[]}",
            })),
            second_uri: format!("/dashboards/{edited}"),
            second_body: Some(json!({
                "dashboardName": "SecondUpdate",
                "dashboardDefinition": widget_definition,
            })),
            token_in_query: false,
            status: 200,
            after_first: Reads::Reports {
                read_uri: format!("/dashboards/{edited}"),
                members: vec![
                    ("dashboardName", json!("FirstUpdate")),
                    ("dashboardDefinition", json!("{\"widgets\":[]}")),
                ],
            },
            after_second: Reads::Reports {
                read_uri: format!("/dashboards/{edited}"),
                members: vec![
                    ("dashboardName", json!("SecondUpdate")),
                    ("dashboardDefinition", json!(widget_definition)),
                ],
            },
        },
        Row {
            case: "UpdateAccessPolicy differing in the permission",
            method: "PUT",
            first_uri: format!("/access-policies/{promoted}"),
            first_body: Some(json!({
                "accessPolicyIdentity": identity.clone(),
                "accessPolicyResource": resource.clone(),
                "accessPolicyPermission": "VIEWER",
            })),
            second_uri: format!("/access-policies/{promoted}"),
            second_body: Some(json!({
                "accessPolicyIdentity": identity,
                "accessPolicyResource": resource,
                "accessPolicyPermission": "ADMINISTRATOR",
            })),
            token_in_query: false,
            status: 200,
            after_first: Reads::Reports {
                read_uri: format!("/access-policies/{promoted}"),
                members: vec![("accessPolicyPermission", json!("VIEWER"))],
            },
            after_second: Reads::Reports {
                read_uri: format!("/access-policies/{promoted}"),
                members: vec![("accessPolicyPermission", json!("ADMINISTRATOR"))],
            },
        },
        Row {
            case: "DeleteDashboard addressed at another dashboard",
            method: "DELETE",
            first_uri: format!("/dashboards/{first_dashboard}"),
            first_body: None,
            second_uri: format!("/dashboards/{second_dashboard}"),
            second_body: None,
            token_in_query: true,
            status: 204,
            after_first: Reads::Gone {
                read_uri: format!("/dashboards/{first_dashboard}"),
            },
            after_second: Reads::Gone {
                read_uri: format!("/dashboards/{second_dashboard}"),
            },
        },
        Row {
            case: "DeleteProject addressed at another project",
            method: "DELETE",
            first_uri: format!("/projects/{first_doomed_project}"),
            first_body: None,
            second_uri: format!("/projects/{second_doomed_project}"),
            second_body: None,
            token_in_query: true,
            status: 204,
            after_first: Reads::Gone {
                read_uri: format!("/projects/{first_doomed_project}"),
            },
            after_second: Reads::Gone {
                read_uri: format!("/projects/{second_doomed_project}"),
            },
        },
        Row {
            case: "DeleteAccessPolicy addressed at another policy",
            method: "DELETE",
            first_uri: format!("/access-policies/{first_doomed_policy}"),
            first_body: None,
            second_uri: format!("/access-policies/{second_doomed_policy}"),
            second_body: None,
            token_in_query: true,
            status: 204,
            after_first: Reads::Gone {
                read_uri: format!("/access-policies/{first_doomed_policy}"),
            },
            after_second: Reads::Gone {
                read_uri: format!("/access-policies/{second_doomed_policy}"),
            },
        },
        Row {
            case: "BatchAssociateProjectAssets differing in the assets",
            method: "POST",
            first_uri: format!("/projects/{growing}/assets/associate"),
            first_body: Some(json!({ "assetIds": [first_asset.clone()] })),
            second_uri: format!("/projects/{growing}/assets/associate"),
            second_body: Some(json!({ "assetIds": [second_asset.clone()] })),
            token_in_query: false,
            status: 200,
            after_first: Reads::Assets {
                read_uri: format!("/projects/{growing}/assets"),
                asset_ids: vec![first_asset.clone()],
            },
            after_second: Reads::Assets {
                read_uri: format!("/projects/{growing}/assets"),
                asset_ids: vec![first_asset.clone(), second_asset.clone()],
            },
        },
        Row {
            case: "BatchDisassociateProjectAssets differing in the assets",
            method: "POST",
            first_uri: format!("/projects/{shrinking}/assets/disassociate"),
            first_body: Some(json!({ "assetIds": [first_asset.clone()] })),
            second_uri: format!("/projects/{shrinking}/assets/disassociate"),
            second_body: Some(json!({ "assetIds": [second_asset.clone()] })),
            token_in_query: false,
            status: 200,
            after_first: Reads::Assets {
                read_uri: format!("/projects/{shrinking}/assets"),
                asset_ids: vec![second_asset.clone()],
            },
            after_second: Reads::Assets {
                read_uri: format!("/projects/{shrinking}/assets"),
                asset_ids: Vec::new(),
            },
        },
    ]
}

#[tokio::test]
async fn a_request_differing_from_the_call_its_token_names_is_carried_out() {
    let (router, _engine) = router_with_engine();
    let portal_id = create_portal(&router, "spent-token-portal").await;

    for row in rows(&router, &portal_id).await {
        let Row {
            case,
            method,
            first_uri,
            first_body,
            second_uri,
            second_body,
            token_in_query,
            status: modelled,
            after_first,
            after_second,
        } = row;
        let token = token_for(case);

        let (uri, body) = carrying(&first_uri, first_body.as_ref(), &token, token_in_query);
        let (status, served) = call(&router, method, &uri, body.clone()).await;
        assert_eq!(
            status, modelled,
            "case {case:?}: the first call got {status} {served}"
        );
        assert_reads(&router, case, "after the first call", &after_first).await;

        // The RETRY: the SAME request under the same token is answered from the
        // call it repeats and applies nothing a second time.
        let (status, replay) = call(&router, method, &uri, body).await;
        assert_eq!(
            (status, &replay),
            (modelled, &served),
            "case {case:?}: a retry must be answered like the call it repeats, got {status} \
             {replay}"
        );
        assert_reads(&router, case, "after the retry", &after_first).await;

        // A DIFFERENT request under that same, spent, token: a different call,
        // which has to be carried out rather than answered from the first one.
        let (uri, body) = carrying(&second_uri, second_body.as_ref(), &token, token_in_query);
        let (status, answered) = call(&router, method, &uri, body).await;
        assert_eq!(
            status, modelled,
            "case {case:?}: a request differing from the call its token names must be served, \
             got {status} {answered}"
        );
        assert_reads(&router, case, "after the differing request", &after_second).await;
    }
}
