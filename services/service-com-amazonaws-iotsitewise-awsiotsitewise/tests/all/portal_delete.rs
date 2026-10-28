// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeletePortal`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule this suite pins: deleting an IoT SiteWise Monitor portal removes it and
//! every Monitor resource the service holds only as a part of it — the projects
//! created in it and those projects' dashboards — so a later read of any of them
//! resolves to nothing, while every portal the call did not name, and everything
//! inside it, is left exactly as it was. An access policy is a resource of its own,
//! addressed by its own id and removed by `DeleteAccessPolicy`, so one that names
//! the deleted portal or one of its projects stays readable — the same line
//! `DeleteProject` draws for the policies naming the project it removes. A portal id
//! no portal is stored under is a modelled 404 that removes nothing.
//!
//! The HTTP harness (`call`, `router`, `error_code`) is the one the asset suite
//! already has; this suite reuses it rather than growing a second.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, ACTIVATION_DELAY};

use super::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine};

/// The Monitor routes, as `OPERATIONS.md` reports them: `POST /portals`,
/// `DELETE|GET /portals/{portalId}`, `POST /projects`, `GET /projects/{projectId}`,
/// `POST /dashboards`, `GET /dashboards/{dashboardId}`, `POST /access-policies`,
/// `GET /access-policies/{accessPolicyId}`.
const PORTALS: &str = "/portals";
const PROJECTS: &str = "/projects";
const DASHBOARDS: &str = "/dashboards";
const ACCESS_POLICIES: &str = "/access-policies";

/// The `PortalState` value `DeletePortal` documents for its answer: "a state
/// (DELETING after successfully calling this operation)". Source:
/// `smithy_query(query="shape", shape="PortalState")` — one of `CREATING`,
/// `PENDING`, `UPDATING`, `DELETING`, `ACTIVE`, `FAILED`.
const DELETING: &str = "DELETING";

/// Create a portal and return its id.
async fn create_portal(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        PORTALS,
        Some(json!({
            "portalName": name,
            "portalContactEmail": "admin@example.com",
            "roleArn": "arn:aws:iam::123456789012:role/service-role-monitor",
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {name:?}: CreatePortal expected 202, got {status} {body}"
    );
    body["portalId"].as_str().unwrap_or_default().to_string()
}

/// Create a project in `portal_id` and return its id.
async fn create_project(router: &MultiServiceRouter, portal_id: &str, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        PROJECTS,
        Some(json!({ "portalId": portal_id, "projectName": name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: CreateProject expected 201, got {status} {body}"
    );
    body["projectId"].as_str().unwrap_or_default().to_string()
}

/// Create a dashboard in `project_id` and return its id.
async fn create_dashboard(router: &MultiServiceRouter, project_id: &str, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        DASHBOARDS,
        Some(json!({
            "projectId": project_id,
            "dashboardName": name,
            "dashboardDefinition": "{\"widgets\":[]}",
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: CreateDashboard expected 201, got {status} {body}"
    );
    body["dashboardId"].as_str().unwrap_or_default().to_string()
}

/// Create an access policy about `resource` and return its id.
async fn create_access_policy(
    router: &MultiServiceRouter,
    label: &str,
    resource: Value,
    permission: &str,
) -> String {
    let (status, body) = call(
        router,
        "POST",
        ACCESS_POLICIES,
        Some(json!({
            "accessPolicyIdentity": {
                "iamRole": { "arn": "arn:aws:iam::123456789012:role/monitor-admin" },
            },
            "accessPolicyResource": resource,
            "accessPolicyPermission": permission,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {label:?}: CreateAccessPolicy expected 201, got {status} {body}"
    );
    body["accessPolicyId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One row of the delete table: what the portal being deleted holds, and how the
/// delete is issued.
#[derive(Debug)]
struct DeleteCase {
    label: &'static str,
    /// How many projects live in the portal.
    projects: usize,
    /// How many dashboards live in each of those projects.
    dashboards_per_project: usize,
    /// Whether an access policy is granted on the portal itself.
    portal_policy: bool,
    /// Whether an access policy is granted on each of the portal's projects.
    project_policies: bool,
    /// The `clientToken` the delete carries as a query parameter, if any.
    client_token: Option<&'static str>,
    /// Whether the test waits for the portal's `CREATING` -> `ACTIVE` transition
    /// before deleting it, so the table spans a portal still settling the create
    /// and one that has settled.
    settle_first: bool,
}

/// The rows. They differ along everything the rule generalises over: how many
/// projects the portal holds (none, one, several), how many dashboards hang off
/// each project (none, one, several), whether grants exist on the portal, on its
/// projects or on both, whether the delete carries an idempotency token, and
/// whether the portal has finished settling its create.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "empty-portal",
            projects: 0,
            dashboards_per_project: 0,
            portal_policy: false,
            project_policies: false,
            client_token: None,
            settle_first: false,
        },
        DeleteCase {
            label: "one-project-no-dashboards",
            projects: 1,
            dashboards_per_project: 0,
            portal_policy: false,
            project_policies: false,
            client_token: None,
            settle_first: false,
        },
        DeleteCase {
            label: "one-project-two-dashboards",
            projects: 1,
            dashboards_per_project: 2,
            portal_policy: false,
            project_policies: false,
            client_token: None,
            settle_first: false,
        },
        DeleteCase {
            label: "three-projects-one-dashboard-each",
            projects: 3,
            dashboards_per_project: 1,
            portal_policy: false,
            project_policies: false,
            client_token: None,
            settle_first: false,
        },
        DeleteCase {
            label: "grant-on-the-portal-only",
            projects: 0,
            dashboards_per_project: 0,
            portal_policy: true,
            project_policies: false,
            client_token: None,
            settle_first: false,
        },
        DeleteCase {
            label: "grants-on-two-projects",
            projects: 2,
            dashboards_per_project: 1,
            portal_policy: false,
            project_policies: true,
            client_token: None,
            settle_first: false,
        },
        DeleteCase {
            label: "settled-portal-with-everything-and-a-client-token",
            projects: 2,
            dashboards_per_project: 2,
            portal_policy: true,
            project_policies: true,
            // Within the modelled `ClientToken` bounds (len 36..64, `^\S{36,64}$`).
            client_token: Some("7a1c9f04-2b6d-4e8a-9c31-5d0e6f7a8b9c-del"),
            settle_first: true,
        },
    ]
}

/// Everything one portal holds, so the test can read each piece back.
#[derive(Debug)]
struct Monitor {
    portal: String,
    projects: Vec<String>,
    dashboards: Vec<String>,
    policies: Vec<String>,
}

/// Build the portal a row describes, under the name `label`-`tag`.
async fn build(router: &MultiServiceRouter, case: &DeleteCase, tag: &str) -> Monitor {
    let portal = create_portal(router, &format!("portal-{}-{tag}", case.label)).await;
    let mut projects = Vec::new();
    let mut dashboards = Vec::new();
    let mut policies = Vec::new();

    if case.portal_policy {
        policies.push(
            create_access_policy(
                router,
                case.label,
                json!({ "portal": { "id": portal } }),
                "ADMINISTRATOR",
            )
            .await,
        );
    }

    for at in 0..case.projects {
        let project =
            create_project(router, &portal, &format!("project-{at}-{}-{tag}", case.label)).await;
        for slot in 0..case.dashboards_per_project {
            dashboards.push(
                create_dashboard(
                    router,
                    &project,
                    &format!("dashboard-{at}-{slot}-{}-{tag}", case.label),
                )
                .await,
            );
        }
        if case.project_policies {
            policies.push(
                create_access_policy(
                    router,
                    case.label,
                    json!({ "project": { "id": project } }),
                    "VIEWER",
                )
                .await,
            );
        }
        projects.push(project);
    }

    Monitor {
        portal,
        projects,
        dashboards,
        policies,
    }
}

/// The `(path, kind)` pairs the portal and everything it CONTAINS are read back
/// through: the resources a delete of the portal takes with it.
fn contained_readers(monitor: &Monitor) -> Vec<(String, String)> {
    let mut paths = vec![(format!("{PORTALS}/{}", monitor.portal), "portal".to_string())];
    for project in &monitor.projects {
        paths.push((format!("{PROJECTS}/{project}"), "project".to_string()));
    }
    for dashboard in &monitor.dashboards {
        paths.push((format!("{DASHBOARDS}/{dashboard}"), "dashboard".to_string()));
    }
    paths
}

/// The `(path, kind)` pairs the grants ABOUT `monitor` are read back through: a
/// resource of its own, which the portal's delete does not remove.
fn policy_readers(monitor: &Monitor) -> Vec<(String, String)> {
    monitor
        .policies
        .iter()
        .map(|policy| {
            (
                format!("{ACCESS_POLICIES}/{policy}"),
                "access policy".to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn deleting_a_portal_removes_it_and_everything_it_holds() {
    for case in delete_cases() {
        let (router, engine) = router_with_engine();
        // The portal the delete names, and a second one built exactly like it that
        // the delete does NOT name: nothing inside the second may move.
        let target = build(&router, &case, "target").await;
        let bystander = build(&router, &case, "bystander").await;

        if case.settle_first {
            tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(50)).await;
        }

        let uri = match case.client_token {
            Some(token) => format!("{PORTALS}/{}?clientToken={token}", target.portal),
            None => format!("{PORTALS}/{}", target.portal),
        };
        let (status, deleted) = call(&router, "DELETE", &uri, None).await;
        assert_eq!(
            status, 202,
            "case {:?}: DeletePortal expected 202, got {status} {deleted}",
            case.label
        );
        assert_eq!(
            deleted["portalStatus"]["state"].as_str().unwrap_or(""),
            DELETING,
            "case {:?}: a deleted portal answers {DELETING}, got {deleted}",
            case.label
        );
        assert!(
            deleted["portalStatus"]
                .get("error")
                .is_none_or(Value::is_null),
            "case {:?}: a delete that failed nothing carries no error, got {deleted}",
            case.label
        );

        // Nothing the deleted portal held resolves any more, and each refusal names
        // the identifier the request supplied.
        for (path, kind) in contained_readers(&target) {
            let (status, body) = call(&router, "GET", &path, None).await;
            assert_eq!(
                (status, error_code(&body)),
                (404, "ResourceNotFoundException"),
                "case {:?}: the {kind} at {path} must be gone, got {status} {body}",
                case.label
            );
            let requested = path.rsplit('/').next().unwrap_or_default();
            assert!(
                body["message"].as_str().unwrap_or("").contains(requested),
                "case {:?}: the refusal for {path} must name {requested}, got {body}",
                case.label
            );
        }

        // The portal the delete did not name, and everything inside it, is readable
        // exactly as before — and so is every access policy, the deleted portal's
        // included: a grant is a resource of its own, removed only by its own delete.
        for (path, kind) in contained_readers(&bystander)
            .into_iter()
            .chain(policy_readers(&bystander))
            .chain(policy_readers(&target))
        {
            let (status, body) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case {:?}: the untouched {kind} at {path} expected 200, got {status} {body}",
                case.label
            );
        }

        // A second delete of the same portal has nothing left to remove: the id
        // resolves to no portal, so a request carrying no token — or a token of
        // its own — is the modelled 404.
        let (status, again) = call(
            &router,
            "DELETE",
            &format!("{PORTALS}/{}", target.portal),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&again)),
            (404, "ResourceNotFoundException"),
            "case {:?}: a repeated DeletePortal expected 404, got {status} {again}",
            case.label
        );

        // A repeat carrying the `clientToken` the delete above spent is the RETRY
        // of that call, so it replays its answer instead of that refusal — and
        // removes nothing further.
        if case.client_token.is_some() {
            let (status, replayed) = call(&router, "DELETE", &uri, None).await;
            assert_eq!(
                status, 202,
                "case {:?}: a retried DeletePortal expected 202, got {status} {replayed}",
                case.label
            );
            assert_eq!(
                replayed["portalStatus"]["state"].as_str().unwrap_or(""),
                DELETING,
                "case {:?}: a retried DeletePortal replays the first answer, got {replayed}",
                case.label
            );
        }

        // The store itself holds only the bystander's resources: asserted directly
        // because this service's listing operations are still scaffolds, so the wire
        // exposes no enumeration to compare against.
        let state = service_state(&*engine).expect("the iotsitewise store");
        let store = state.lock();
        assert_eq!(
            store.portals.keys().cloned().collect::<Vec<String>>(),
            vec![bystander.portal.clone()],
            "case {:?}: only the untouched portal may remain",
            case.label
        );
        let mut remaining_projects: Vec<String> = store.projects.keys().cloned().collect();
        remaining_projects.sort();
        let mut expected_projects = bystander.projects.clone();
        expected_projects.sort();
        assert_eq!(
            remaining_projects, expected_projects,
            "case {:?}: only the untouched portal's projects may remain",
            case.label
        );
        let mut remaining_dashboards: Vec<String> = store.dashboards.keys().cloned().collect();
        remaining_dashboards.sort();
        let mut expected_dashboards = bystander.dashboards.clone();
        expected_dashboards.sort();
        assert_eq!(
            remaining_dashboards, expected_dashboards,
            "case {:?}: only the untouched portal's dashboards may remain",
            case.label
        );
        let mut remaining_policies: Vec<String> = store.access_policies.keys().cloned().collect();
        remaining_policies.sort();
        let mut expected_policies = bystander.policies.clone();
        expected_policies.extend(target.policies.iter().cloned());
        expected_policies.sort();
        assert_eq!(
            remaining_policies, expected_policies,
            "case {:?}: every grant must survive the portal's delete",
            case.label
        );
    }
}

#[tokio::test]
async fn deleting_a_portal_that_was_never_created_removes_nothing() {
    let (router, engine) = router_with_engine();
    let case = DeleteCase {
        label: "untouched-by-a-refused-delete",
        projects: 1,
        dashboards_per_project: 1,
        portal_policy: true,
        project_policies: true,
        client_token: None,
        settle_first: false,
    };
    let stored = build(&router, &case, "only").await;

    // Well-formed ids no create ever minted: one all-f, one that differs from the
    // stored id in a single nibble.
    let mut neighbour: Vec<char> = stored.portal.chars().collect();
    let at = neighbour
        .iter()
        .position(|ch| *ch != '-' && *ch != '0')
        .expect("a non-zero character in the id");
    neighbour[at] = '0';
    let neighbour: String = neighbour.into_iter().collect();
    // The probe is a DIFFERENT id from the stored one, and is itself a modelled
    // `ID` rather than the all-zero UUID the pattern refuses: both are what make
    // the 404 below an answer about a portal nothing created.
    assert_ne!(
        neighbour, stored.portal,
        "the neighbour probe must differ from the stored portal id"
    );
    assert!(
        is_modelled_id(&neighbour),
        "the neighbour probe must itself be a modelled id, got {neighbour:?}"
    );

    for probe in ["ffffffff-ffff-ffff-ffff-ffffffffffff", neighbour.as_str()] {
        let (status, body) = call(&router, "DELETE", &format!("{PORTALS}/{probe}"), None).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {probe:?}: expected a modelled 404, got {status} {body}"
        );
        // The refusal names the identifier the REQUEST supplied, not the one portal
        // that does exist.
        let message = body["message"].as_str().unwrap_or("");
        assert!(
            message.contains(probe) && !message.contains(&stored.portal),
            "case {probe:?}: the refusal must name the requested id only, got {body}"
        );
    }

    // Every piece of the stored portal survived the refusals.
    for (path, kind) in contained_readers(&stored)
        .into_iter()
        .chain(policy_readers(&stored))
    {
        let (status, body) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: {path} must survive a refused delete, got {status} {body}"
        );
    }
    let state = service_state(&*engine).expect("the iotsitewise store");
    let store = state.lock();
    assert_eq!(
        (
            store.portals.len(),
            store.projects.len(),
            store.dashboards.len(),
            store.access_policies.len()
        ),
        (1, 1, 1, 2),
        "a refused delete must remove nothing from the store"
    );
}
