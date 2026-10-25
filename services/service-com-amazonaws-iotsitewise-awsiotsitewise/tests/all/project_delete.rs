// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteProject`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rules these suites pin:
//!
//! * a delete removes EXACTLY the project the request names, with the dashboards
//!   that project contained, and leaves every other project, every other project's
//!   dashboards and every other resource of the store alone;
//! * the deleted project stops resolving everywhere it used to: its reader answers
//!   the modelled 404 and its ARN no longer carries tags;
//! * an id that resolves to no stored project — never created, or already deleted —
//!   is the modelled 404 naming the identifier the REQUEST supplied, and writes
//!   nothing.
//!
//! The rows differ along what the rule generalises over: how many dashboards the
//! project held (none, one, several), whether it carried tags, whether it had a
//! description, whether the delete carried a `clientToken`, and whether the portal
//! the project lived in also held another project.
//!
//! The HTTP harness (`call`, `error_code`, `is_modelled_id`, `router_with_engine`,
//! `REGION`) is the one the asset suite already has and the portal helper is the
//! one the access-policy suite already has; this suite reuses both rather than
//! growing a second copy.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use super::access_policy_lifecycle::create_portal;
use super::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine, REGION};

/// Where every project request goes, as `OPERATIONS.md` reports the routes:
/// `POST /projects`, `GET /projects/{projectId}`,
/// `DELETE /projects/{projectId}`.
const PROJECTS: &str = "/projects";

/// Where every dashboard request goes: `POST /dashboards`,
/// `GET /dashboards/{dashboardId}`.
const DASHBOARDS: &str = "/dashboards";

/// One row of the delete table.
#[derive(Debug)]
struct DeleteCase {
    label: &'static str,
    /// How many dashboards the deleted project holds.
    dashboards: usize,
    /// How many dashboards the surviving neighbour project holds.
    neighbour_dashboards: usize,
    /// The tags the deleted project was created with, if any.
    tags: Option<Value>,
    /// The description the deleted project was created with, if any.
    description: Option<&'static str>,
    /// The `clientToken` the delete carries as a query parameter, if any. A token
    /// targets `ClientToken` (len 36..64, pattern `^\S{36,64}$`), per
    /// smithy_query query="operation" operation="DeleteProject".
    client_token: Option<&'static str>,
    /// Whether the neighbour project lives in the same portal as the deleted one.
    same_portal: bool,
}

/// The rows.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "empty-project-sharing-its-portal",
            dashboards: 0,
            neighbour_dashboards: 1,
            tags: None,
            description: None,
            client_token: None,
            same_portal: true,
        },
        DeleteCase {
            label: "one-dashboard-and-tags",
            dashboards: 1,
            neighbour_dashboards: 1,
            tags: Some(json!({ "floor": "one", "line": "two" })),
            description: Some("the project a single dashboard lives in"),
            client_token: None,
            same_portal: true,
        },
        DeleteCase {
            label: "several-dashboards-and-a-client-token",
            dashboards: 3,
            neighbour_dashboards: 2,
            tags: Some(json!({ "owner": "monitor" })),
            description: Some("the project three dashboards live in"),
            client_token: Some("b9b5f0d1-0f2c-4d5e-9a7b-6c5d4e3f2a1b-token"),
            same_portal: false,
        },
        DeleteCase {
            label: "neighbour-in-another-portal-and-no-dashboards-anywhere",
            dashboards: 0,
            neighbour_dashboards: 0,
            tags: None,
            description: Some("d"),
            client_token: Some("0123456789abcdef0123456789abcdef0123"),
            same_portal: false,
        },
    ]
}

/// The create body a project row sends.
fn create_body(portal_id: &str, name: &str, description: Option<&str>, tags: Option<&Value>) -> Value {
    let mut body = Map::new();
    body.insert("portalId".to_string(), Value::String(portal_id.to_string()));
    body.insert("projectName".to_string(), Value::String(name.to_string()));
    if let Some(description) = description {
        body.insert(
            "projectDescription".to_string(),
            Value::String(description.to_string()),
        );
    }
    if let Some(tags) = tags {
        body.insert("tags".to_string(), tags.clone());
    }
    Value::Object(body)
}

/// Create one project and return its id.
async fn create_project(
    router: &MultiServiceRouter,
    case: &str,
    portal_id: &str,
    name: &str,
    description: Option<&str>,
    tags: Option<&Value>,
) -> String {
    let (status, created) = call(
        router,
        "POST",
        PROJECTS,
        Some(create_body(portal_id, name, description, tags)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateProject expected 201, got {status} {created}"
    );
    let id = created["projectId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_modelled_id(&id),
        "case {case:?}: projectId must be a modelled UUID, got {created}"
    );
    id
}

/// Create `count` dashboards in `project_id` and return their ids.
async fn create_dashboards(
    router: &MultiServiceRouter,
    case: &str,
    project_id: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::new();
    for at in 0..count {
        let (status, created) = call(
            router,
            "POST",
            DASHBOARDS,
            Some(json!({
                "projectId": project_id,
                "dashboardName": format!("dashboard-{at}"),
                "dashboardDefinition": "{\"widgets\":[]}",
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateDashboard expected 201, got {status} {created}"
        );
        ids.push(
            created["dashboardId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    ids
}

/// `DescribeProject` on `id`.
async fn describe_project(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{PROJECTS}/{id}"), None).await
}

/// `DescribeDashboard` on `id`.
async fn describe_dashboard(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{DASHBOARDS}/{id}"), None).await
}

/// `DeleteProject` on `id`, carrying `client_token` as the query parameter the
/// model binds it to.
async fn delete_project(
    router: &MultiServiceRouter,
    id: &str,
    client_token: Option<&str>,
) -> (u16, Value) {
    let uri = match client_token {
        Some(token) => format!("{PROJECTS}/{id}?clientToken={token}"),
        None => format!("{PROJECTS}/{id}"),
    };
    call(router, "DELETE", &uri, None).await
}

/// `ListTagsForResource` on `arn` — the reader that proves the deleted project's
/// ARN stops resolving.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags?resourceArn={arn}"), None).await
}

/// The ARN form `CreateProject` documents, built here as an independent oracle.
fn expected_project_arn(id: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:project/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The ids of every project the store holds.
fn stored_project_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the iotsitewise store");
    let mut ids: Vec<String> = state.lock().projects.keys().cloned().collect();
    ids.sort();
    ids
}

/// The ids of every dashboard the store holds.
fn stored_dashboard_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the iotsitewise store");
    let mut ids: Vec<String> = state.lock().dashboards.keys().cloned().collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn a_delete_removes_the_named_project_with_its_dashboards_and_nothing_else() {
    for case in delete_cases() {
        let (router, engine) = router_with_engine();
        let portal_id = create_portal(&router, "the-delete-portal").await;
        let neighbour_portal = if case.same_portal {
            portal_id.clone()
        } else {
            create_portal(&router, "the-neighbour-portal").await
        };

        let doomed = create_project(
            &router,
            case.label,
            &portal_id,
            "the-project-that-goes",
            case.description,
            case.tags.as_ref(),
        )
        .await;
        let neighbour = create_project(
            &router,
            case.label,
            &neighbour_portal,
            "the-project-that-stays",
            Some("the neighbour's description"),
            Some(&json!({ "keep": "me" })),
        )
        .await;

        let doomed_dashboards =
            create_dashboards(&router, case.label, &doomed, case.dashboards).await;
        let surviving_dashboards =
            create_dashboards(&router, case.label, &neighbour, case.neighbour_dashboards).await;

        // The project reads back before the delete, so what the delete removed is
        // measured against what was really stored.
        let (status, before) = describe_project(&router, &doomed).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeProject before the delete expected 200, got {status} {before}",
            case.label
        );
        let arn = before["projectArn"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            arn,
            expected_project_arn(&doomed),
            "case {:?}: unexpected projectArn in {before}",
            case.label
        );
        let neighbour_before = describe_project(&router, &neighbour).await.1;

        let (status, answer) = delete_project(&router, &doomed, case.client_token).await;
        assert_eq!(
            status, 204,
            "case {:?}: DeleteProject expected 204, got {status} {answer}",
            case.label
        );
        assert!(
            answer.is_null() || answer.as_object().is_some_and(Map::is_empty),
            "case {:?}: DeleteProject's modelled output carries no members, got {answer}",
            case.label
        );

        // The project is gone from the store and from its reader.
        let (status, after) = describe_project(&router, &doomed).await;
        assert_eq!(
            (status, error_code(&after)),
            (404, "ResourceNotFoundException"),
            "case {:?}: DescribeProject after the delete must be a modelled 404, \
             got {status} {after}",
            case.label
        );
        assert_eq!(
            stored_project_ids(&engine),
            vec![neighbour.clone()],
            "case {:?}: the store must hold only the neighbour project",
            case.label
        );

        // Its ARN stops resolving: the tags it carried went with the record.
        let (status, tags) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, error_code(&tags)),
            (404, "ResourceNotFoundException"),
            "case {:?}: the deleted project's ARN must stop resolving, got {status} {tags}",
            case.label
        );

        // Every dashboard the project held went with it; every other dashboard
        // stayed, and reads back exactly as it was.
        for dashboard in &doomed_dashboards {
            let (status, answer) = describe_dashboard(&router, dashboard).await;
            assert_eq!(
                (status, error_code(&answer)),
                (404, "ResourceNotFoundException"),
                "case {:?}: dashboard {dashboard} of the deleted project must be gone, \
                 got {status} {answer}",
                case.label
            );
        }
        for dashboard in &surviving_dashboards {
            let (status, answer) = describe_dashboard(&router, dashboard).await;
            assert_eq!(
                status, 200,
                "case {:?}: dashboard {dashboard} of another project must survive, \
                 got {status} {answer}",
                case.label
            );
            assert_eq!(
                answer["projectId"].as_str().unwrap_or_default(),
                neighbour,
                "case {:?}: a surviving dashboard still names its own project, got {answer}",
                case.label
            );
        }
        let mut expected_dashboards = surviving_dashboards.clone();
        expected_dashboards.sort();
        assert_eq!(
            stored_dashboard_ids(&engine),
            expected_dashboards,
            "case {:?}: the store must hold exactly the other project's dashboards",
            case.label
        );

        // The neighbour project itself is untouched.
        let (status, neighbour_after) = describe_project(&router, &neighbour).await;
        assert_eq!(
            status, 200,
            "case {:?}: the neighbour project expected 200, got {status} {neighbour_after}",
            case.label
        );
        assert_eq!(
            neighbour_after, neighbour_before,
            "case {:?}: the delete must leave the neighbour project exactly as it was, \
             got {neighbour_after}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_delete_of_a_project_the_store_does_not_hold_is_a_modelled_not_found() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "the-only-portal").await;
    let stored = create_project(
        &router,
        "the-stored-project",
        &portal_id,
        "the-only-project",
        Some("as created"),
        None,
    )
    .await;
    let dashboards = create_dashboards(&router, "the-stored-project", &stored, 2).await;
    let before = describe_project(&router, &stored).await.1;

    // A delete that lands first, so the SECOND delete of the same id is a row of
    // this table: a `clientToken` buys no replay of the answer that already went
    // out, exactly as the creates in this crate leave idempotency to its own issue.
    let doomed = create_project(
        &router,
        "the-twice-deleted-project",
        &portal_id,
        "the-twice-deleted-project",
        None,
        None,
    )
    .await;
    let (status, answer) = delete_project(&router, &doomed, None).await;
    assert_eq!(
        status, 204,
        "the first delete expected 204, got {status} {answer}"
    );

    // Two ids no project was ever created with, and the id of the project the
    // delete above already removed.
    let cases = [
        ("never-created", "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string()),
        ("never-created-neighbour-of-the-nil-id", "00000000-0000-0000-0000-000000000001".to_string()),
        ("already-deleted", doomed.clone()),
    ];

    for (case, id) in cases {
        assert!(
            is_modelled_id(&id),
            "case {case:?}: the probe id must itself be modelled"
        );
        assert_ne!(
            id, stored,
            "case {case:?}: the probe must not be the project that exists"
        );
        for token in [None, Some("3f2a1b0c-9d8e-7f6a-5b4c-3d2e1f0a9b8c-tok")] {
            let (status, answer) = delete_project(&router, &id, token).await;
            assert_eq!(
                (status, error_code(&answer)),
                (404, "ResourceNotFoundException"),
                "case {case:?} (token {token:?}): a delete of an absent project must be a \
                 modelled 404, got {status} {answer}"
            );
            // The refusal names the identifier the REQUEST supplied, not the one
            // project that does exist.
            let message = answer["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&id) && !message.contains(&stored),
                "case {case:?}: the refusal must name the requested id only, got {answer}"
            );
        }
    }

    // Every refusal wrote nothing: the stored project and its dashboards are
    // exactly as they were.
    let (status, after) = describe_project(&router, &stored).await;
    assert_eq!(status, 200, "the stored project: got {status} {after}");
    assert_eq!(
        after, before,
        "a refused delete must leave the stored project exactly as it was, got {after}"
    );
    let mut expected = dashboards.clone();
    expected.sort();
    assert_eq!(
        stored_dashboard_ids(&engine),
        expected,
        "a refused delete must leave every dashboard in place"
    );
    assert_eq!(
        stored_project_ids(&engine),
        vec![stored.clone()],
        "the store must hold exactly the one project that was never deleted"
    );
}
