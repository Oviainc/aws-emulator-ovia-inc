// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteDashboard`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a delete removes EXACTLY the dashboard the request names and leaves every
//!   other dashboard, the project the dashboard lived in and every other resource
//!   of the store alone;
//! * the deleted dashboard stops resolving everywhere it used to: its reader
//!   answers the modelled 404, the project's listing no longer reports it, and its
//!   ARN no longer carries tags;
//! * an id that resolves to no stored dashboard — never created, or already
//!   deleted — is the modelled 404 naming the identifier the REQUEST supplied, and
//!   writes nothing.
//!
//! The rows differ along what the rule generalises over: whether the dashboard
//! carried tags, whether it had a description, the size of its definition literal,
//! whether the delete carried a `clientToken`, how many sibling dashboards its own
//! project held and how many a second project held.
//!
//! Every request this suite sends goes through a helper a sibling suite already
//! has, rather than a second copy of it: the HTTP harness (`call`, `error_code`,
//! `is_modelled_id`, `router_with_engine`, `REGION`) is the asset suite's, the
//! project fixture is the project-assets suite's, the project reader is the
//! project-lifecycle suite's, the dashboard reader, the definition builder and the
//! unknown-id builder are the dashboard lifecycle suite's, the tag reader is the
//! resource-tags suite's and the two store readings are the project-delete
//! suite's.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine, REGION};
use super::dashboard_lifecycle::{definition_of, describe_dashboard, modelled_id};
use super::project_assets::create_project;
use super::project_delete::{stored_dashboard_ids, stored_project_ids};
use super::project_lifecycle::describe_project;
use super::resource_tags::list_tags;

/// Where every dashboard request goes, as `OPERATIONS.md` reports the routes:
/// `POST /dashboards`, `GET /dashboards?projectId=...`,
/// `GET|PUT|DELETE /dashboards/{dashboardId}`.
const DASHBOARDS: &str = "/dashboards";

/// One row of the delete table.
#[derive(Debug)]
struct DeleteCase {
    label: &'static str,
    /// The tags the deleted dashboard was created with, if any.
    tags: Option<Value>,
    /// The description the deleted dashboard was created with, if any.
    description: Option<&'static str>,
    /// How long the deleted dashboard's definition literal is.
    definition_length: usize,
    /// How many sibling dashboards the deleted dashboard's own project holds.
    siblings: usize,
    /// How many dashboards a second project holds.
    other_project_dashboards: usize,
    /// The `clientToken` the delete carries as a query parameter, if any. A token
    /// targets `ClientToken` (len 36..64, pattern `^\S{36,64}$`), per
    /// smithy_query query="operation" operation="DeleteDashboard".
    client_token: Option<&'static str>,
}

/// The rows.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "only-dashboard-of-its-project",
            tags: None,
            description: None,
            definition_length: 32,
            siblings: 0,
            other_project_dashboards: 0,
            client_token: None,
        },
        DeleteCase {
            label: "one-sibling-tags-and-a-description",
            tags: Some(json!({ "floor": "one", "line": "two" })),
            description: Some("the dashboard that goes"),
            definition_length: 256,
            siblings: 1,
            other_project_dashboards: 1,
            client_token: None,
        },
        DeleteCase {
            label: "several-siblings-and-a-client-token",
            tags: Some(json!({ "owner": "monitor" })),
            description: Some("d"),
            definition_length: 4_096,
            siblings: 3,
            other_project_dashboards: 2,
            client_token: Some("b9b5f0d1-0f2c-4d5e-9a7b-6c5d4e3f2a1b-token"),
        },
        DeleteCase {
            label: "no-siblings-but-another-project-holds-dashboards",
            tags: Some(json!({ "team": "controls" })),
            description: None,
            definition_length: 30,
            siblings: 0,
            other_project_dashboards: 3,
            client_token: Some("0123456789abcdef0123456789abcdef0123"),
        },
    ]
}

/// `CreateDashboard` in `project_id`, and the `(id, arn)` it minted.
async fn create_dashboard(
    router: &MultiServiceRouter,
    case: &str,
    project_id: &str,
    name: &str,
    description: Option<&str>,
    definition_length: usize,
    tags: Option<&Value>,
) -> (String, String) {
    let mut body = Map::new();
    body.insert("projectId".to_string(), Value::String(project_id.to_string()));
    body.insert("dashboardName".to_string(), Value::String(name.to_string()));
    body.insert(
        "dashboardDefinition".to_string(),
        Value::String(definition_of(definition_length)),
    );
    if let Some(description) = description {
        body.insert(
            "dashboardDescription".to_string(),
            Value::String(description.to_string()),
        );
    }
    if let Some(tags) = tags {
        body.insert("tags".to_string(), tags.clone());
    }

    let (status, created) = call(router, "POST", DASHBOARDS, Some(Value::Object(body))).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateDashboard expected 201, got {status} {created}"
    );
    let id = created["dashboardId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_modelled_id(&id),
        "case {case:?}: dashboardId must be a modelled UUID, got {created}"
    );
    (
        id,
        created["dashboardArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// Create `count` plain dashboards in `project_id` and return their ids.
async fn create_dashboards(
    router: &MultiServiceRouter,
    case: &str,
    project_id: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::new();
    for at in 0..count {
        let (id, _) = create_dashboard(
            router,
            case,
            project_id,
            &format!("sibling-dashboard-{at}"),
            None,
            64,
            None,
        )
        .await;
        ids.push(id);
    }
    ids
}

/// `DeleteDashboard` on `id`, carrying `client_token` as the query parameter the
/// model binds it to.
async fn delete_dashboard(
    router: &MultiServiceRouter,
    id: &str,
    client_token: Option<&str>,
) -> (u16, Value) {
    let uri = match client_token {
        Some(token) => format!("{DASHBOARDS}/{id}?clientToken={token}"),
        None => format!("{DASHBOARDS}/{id}"),
    };
    call(router, "DELETE", &uri, None).await
}

/// The ids `ListDashboards` reports for `project_id`, sorted.
async fn listed_dashboard_ids(router: &MultiServiceRouter, project_id: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("{DASHBOARDS}?projectId={project_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListDashboards expected 200, got {status} {body}"
    );
    let mut ids: Vec<String> = body["dashboardSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("dashboardSummaries must be a list, got {body}"))
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect();
    ids.sort();
    ids
}

/// The ARN form `CreateDashboard` documents, built here as an independent oracle:
/// `arn:${Partition}:iotsitewise:${Region}:${Account}:dashboard/${DashboardId}`.
fn expected_dashboard_arn(id: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:dashboard/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

#[tokio::test]
async fn a_delete_removes_the_named_dashboard_and_nothing_else() {
    for case in delete_cases() {
        let (router, engine) = router_with_engine();
        let project = create_project(&router, case.label, "the-owning-project").await;
        let other_project = create_project(&router, case.label, "the-other-project").await;

        let (doomed, arn) = create_dashboard(
            &router,
            case.label,
            &project,
            "the-dashboard-that-goes",
            case.description,
            case.definition_length,
            case.tags.as_ref(),
        )
        .await;
        let siblings = create_dashboards(&router, case.label, &project, case.siblings).await;
        let elsewhere =
            create_dashboards(&router, case.label, &other_project, case.other_project_dashboards)
                .await;

        // The dashboard reads back before the delete, so what the delete removed is
        // measured against what was really stored, and its ARN is the documented
        // form rather than whatever the create happened to echo.
        let (status, before) = describe_dashboard(&router, &doomed).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeDashboard before the delete expected 200, got {status} {before}",
            case.label
        );
        assert_eq!(
            arn,
            expected_dashboard_arn(&doomed),
            "case {:?}: unexpected dashboardArn in {before}",
            case.label
        );
        if let Some(expected_tags) = &case.tags {
            let (status, tags) = list_tags(&router, &arn).await;
            assert_eq!(
                (status, &tags["tags"]),
                (200, expected_tags),
                "case {:?}: the created dashboard's ARN must carry its tags, got {tags}",
                case.label
            );
        }
        let project_before = describe_project(&router, &project).await.1;
        let siblings_before: Vec<Value> = {
            let mut snapshots = Vec::new();
            for sibling in &siblings {
                snapshots.push(describe_dashboard(&router, sibling).await.1);
            }
            snapshots
        };

        let (status, answer) = delete_dashboard(&router, &doomed, case.client_token).await;
        assert_eq!(
            status, 204,
            "case {:?}: DeleteDashboard expected 204, got {status} {answer}",
            case.label
        );
        assert!(
            answer.is_null() || answer.as_object().is_some_and(Map::is_empty),
            "case {:?}: DeleteDashboard's modelled output carries no members, got {answer}",
            case.label
        );

        // The dashboard is gone from its reader, from its project's listing and from
        // the store.
        let (status, after) = describe_dashboard(&router, &doomed).await;
        assert_eq!(
            (status, error_code(&after)),
            (404, "ResourceNotFoundException"),
            "case {:?}: DescribeDashboard after the delete must be a modelled 404, \
             got {status} {after}",
            case.label
        );
        let mut expected_listed = siblings.clone();
        expected_listed.sort();
        assert_eq!(
            listed_dashboard_ids(&router, &project).await,
            expected_listed,
            "case {:?}: the project's listing must report exactly its surviving dashboards",
            case.label
        );
        let mut expected_stored = siblings.clone();
        expected_stored.extend(elsewhere.iter().cloned());
        expected_stored.sort();
        assert_eq!(
            stored_dashboard_ids(&engine),
            expected_stored,
            "case {:?}: the store must hold exactly the dashboards the delete did not name",
            case.label
        );

        // Its ARN stops resolving: the tags it carried went with the record.
        let (status, tags) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, error_code(&tags)),
            (404, "ResourceNotFoundException"),
            "case {:?}: the deleted dashboard's ARN must stop resolving, got {status} {tags}",
            case.label
        );

        // Every sibling and every dashboard of the other project stayed, exactly as
        // it was, and so did both projects.
        for (sibling, snapshot) in siblings.iter().zip(&siblings_before) {
            let (status, sibling_after) = describe_dashboard(&router, sibling).await;
            assert_eq!(
                status, 200,
                "case {:?}: sibling {sibling} must survive, got {status} {sibling_after}",
                case.label
            );
            assert_eq!(
                &sibling_after, snapshot,
                "case {:?}: the delete must leave sibling {sibling} exactly as it was, \
                 got {sibling_after}",
                case.label
            );
        }
        assert_eq!(
            listed_dashboard_ids(&router, &other_project).await,
            {
                let mut ids = elsewhere.clone();
                ids.sort();
                ids
            },
            "case {:?}: the other project's dashboards must be untouched",
            case.label
        );
        let (status, project_after) = describe_project(&router, &project).await;
        assert_eq!(
            status, 200,
            "case {:?}: the owning project expected 200, got {status} {project_after}",
            case.label
        );
        assert_eq!(
            project_after, project_before,
            "case {:?}: deleting a dashboard must leave its project exactly as it was, \
             got {project_after}",
            case.label
        );
        assert_eq!(
            stored_project_ids(&engine),
            {
                let mut ids = vec![project.clone(), other_project.clone()];
                ids.sort();
                ids
            },
            "case {:?}: the delete must remove no project",
            case.label
        );
    }
}

#[tokio::test]
async fn a_delete_of_a_dashboard_the_store_does_not_hold_is_a_modelled_not_found() {
    let (router, engine) = router_with_engine();
    let project = create_project(&router, "the-stored-dashboard", "the-project").await;
    let (stored, _) = create_dashboard(
        &router,
        "the-stored-dashboard",
        &project,
        "the-dashboard-that-stays",
        Some("as created"),
        128,
        Some(&json!({ "keep": "me" })),
    )
    .await;
    let before = describe_dashboard(&router, &stored).await.1;

    // A delete that lands first, so the SECOND delete of the same id is a row of
    // this table: a `clientToken` buys no replay of the answer that already went
    // out, exactly as the creates in this crate leave idempotency to its own issue.
    let (doomed, _) = create_dashboard(
        &router,
        "the-twice-deleted-dashboard",
        &project,
        "the-dashboard-deleted-twice",
        None,
        64,
        None,
    )
    .await;
    let (status, answer) = delete_dashboard(&router, &doomed, None).await;
    assert_eq!(
        status, 204,
        "the first delete expected 204, got {status} {answer}"
    );

    // Two ids no dashboard was ever created with — one of them the neighbour of the
    // all-zero id the modelled pattern refuses — and the id of the dashboard the
    // delete above already removed.
    let cases = [
        ("never-created", modelled_id(0x5e5e_5e5e)),
        (
            "never-created-neighbour-of-the-nil-id",
            "00000000-0000-0000-0000-000000000001".to_string(),
        ),
        ("already-deleted", doomed.clone()),
    ];

    for (case, id) in cases {
        assert!(
            is_modelled_id(&id),
            "case {case:?}: the probe id must itself be modelled"
        );
        assert_ne!(
            id, stored,
            "case {case:?}: the probe must not be the dashboard that exists"
        );
        for token in [None, Some("3f2a1b0c-9d8e-7f6a-5b4c-3d2e1f0a9b8c-tok")] {
            let (status, answer) = delete_dashboard(&router, &id, token).await;
            assert_eq!(
                (status, error_code(&answer)),
                (404, "ResourceNotFoundException"),
                "case {case:?} (token {token:?}): a delete of an absent dashboard must be a \
                 modelled 404, got {status} {answer}"
            );
            // The refusal names the identifier the REQUEST supplied, not the one
            // dashboard that does exist.
            let message = answer["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&id) && !message.contains(&stored),
                "case {case:?}: the refusal must name the requested id only, got {answer}"
            );
        }
    }

    // Every refusal wrote nothing: the stored dashboard is exactly as it was and it
    // is the only one left.
    let (status, after) = describe_dashboard(&router, &stored).await;
    assert_eq!(status, 200, "the stored dashboard: got {status} {after}");
    assert_eq!(
        after, before,
        "a refused delete must leave the stored dashboard exactly as it was, got {after}"
    );
    assert_eq!(
        stored_dashboard_ids(&engine),
        vec![stored.clone()],
        "the store must hold exactly the one dashboard that was never deleted"
    );
}
