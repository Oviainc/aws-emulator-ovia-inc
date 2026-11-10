// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the `clientToken` the Monitor, dataset, delete and batch
//! operations take, driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a real
//! client sees.
//!
//! The rule these suites pin: a mutating request that carries a `clientToken` is
//! served ONCE. A repeat of it under the SAME token is a RETRY of that call: it is
//! answered from what the first call left behind — the resource it minted, the
//! answer it gave — and changes nothing further, so it mints no second resource,
//! applies no mutation twice and is no not-found refusal just because the first
//! call removed what it addressed. A token already spent on a DIFFERENT request is
//! refused `ConflictingOperationException` (409) naming the resource the first
//! call acted on, where the operation declares that error; where it declares no
//! conflict error at all, that request is carried out as the call it is, so a
//! CREATE mints a resource of its own and answers with ITS identifiers — a
//! create's answer carries nothing but the resource it minted, so answering from
//! the earlier call would name a resource this request never asked for. A request
//! carrying NO token is a new call every time.
//!
//! What a token spent on a different request means for the operations whose
//! answer carries nothing of the call the token names is pinned by the
//! `client_token_spent_on_another_request` suite beside this one, and for the
//! Monitor creates by `client_token_spent_on_another_create`.
//!
//! The rows span what the rule generalises over: the operation kind (create,
//! update, delete, batch association, batch data-segment write), the resource kind
//! (portal, project, access policy, dashboard, dataset, asset, asset model,
//! workspace, time series), how the token travels (in the payload, on the query
//! string), the answer shape (a minted id, an empty body, a lifecycle status, a
//! per-entry report) and which member a mismatched replay differs in.
//!
//! The HTTP harness (`call`, `error_code`, `router_with_engine`, `create_model`,
//! `create_asset`), the token helpers (`client_token`, `with_token`) and the
//! Monitor, workspace and dataset creates are the ones the sibling suites define;
//! all are reused rather than copied.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, DataSegmentRecord};

use crate::access_policy_lifecycle::create_portal;
use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, router_with_engine, REGION,
};
use crate::client_token_idempotency::{client_token, with_token};
use crate::dataset_data_segments::{create_dataset, ingest, instant, time_series_id, wire_instant};
use crate::search_lifecycle::create_workspace;

/// The workspace the dataset rows live in.
const WORKSPACE: &str = "replay-line";

/// The aliases of the data streams the batch rows name slices of.
const FIRST_ALIAS: &str = "/replay/line/1/thickness";
const SECOND_ALIAS: &str = "/replay/line/2/thickness";

/// The seconds, relative to the suites' anchor, each stream is ingested with.
const SAMPLES: &[i64] = &[0, 5, 10, 15];

/// A `ClientToken`-shaped value for the row `case`.
///
/// The row labels read as prose and the modelled `ClientToken` pattern
/// `^\S{36,64}$` admits no whitespace (smithy_query query="operation"
/// operation="CreatePortal"), so every character outside that pattern becomes a
/// hyphen before the shared helper pads the seed out to the modelled minimum.
pub(crate) fn token_for(case: &str) -> String {
    let seed: String = case
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    client_token(&seed)
}

/// What a request reusing a token already spent on a DIFFERENT request must get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reuse {
    /// The modelled `ConflictingOperationException` (409), naming the resource
    /// the first call acted on.
    Conflict,
    /// Served as the create it is: the operation declares no error that states a
    /// token conflict, and its answer carries nothing of the earlier call, so the
    /// request mints a resource of its own.
    Served,
}

/// Which map of the store a row's resources are counted in.
///
/// `pub(crate)` beside [`held`] because the sibling token suites count the same
/// maps, so they share one definition rather than growing a second.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Kind {
    Portal,
    Project,
    AccessPolicy,
    Dataset,
    Dashboard,
}

/// How many resources of `kind` the store holds — the only way to tell a retry
/// minted nothing from one that answered the same id twice.
pub(crate) fn held(engine: &Arc<engine::InMemoryEngine>, kind: Kind) -> usize {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let store = state.lock();
    match kind {
        Kind::Portal => store.portals.len(),
        Kind::Project => store.projects.len(),
        Kind::AccessPolicy => store.access_policies.len(),
        Kind::Dataset => store.datasets.len(),
        Kind::Dashboard => store.dashboards.len(),
    }
}

/// One row of the create table.
struct CreateRow {
    case: &'static str,
    kind: Kind,
    path: &'static str,
    /// The status the operation models for a served create.
    status: u16,
    body: Value,
    /// The same request with ONE member different, which is a DIFFERENT request.
    changed: Value,
    id_member: &'static str,
    arn_member: &'static str,
    reuse: Reuse,
}

/// The create rows: every create of this crate that had been dropping the token,
/// over the Monitor resources and the dataset, each with the member a mismatched
/// replay differs in.
fn create_rows(portal_id: &str) -> Vec<CreateRow> {
    let role_arn = "arn:aws:iam::123456789012:role/service-role-monitor";
    vec![
        CreateRow {
            case: "CreatePortal, differing in the portal name",
            kind: Kind::Portal,
            path: "/portals",
            status: 202,
            body: json!({
                "portalName": "ReplayPortal",
                "portalContactEmail": "admin@example.com",
                "roleArn": role_arn,
            }),
            changed: json!({
                "portalName": "ReplayPortal renamed",
                "portalContactEmail": "admin@example.com",
                "roleArn": role_arn,
            }),
            id_member: "portalId",
            arn_member: "portalArn",
            reuse: Reuse::Served,
        },
        CreateRow {
            case: "CreateProject, differing in the description",
            kind: Kind::Project,
            path: "/projects",
            status: 201,
            body: json!({ "portalId": portal_id, "projectName": "ReplayProject" }),
            changed: json!({
                "portalId": portal_id,
                "projectName": "ReplayProject",
                "projectDescription": "a description the first call did not carry",
            }),
            id_member: "projectId",
            arn_member: "projectArn",
            reuse: Reuse::Served,
        },
        CreateRow {
            case: "CreateAccessPolicy, differing in the permission",
            kind: Kind::AccessPolicy,
            path: "/access-policies",
            status: 201,
            body: json!({
                "accessPolicyIdentity": { "user": { "id": "u-0123456789abcdef" } },
                "accessPolicyResource": { "portal": { "id": portal_id } },
                "accessPolicyPermission": "VIEWER",
            }),
            changed: json!({
                "accessPolicyIdentity": { "user": { "id": "u-0123456789abcdef" } },
                "accessPolicyResource": { "portal": { "id": portal_id } },
                "accessPolicyPermission": "ADMINISTRATOR",
            }),
            id_member: "accessPolicyId",
            arn_member: "accessPolicyArn",
            reuse: Reuse::Served,
        },
        CreateRow {
            case: "CreateDataset, differing in the tags",
            kind: Kind::Dataset,
            path: "/datasets",
            status: 202,
            body: json!({
                "datasetName": "ReplayDataset",
                "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
            }),
            changed: json!({
                "datasetName": "ReplayDataset",
                "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
                "tags": { "stage": "test" },
            }),
            id_member: "datasetId",
            arn_member: "datasetArn",
            reuse: Reuse::Conflict,
        },
    ]
}

#[tokio::test]
async fn a_retried_create_replays_the_first_answer_and_mints_nothing_more() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "replay-create-portal").await;

    for row in create_rows(&portal_id) {
        let CreateRow {
            case,
            kind,
            path,
            status: modelled,
            body,
            changed,
            id_member,
            arn_member,
            reuse,
        } = row;
        let token = token_for(case);

        let before = held(&engine, kind);
        let (status, first) = call(&router, "POST", path, Some(with_token(&body, &token))).await;
        assert_eq!(
            status, modelled,
            "case {case:?}: the create got {status} {first}"
        );
        let after_first = held(&engine, kind);
        assert_eq!(
            after_first,
            before + 1,
            "case {case:?}: the create must mint exactly one resource, {before} -> {after_first}"
        );

        // The retry: the same request under the same token.
        let (replay_status, replay) =
            call(&router, "POST", path, Some(with_token(&body, &token))).await;
        assert_eq!(
            replay_status, status,
            "case {case:?}: a retry must be answered like the call it repeats, got \
             {replay_status} {replay}"
        );
        for member in [id_member, arn_member] {
            assert_eq!(
                replay[member], first[member],
                "case {case:?}: a retry must replay {member}, got {replay} after {first}"
            );
        }
        assert_eq!(
            held(&engine, kind),
            after_first,
            "case {case:?}: a retry must mint nothing, got {} resources",
            held(&engine, kind)
        );

        // A DIFFERENT request reusing the spent token.
        let (reused_status, reused) =
            call(&router, "POST", path, Some(with_token(&changed, &token))).await;
        let after_reuse = match reuse {
            Reuse::Conflict => {
                assert_eq!(
                    (reused_status, error_code(&reused)),
                    (409, "ConflictingOperationException"),
                    "case {case:?}: a token spent on another request must be refused, got \
                     {reused_status} {reused}"
                );
                assert_eq!(
                    reused["resourceId"], first[id_member],
                    "case {case:?}: the refusal must name the resource the token was spent \
                     on, got {reused} after {first}"
                );
                assert_eq!(
                    reused["resourceArn"], first[arn_member],
                    "case {case:?}: the refusal must carry that resource's ARN, got {reused} \
                     after {first}"
                );
                // A refused create wrote nothing.
                after_first
            }
            Reuse::Served => {
                assert_eq!(
                    reused_status, status,
                    "case {case:?}: a token this operation models no conflict for must serve \
                     the differing request, got {reused_status} {reused}"
                );
                for member in [id_member, arn_member] {
                    assert_ne!(
                        reused[member], first[member],
                        "case {case:?}: the differing request must answer with its OWN \
                         {member}, got {reused} after {first}"
                    );
                }
                after_first + 1
            }
        };
        assert_eq!(
            held(&engine, kind),
            after_reuse,
            "case {case:?}: reusing a spent token must leave {after_reuse} resources, got {}",
            held(&engine, kind)
        );

        // The same changed request under a FRESH token is simply a new call.
        let fresh = token_for(&format!("{case}-fresh"));
        let (fresh_status, created) =
            call(&router, "POST", path, Some(with_token(&changed, &fresh))).await;
        assert_eq!(
            fresh_status, status,
            "case {case:?}: a fresh token must serve the changed request, got {fresh_status} \
             {created}"
        );
        assert_ne!(
            created[id_member], first[id_member],
            "case {case:?}: a fresh token must mint a new resource, got {created} after {first}"
        );
        let after_fresh = held(&engine, kind);
        assert_eq!(
            after_fresh,
            after_reuse + 1,
            "case {case:?}: the fresh-token call must mint one resource, {after_reuse} -> \
             {after_fresh}"
        );
    }
}

/// One row of the update table: a resource, the update that rewrites it, a SECOND
/// update that rewrites it again, and the member a read reports it by.
struct UpdateRow {
    case: &'static str,
    uri: String,
    read_uri: String,
    member: &'static str,
    /// The update the row's token is spent on.
    first: Value,
    /// A later update of the same resource, under a token of its own, whose write
    /// a retry of the first must NOT undo.
    second: Value,
    first_value: &'static str,
    second_value: &'static str,
    status: u16,
}

/// The rows, each with the resource it updates created first.
async fn update_rows(router: &MultiServiceRouter, portal_id: &str) -> Vec<UpdateRow> {
    let (status, project) = call(
        router,
        "POST",
        "/projects",
        Some(json!({ "portalId": portal_id, "projectName": "BeforeAnyUpdate" })),
    )
    .await;
    assert_eq!(status, 201, "CreateProject: got {status} {project}");
    let project_id = project["projectId"].as_str().unwrap_or_default().to_string();

    let (status, dashboard) = call(
        router,
        "POST",
        "/dashboards",
        Some(json!({
            "projectId": project_id,
            "dashboardName": "BeforeAnyUpdate",
            "dashboardDefinition": "{\"widgets\":[]}",
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateDashboard: got {status} {dashboard}");
    let dashboard_id = dashboard["dashboardId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let identity = json!({ "user": { "id": "u-fedcba9876543210" } });
    let resource = json!({ "portal": { "id": portal_id } });
    let (status, policy) = call(
        router,
        "POST",
        "/access-policies",
        Some(json!({
            "accessPolicyIdentity": identity.clone(),
            "accessPolicyResource": resource.clone(),
            "accessPolicyPermission": "VIEWER",
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateAccessPolicy: got {status} {policy}");
    let policy_id = policy["accessPolicyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let dataset_id = create_dataset(router, "BeforeAnyUpdate", None, None).await;
    let source = json!({ "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" });

    vec![
        UpdateRow {
            case: "UpdateProject",
            uri: format!("/projects/{project_id}"),
            read_uri: format!("/projects/{project_id}"),
            member: "projectName",
            first: json!({ "projectName": "FirstUpdate" }),
            second: json!({ "projectName": "SecondUpdate" }),
            first_value: "FirstUpdate",
            second_value: "SecondUpdate",
            status: 200,
        },
        UpdateRow {
            case: "UpdateDashboard",
            uri: format!("/dashboards/{dashboard_id}"),
            read_uri: format!("/dashboards/{dashboard_id}"),
            member: "dashboardName",
            first: json!({
                "dashboardName": "FirstUpdate",
                "dashboardDefinition": "{\"widgets\":[]}",
            }),
            second: json!({
                "dashboardName": "SecondUpdate",
                "dashboardDefinition": "{\"widgets\":[]}",
            }),
            first_value: "FirstUpdate",
            second_value: "SecondUpdate",
            status: 200,
        },
        UpdateRow {
            case: "UpdateAccessPolicy",
            uri: format!("/access-policies/{policy_id}"),
            read_uri: format!("/access-policies/{policy_id}"),
            member: "accessPolicyPermission",
            first: json!({
                "accessPolicyIdentity": identity.clone(),
                "accessPolicyResource": resource.clone(),
                "accessPolicyPermission": "VIEWER",
            }),
            second: json!({
                "accessPolicyIdentity": identity,
                "accessPolicyResource": resource,
                "accessPolicyPermission": "ADMINISTRATOR",
            }),
            first_value: "VIEWER",
            second_value: "ADMINISTRATOR",
            status: 200,
        },
        UpdateRow {
            case: "UpdateDataset",
            uri: format!("/datasets/{dataset_id}"),
            read_uri: format!("/datasets/{dataset_id}"),
            member: "datasetName",
            first: json!({ "datasetName": "FirstUpdate", "datasetSource": source.clone() }),
            second: json!({ "datasetName": "SecondUpdate", "datasetSource": source }),
            first_value: "FirstUpdate",
            second_value: "SecondUpdate",
            status: 202,
        },
    ]
}

#[tokio::test]
async fn a_retried_update_writes_nothing_and_leaves_the_later_write_standing() {
    let (router, _engine) = router_with_engine();
    let portal_id = create_portal(&router, "replay-update-portal").await;

    for row in update_rows(&router, &portal_id).await {
        let UpdateRow {
            case,
            uri,
            read_uri,
            member,
            first,
            second,
            first_value,
            second_value,
            status: modelled,
        } = row;
        let token = token_for(case);

        let (status, answered) = call(&router, "PUT", &uri, Some(with_token(&first, &token))).await;
        assert_eq!(
            status, modelled,
            "case {case:?}: the update got {status} {answered}"
        );
        let (status, read) = call(&router, "GET", &read_uri, None).await;
        assert_eq!(status, 200, "case {case:?}: the read got {status} {read}");
        assert_eq!(
            read[member].as_str(),
            Some(first_value),
            "case {case:?}: the accepted update must be what a read answers, got {read}"
        );

        // A LATER update of the same resource, under a token of its own.
        let later = token_for(&format!("{case}-later"));
        let (status, answered) =
            call(&router, "PUT", &uri, Some(with_token(&second, &later))).await;
        assert_eq!(
            status, modelled,
            "case {case:?}: the second update got {status} {answered}"
        );

        // The RETRY of the first update: it is answered from the call it repeats
        // and writes nothing, so the later write still stands.
        let (status, replay) = call(&router, "PUT", &uri, Some(with_token(&first, &token))).await;
        assert_eq!(
            status, modelled,
            "case {case:?}: a retried update must be answered like the call it repeats, got \
             {status} {replay}"
        );
        let (status, read) = call(&router, "GET", &read_uri, None).await;
        assert_eq!(status, 200, "case {case:?}: the read got {status} {read}");
        assert_eq!(
            read[member].as_str(),
            Some(second_value),
            "case {case:?}: a retry must apply nothing a second time, got {read}"
        );
    }
}

/// One row of the delete table: the request that removes a resource, with the
/// token left out, and how that request carries one.
struct DeleteRow {
    case: &'static str,
    method: &'static str,
    uri: String,
    /// Whether `clientToken` travels on the query string (`@httpQuery`) rather
    /// than in the payload.
    token_in_query: bool,
    /// The payload the request carries, for the operations that take one.
    body: Option<Value>,
    status: u16,
}

impl DeleteRow {
    /// This row's request carrying `token`.
    fn with_token(&self, token: &str) -> (String, Option<Value>) {
        carrying(&self.uri, self.body.as_ref(), token, self.token_in_query)
    }
}

/// The request `uri`/`body` carrying `token`, where `in_query` says whether
/// `clientToken` travels on the query string (`@httpQuery`) rather than in the
/// payload.
pub(crate) fn carrying(
    uri: &str,
    body: Option<&Value>,
    token: &str,
    in_query: bool,
) -> (String, Option<Value>) {
    if in_query {
        let separator = if uri.contains('?') { '&' } else { '?' };
        (
            format!("{uri}{separator}clientToken={token}"),
            body.cloned(),
        )
    } else {
        let carried = body.cloned().unwrap_or_else(|| json!({}));
        (uri.to_string(), Some(with_token(&carried, token)))
    }
}

/// The rows, each with the resource it removes created first. They span both ways
/// a delete carries the token and every answer shape a delete of this service
/// gives: a lifecycle status, an id with one, and an empty body.
async fn delete_rows(router: &MultiServiceRouter) -> Vec<DeleteRow> {
    let model_id = create_model(router, "ReplayDeleteModel", json!([])).await;
    let asset_id = create_asset(router, "ReplayDeleteAsset", &model_id).await;
    let doomed_model = create_model(router, "ReplayDoomedModel", json!([])).await;
    let portal_id = create_portal(router, "replay-delete-portal").await;
    let project_portal = create_portal(router, "replay-delete-project-portal").await;

    let (status, project) = call(
        router,
        "POST",
        "/projects",
        Some(json!({ "portalId": project_portal, "projectName": "ReplayDeleteProject" })),
    )
    .await;
    assert_eq!(status, 201, "CreateProject: got {status} {project}");
    let project_id = project["projectId"].as_str().unwrap_or_default().to_string();

    let (status, policy) = call(
        router,
        "POST",
        "/access-policies",
        Some(json!({
            "accessPolicyIdentity": { "user": { "id": "u-1122334455667788" } },
            "accessPolicyResource": { "portal": { "id": project_portal } },
            "accessPolicyPermission": "VIEWER",
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateAccessPolicy: got {status} {policy}");
    let policy_id = policy["accessPolicyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The dashboard goes in a project of its OWN: the `DeleteProject` row below
    // removes the project it addresses with every dashboard that project holds, and
    // a row must not remove the resource another row addresses.
    let (status, dashboard_project) = call(
        router,
        "POST",
        "/projects",
        Some(json!({ "portalId": project_portal, "projectName": "ReplayDashboardProject" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateProject for the dashboard: got {status} {dashboard_project}"
    );
    let dashboard_project_id = dashboard_project["projectId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let (status, dashboard) = call(
        router,
        "POST",
        "/dashboards",
        Some(json!({
            "projectId": dashboard_project_id,
            "dashboardName": "ReplayDeleteDashboard",
            "dashboardDefinition": "{\"widgets\":[]}",
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateDashboard: got {status} {dashboard}");
    let dashboard_id = dashboard["dashboardId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    create_workspace(router, "replay-delete-workspace").await;
    ingest(router, FIRST_ALIAS, SAMPLES).await;

    vec![
        DeleteRow {
            case: "DeleteAsset",
            method: "DELETE",
            uri: format!("/assets/{asset_id}"),
            token_in_query: true,
            body: None,
            status: 202,
        },
        DeleteRow {
            case: "DeleteAssetModel",
            method: "DELETE",
            uri: format!("/asset-models/{doomed_model}"),
            token_in_query: true,
            body: None,
            status: 202,
        },
        DeleteRow {
            case: "DeletePortal",
            method: "DELETE",
            uri: format!("/portals/{portal_id}"),
            token_in_query: true,
            body: None,
            status: 202,
        },
        DeleteRow {
            case: "DeleteProject",
            method: "DELETE",
            uri: format!("/projects/{project_id}"),
            token_in_query: true,
            body: None,
            status: 204,
        },
        DeleteRow {
            case: "DeleteDashboard",
            method: "DELETE",
            uri: format!("/dashboards/{dashboard_id}"),
            token_in_query: true,
            body: None,
            status: 204,
        },
        DeleteRow {
            case: "DeleteAccessPolicy",
            method: "DELETE",
            uri: format!("/access-policies/{policy_id}"),
            token_in_query: true,
            body: None,
            status: 204,
        },
        DeleteRow {
            case: "DeleteWorkspace",
            method: "DELETE",
            uri: "/workspaces/replay-delete-workspace".to_string(),
            token_in_query: true,
            body: None,
            status: 200,
        },
        DeleteRow {
            case: "DeleteTimeSeries",
            method: "POST",
            uri: format!("/timeseries/delete?alias={FIRST_ALIAS}"),
            token_in_query: false,
            body: None,
            status: 200,
        },
    ]
}

#[tokio::test]
async fn a_retried_delete_replays_its_answer_where_an_untokened_repeat_is_a_not_found() {
    let (router, _engine) = router_with_engine();

    for row in delete_rows(&router).await {
        let token = token_for(row.case);
        let case = row.case;
        let (uri, body) = row.with_token(&token);

        let (status, first) = call(&router, row.method, &uri, body.clone()).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: the delete got {status} {first}"
        );

        // The retry: the same request under the same token. It answers from the
        // call it repeats rather than from a store that no longer holds the
        // resource that call removed.
        let (replay_status, replay) = call(&router, row.method, &uri, body).await;
        assert_eq!(
            replay_status, row.status,
            "case {case:?}: a retried delete must be answered like the call it repeats, got \
             {replay_status} {replay}"
        );
        assert_eq!(
            replay, first,
            "case {case:?}: a retry must replay the first answer, got {replay} after {first}"
        );

        // A request carrying NO token is a new call, and the resource is gone.
        let (refused_status, refused) =
            call(&router, row.method, &row.uri, row.body.clone()).await;
        assert_eq!(
            (refused_status, error_code(&refused)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a repeat carrying no token must be the modelled not-found, got \
             {refused_status} {refused}"
        );
    }
}

/// One row of the conflict table: a request served under a token, the SAME
/// operation addressed at another resource under that same token, and the
/// resource the refusal must name.
struct ConflictRow {
    case: &'static str,
    method: &'static str,
    first_uri: String,
    second_uri: String,
    /// The payload both requests carry, for the operations that take one.
    body: Option<Value>,
    token_in_query: bool,
    status: u16,
    /// What `resourceId` and `resourceArn` must name: the resource the token was
    /// spent on.
    resource_id: String,
    resource_arn: String,
    /// A read of the resource the refused request addressed, which must still
    /// answer: a refused request changes nothing.
    read_uri: String,
}

/// An ARN of this service as an INDEPENDENT oracle: built here from the region
/// the harness signs for and the emulator's account id, rather than from the
/// helper the handlers use.
fn expected_arn(resource: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:{resource}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The conflict rows, each with the two resources it addresses created first.
/// They span the answer shapes a served call of these operations gives — an id
/// with a lifecycle status, a status alone, an empty body, a full record — and
/// both ways the token travels.
async fn conflict_rows(router: &MultiServiceRouter) -> Vec<ConflictRow> {
    let model_id = create_model(router, "ConflictModel", json!([])).await;
    let first_asset = create_asset(router, "ConflictAssetOne", &model_id).await;
    let second_asset = create_asset(router, "ConflictAssetTwo", &model_id).await;
    let first_portal = create_portal(router, "conflict-portal-one").await;
    let second_portal = create_portal(router, "conflict-portal-two").await;
    ingest(router, FIRST_ALIAS, SAMPLES).await;
    ingest(router, SECOND_ALIAS, SAMPLES).await;
    let first_series = time_series_id(router, FIRST_ALIAS).await;
    let first_dataset = create_dataset(router, "ConflictDatasetOne", None, None).await;
    let second_dataset = create_dataset(router, "ConflictDatasetTwo", None, None).await;
    let source = json!({ "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" });

    vec![
        ConflictRow {
            case: "DeleteAsset addressed at another asset",
            method: "DELETE",
            first_uri: format!("/assets/{first_asset}"),
            second_uri: format!("/assets/{second_asset}"),
            body: None,
            token_in_query: true,
            status: 202,
            resource_id: first_asset.clone(),
            resource_arn: expected_arn(&format!("asset/{first_asset}")),
            read_uri: format!("/assets/{second_asset}"),
        },
        ConflictRow {
            case: "DeletePortal addressed at another portal",
            method: "DELETE",
            first_uri: format!("/portals/{first_portal}"),
            second_uri: format!("/portals/{second_portal}"),
            body: None,
            token_in_query: true,
            status: 202,
            resource_id: first_portal.clone(),
            resource_arn: expected_arn(&format!("portal/{first_portal}")),
            read_uri: format!("/portals/{second_portal}"),
        },
        ConflictRow {
            case: "DeleteTimeSeries addressed at another stream",
            method: "POST",
            first_uri: format!("/timeseries/delete?alias={FIRST_ALIAS}"),
            second_uri: format!("/timeseries/delete?alias={SECOND_ALIAS}"),
            body: None,
            token_in_query: false,
            status: 200,
            resource_id: first_series.clone(),
            resource_arn: expected_arn(&format!("time-series/{first_series}")),
            read_uri: format!("/timeseries/describe?alias={SECOND_ALIAS}"),
        },
        ConflictRow {
            case: "UpdateDataset addressed at another dataset",
            method: "PUT",
            first_uri: format!("/datasets/{first_dataset}"),
            second_uri: format!("/datasets/{second_dataset}"),
            body: Some(json!({ "datasetName": "Renamed", "datasetSource": source })),
            token_in_query: false,
            status: 202,
            resource_id: first_dataset.clone(),
            resource_arn: expected_arn(&format!("dataset/{first_dataset}")),
            read_uri: format!("/datasets/{second_dataset}"),
        },
    ]
}

#[tokio::test]
async fn a_token_spent_on_another_request_is_the_modelled_conflict_and_changes_nothing() {
    let (router, _engine) = router_with_engine();

    for row in conflict_rows(&router).await {
        let case = row.case;
        let token = token_for(case);

        let (uri, body) = carrying(&row.first_uri, row.body.as_ref(), &token, row.token_in_query);
        let (status, served) = call(&router, row.method, &uri, body).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: the first call got {status} {served}"
        );

        // The SAME operation, addressed at another resource, under the token the
        // call above spent.
        let (uri, body) = carrying(&row.second_uri, row.body.as_ref(), &token, row.token_in_query);
        let (status, refused) = call(&router, row.method, &uri, body).await;
        assert_eq!(
            (status, error_code(&refused)),
            (409, "ConflictingOperationException"),
            "case {case:?}: a token spent on another request must be refused, got {status} \
             {refused}"
        );
        assert_eq!(
            refused["resourceId"].as_str(),
            Some(row.resource_id.as_str()),
            "case {case:?}: the refusal must name the resource the token was spent on, got \
             {refused}"
        );
        assert_eq!(
            refused["resourceArn"].as_str(),
            Some(row.resource_arn.as_str()),
            "case {case:?}: the refusal must carry that resource's ARN, got {refused}"
        );

        // The refused request changed nothing: the resource it addressed is there.
        let (status, read) = call(&router, "GET", &row.read_uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused request must leave its resource alone, got {status} \
             {read}"
        );
    }
}

#[tokio::test]
async fn a_retried_project_asset_batch_does_not_apply_itself_a_second_time() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "replay-project-assets-portal").await;
    let (status, project) = call(
        &router,
        "POST",
        "/projects",
        Some(json!({ "portalId": portal_id, "projectName": "ReplayProjectAssets" })),
    )
    .await;
    assert_eq!(status, 201, "CreateProject: got {status} {project}");
    let project_id = project["projectId"].as_str().unwrap_or_default().to_string();
    let model_id = create_model(&router, "ReplayProjectAssetModel", json!([])).await;
    let asset_id = create_asset(&router, "ReplayProjectAsset", &model_id).await;

    let membership = |label: &str| -> Vec<String> {
        let state = service_state(&*engine).expect("the IoT SiteWise store");
        let store = state.lock();
        let held = store
            .projects
            .get(&project_id)
            .map(|project| project.asset_ids.iter().cloned().collect::<Vec<String>>())
            .unwrap_or_default();
        assert!(
            store.projects.contains_key(&project_id),
            "case {label:?}: the project must still be there"
        );
        held
    };

    let associate_uri = format!("/projects/{project_id}/assets/associate");
    let disassociate_uri = format!("/projects/{project_id}/assets/disassociate");
    let request = json!({ "assetIds": [asset_id.clone()] });

    // The association, and its retry after the asset has been taken out again: the
    // retry answers from the call it repeats, so it puts nothing back.
    let add = token_for("batch-associate-project-assets");
    let (status, first) = call(
        &router,
        "POST",
        &associate_uri,
        Some(with_token(&request, &add)),
    )
    .await;
    assert_eq!(status, 200, "the association got {status} {first}");
    assert_eq!(
        membership("after the association"),
        vec![asset_id.clone()],
        "the association must put the asset in the project"
    );

    let remove = token_for("batch-disassociate-project-assets");
    let (status, removed) = call(
        &router,
        "POST",
        &disassociate_uri,
        Some(with_token(&request, &remove)),
    )
    .await;
    assert_eq!(status, 200, "the disassociation got {status} {removed}");
    assert!(
        membership("after the disassociation").is_empty(),
        "the disassociation must take the asset out"
    );

    let (status, replay) = call(
        &router,
        "POST",
        &associate_uri,
        Some(with_token(&request, &add)),
    )
    .await;
    assert_eq!(
        (status, &replay),
        (200, &first),
        "a retried association must replay the first answer, got {status} {replay}"
    );
    assert!(
        membership("after the retried association").is_empty(),
        "a retried association must write nothing, so the asset stays out"
    );

    // The mirror: the retry of the disassociation takes nothing out of the
    // membership a later call put back.
    let (status, again) = call(
        &router,
        "POST",
        &associate_uri,
        Some(with_token(&request, &token_for("associate-again"))),
    )
    .await;
    assert_eq!(status, 200, "the second association got {status} {again}");
    let (status, replay) = call(
        &router,
        "POST",
        &disassociate_uri,
        Some(with_token(&request, &remove)),
    )
    .await;
    assert_eq!(
        (status, &replay),
        (200, &removed),
        "a retried disassociation must replay the first answer, got {status} {replay}"
    );
    assert_eq!(
        membership("after the retried disassociation"),
        vec![asset_id],
        "a retried disassociation must write nothing, so the asset stays in"
    );
}

#[tokio::test]
async fn a_retried_data_segment_batch_replays_its_own_per_entry_answer() {
    let (router, engine) = router_with_engine();
    create_workspace(&router, WORKSPACE).await;
    let curated = create_dataset(&router, "ReplayCurated", Some("CURATED"), Some(WORKSPACE)).await;
    let session = create_dataset(&router, "ReplaySession", Some("SESSION"), Some(WORKSPACE)).await;
    ingest(&router, FIRST_ALIAS, SAMPLES).await;
    ingest(&router, SECOND_ALIAS, SAMPLES).await;
    let first_series = time_series_id(&router, FIRST_ALIAS).await;
    let second_series = time_series_id(&router, SECOND_ALIAS).await;

    // One entry the call can apply and one the request itself states wrongly (a
    // span covering no time), so the answer a retry has to replay carries both a
    // stored segment and a per-entry failure.
    let entries = json!([
        {
            "sourceDatasetId": session,
            "timeSeriesId": first_series,
            "startTimestamp": wire_instant(0, 0),
            "endTimestamp": wire_instant(10, 0),
        },
        {
            "sourceDatasetId": session,
            "timeSeriesId": second_series,
            "startTimestamp": wire_instant(10, 0),
            "endTimestamp": wire_instant(10, 0),
        },
    ]);
    let associate_uri = format!("/datasets/{curated}/data-segments/associate");
    let associate = json!({
        "workspaceName": WORKSPACE,
        "associateDataSegmentEntries": entries.clone(),
    });
    let token = token_for("batch-associate-data-segments");
    let (status, first) = call(
        &router,
        "POST",
        &associate_uri,
        Some(with_token(&associate, &token)),
    )
    .await;
    assert_eq!(status, 200, "the association got {status} {first}");
    assert_eq!(
        first["failedAssociations"].as_array().map(Vec::len),
        Some(1),
        "the association must report exactly the entry it could not apply, got {first}"
    );
    let stored = segment_count(&engine, &curated);

    let (replay_status, replay) = call(
        &router,
        "POST",
        &associate_uri,
        Some(with_token(&associate, &token)),
    )
    .await;
    assert_eq!(
        (replay_status, &replay),
        (200, &first),
        "a retried association must replay its own per-entry answer, got {replay_status} \
         {replay}"
    );
    assert_eq!(
        segment_count(&engine, &curated),
        stored,
        "a retried association must store nothing further"
    );

    // The disassociation of that same segment, and its retry.
    let disassociate_uri = format!("/datasets/{curated}/data-segments/disassociate");
    let disassociate = json!({
        "workspaceName": WORKSPACE,
        "disassociateDataSegmentEntries": entries,
    });
    let token = token_for("batch-disassociate-data-segments");
    let (status, first) = call(
        &router,
        "POST",
        &disassociate_uri,
        Some(with_token(&disassociate, &token)),
    )
    .await;
    assert_eq!(status, 200, "the disassociation got {status} {first}");
    assert_eq!(
        segment_count(&engine, &curated),
        0,
        "the disassociation must take the segment out, got {first}"
    );
    let (replay_status, replay) = call(
        &router,
        "POST",
        &disassociate_uri,
        Some(with_token(&disassociate, &token)),
    )
    .await;
    assert_eq!(
        (replay_status, &replay),
        (200, &first),
        "a retried disassociation must replay its own per-entry answer, got {replay_status} \
         {replay}"
    );

    // The deletion batch, over the segments a SESSION dataset holds of its own.
    seed_segment(&engine, &session, &first_series);
    let delete_uri = format!("/datasets/{session}/data-segments/batch-delete");
    let delete = json!({
        "workspaceName": WORKSPACE,
        "deleteDataSegmentEntries": [
            {
                "timeSeriesId": first_series,
                "startTimestamp": wire_instant(0, 0),
                "endTimestamp": wire_instant(10, 0),
            },
            {
                "timeSeriesId": second_series,
                "startTimestamp": wire_instant(0, 0),
                "endTimestamp": wire_instant(10, 0),
            },
        ],
    });
    let token = token_for("batch-delete-dataset-data-segments");
    let (status, first) = call(
        &router,
        "POST",
        &delete_uri,
        Some(with_token(&delete, &token)),
    )
    .await;
    assert_eq!(status, 200, "the deletion got {status} {first}");
    assert_eq!(
        first["errors"].as_array().map(Vec::len),
        Some(1),
        "the deletion must report exactly the entry naming no stored segment, got {first}"
    );
    let (replay_status, replay) = call(
        &router,
        "POST",
        &delete_uri,
        Some(with_token(&delete, &token)),
    )
    .await;
    assert_eq!(
        (replay_status, &replay),
        (200, &first),
        "a retried deletion must replay its own per-entry answer, got {replay_status} {replay}"
    );
}

/// How many data segments `dataset_id` holds.
fn segment_count(engine: &Arc<engine::InMemoryEngine>, dataset_id: &str) -> usize {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let store = state.lock();
    store
        .dataset_data_segments
        .get(dataset_id)
        .map(Vec::len)
        .unwrap_or(0)
}

/// Put one data segment on `dataset_id` through the state's own producer entry
/// point: a SESSION dataset's segments are its own ingested data, which no
/// implemented operation of this service writes yet.
fn seed_segment(engine: &Arc<engine::InMemoryEngine>, dataset_id: &str, time_series_id: &str) {
    let state = service_state(&**engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    let stored = store.put_dataset_data_segment(
        dataset_id,
        DataSegmentRecord {
            source_dataset_id: dataset_id.to_string(),
            time_series_id: time_series_id.to_string(),
            start_timestamp: instant(0, 0),
            end_timestamp: instant(10, 0),
        },
    );
    assert!(stored, "seeding one segment of {dataset_id} must be accepted");
}
