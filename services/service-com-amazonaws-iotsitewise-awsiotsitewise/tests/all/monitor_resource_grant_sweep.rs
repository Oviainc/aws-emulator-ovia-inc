// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what a delete of an IoT SiteWise Monitor resource does to
//! the access policies that NAME it, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: removing a resource removes every record this
//! service holds only as a reference to it, so no read reports a reference to a
//! resource the service no longer holds. An access policy is written and rewritten
//! only against a Monitor resource this store holds — `CreateAccessPolicy` and
//! `UpdateAccessPolicy` both refuse a `Resource` (`portal` or `project`, per
//! `smithy_query(query="shape", shape="Resource")`) that resolves to nothing — so a
//! grant naming a deleted portal or project is a record no write could have
//! produced and no caller could repair. Deleting the portal therefore takes the
//! grants on it and on its projects with it, deleting a project takes the grants on
//! that project, and what survives is exactly what still names a live resource: the
//! suites check that by rewriting every surviving grant through
//! `UpdateAccessPolicy`, which no stored grant may be refused.
//!
//! A refused delete sweeps nothing: it removed no resource, so no grant lost the
//! resource it names.
//!
//! The HTTP harness (`call`, `error_code`, `router_with_engine`) is the one the
//! asset suite already has, and the portal/project creates and the identity and
//! permission catalogues are the access-policy suite's; this suite reuses both
//! rather than growing a second copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::access_policy_lifecycle::{
    create_portal, create_project, identity_catalog, PERMISSIONS,
};
use crate::asset_lifecycle::{call, error_code, router_with_engine};

/// The Monitor routes, as `OPERATIONS.md` reports them: `POST /access-policies`,
/// `GET|PUT /access-policies/{accessPolicyId}`, `GET /access-policies`,
/// `DELETE /portals/{portalId}`, `DELETE /projects/{projectId}`.
const ACCESS_POLICIES: &str = "/access-policies";
const PORTALS: &str = "/portals";
const PROJECTS: &str = "/projects";

/// The `ResourceType` values `ListAccessPolicies` filters by. Source:
/// `smithy_query(query="shape", shape="ResourceType")` — `PORTAL | PROJECT`.
const RESOURCE_TYPE_PORTAL: &str = "PORTAL";
const RESOURCE_TYPE_PROJECT: &str = "PROJECT";

/// Which delete the row issues: the one that removes the portal, or the one that
/// removes a single project of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sweep {
    Portal,
    Project,
}

/// One row: the Monitor resources the fixture builds, how many grants sit on each
/// of them, and how the delete is issued.
#[derive(Debug)]
struct SweepCase {
    label: &'static str,
    sweep: Sweep,
    /// How many projects the portal holds.
    projects: usize,
    /// How many grants are made on the portal itself.
    portal_grants: usize,
    /// How many grants are made on EACH of those projects.
    grants_per_project: usize,
    /// Whether the grants carry tags, so the sweep is observed through the policy
    /// ARN's tag reader too.
    tags: bool,
    /// The `clientToken` the delete carries as a query parameter, if any. Within
    /// the modelled `ClientToken` bounds (len 36..64, `^\S{36,64}$`).
    client_token: Option<&'static str>,
}

/// The rows. They differ along everything the rule generalises over: which delete
/// removes the resource (the portal's or a project's), which alternative of
/// `Resource` the grants take (the portal, its projects, or both), how many grants
/// name the removed resource (none, one, several), whether the grants carry tags,
/// and whether the delete carries an idempotency token.
fn sweep_cases() -> Vec<SweepCase> {
    vec![
        SweepCase {
            label: "portal-delete-one-grant-on-the-portal",
            sweep: Sweep::Portal,
            projects: 0,
            portal_grants: 1,
            grants_per_project: 0,
            tags: false,
            client_token: None,
        },
        SweepCase {
            label: "portal-delete-one-grant-on-each-of-two-projects",
            sweep: Sweep::Portal,
            projects: 2,
            portal_grants: 0,
            grants_per_project: 1,
            tags: true,
            client_token: None,
        },
        SweepCase {
            label: "portal-delete-grants-on-the-portal-and-on-every-project",
            sweep: Sweep::Portal,
            projects: 2,
            portal_grants: 2,
            grants_per_project: 2,
            tags: true,
            client_token: Some("4e3d2c1b-0a9f-8e7d-6c5b-4a39281706f5-pd"),
        },
        SweepCase {
            label: "portal-delete-with-no-grant-anywhere",
            sweep: Sweep::Portal,
            projects: 1,
            portal_grants: 0,
            grants_per_project: 0,
            tags: false,
            client_token: None,
        },
        SweepCase {
            label: "project-delete-one-grant-on-the-project-one-on-the-portal",
            sweep: Sweep::Project,
            projects: 2,
            portal_grants: 1,
            grants_per_project: 1,
            tags: false,
            client_token: None,
        },
        SweepCase {
            label: "project-delete-three-grants-on-the-project",
            sweep: Sweep::Project,
            projects: 1,
            portal_grants: 0,
            grants_per_project: 3,
            tags: true,
            client_token: Some("9f8e7d6c-5b4a-3928-1706-f5e4d3c2b1a0-prd"),
        },
        SweepCase {
            label: "project-delete-with-no-grant-anywhere",
            sweep: Sweep::Project,
            projects: 2,
            portal_grants: 0,
            grants_per_project: 0,
            tags: false,
            client_token: None,
        },
    ]
}

/// One grant the fixture made, with everything a rewrite of it would send.
#[derive(Debug, Clone)]
struct Grant {
    label: String,
    id: String,
    arn: String,
    identity: Value,
    resource: Value,
    permission: &'static str,
}

/// Everything one portal holds, so each piece can be read back after the delete.
#[derive(Debug)]
struct Monitor {
    portal: String,
    projects: Vec<String>,
    /// The grants on the portal itself.
    portal_grants: Vec<Grant>,
    /// The grants on each project, index-aligned with `projects`.
    project_grants: Vec<Vec<Grant>>,
}

impl Monitor {
    /// Every grant the fixture made on this portal or on one of its projects.
    fn all_grants(&self) -> Vec<Grant> {
        let mut grants = self.portal_grants.clone();
        for project in &self.project_grants {
            grants.extend(project.iter().cloned());
        }
        grants
    }
}

/// `CreateAccessPolicy` for one grant: the identity form and the permission rotate
/// through the catalogues, so a row with several grants spans several of each.
async fn create_grant(
    router: &MultiServiceRouter,
    label: &str,
    resource: Value,
    nth: usize,
    tags: bool,
) -> Grant {
    let catalog = identity_catalog();
    let (identity_label, identity) = catalog[nth % catalog.len()].clone();
    let permission = PERMISSIONS[nth % PERMISSIONS.len()];
    let label = format!("{label}/{identity_label}-{permission}-{nth}");

    let mut body = json!({
        "accessPolicyIdentity": identity,
        "accessPolicyResource": resource,
        "accessPolicyPermission": permission,
    });
    if tags {
        body["tags"] = json!({ "slot": format!("{nth}") });
    }

    let (status, created) = call(router, "POST", ACCESS_POLICIES, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {label:?}: CreateAccessPolicy expected 201, got {status} {created}"
    );
    Grant {
        label,
        id: created["accessPolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        arn: created["accessPolicyArn"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        identity,
        resource,
        permission,
    }
}

/// Build the portal, the projects and the grants a row describes, under names
/// carrying `tag` so the target and the bystander never collide.
async fn build(router: &MultiServiceRouter, case: &SweepCase, tag: &str) -> Monitor {
    let portal = create_portal(router, &format!("portal-{}-{tag}", case.label)).await;
    let mut nth = 0;
    let mut portal_grants = Vec::new();
    for _ in 0..case.portal_grants {
        portal_grants.push(
            create_grant(
                router,
                &format!("{}-{tag}-portal", case.label),
                json!({ "portal": { "id": portal } }),
                nth,
                case.tags,
            )
            .await,
        );
        nth += 1;
    }

    let mut projects = Vec::new();
    let mut project_grants = Vec::new();
    for at in 0..case.projects {
        let project =
            create_project(router, &portal, &format!("project-{at}-{}-{tag}", case.label)).await;
        let mut grants = Vec::new();
        for _ in 0..case.grants_per_project {
            grants.push(
                create_grant(
                    router,
                    &format!("{}-{tag}-project-{at}", case.label),
                    json!({ "project": { "id": project } }),
                    nth,
                    case.tags,
                )
                .await,
            );
            nth += 1;
        }
        projects.push(project);
        project_grants.push(grants);
    }

    Monitor {
        portal,
        projects,
        portal_grants,
        project_grants,
    }
}

/// The `(resourceType, resourceId, reader path)` triple a Monitor resource is
/// addressed by: the pair `ListAccessPolicies` filters on, and the path its own
/// describe answers.
fn resource_address(resource: &Value) -> (&'static str, String, String) {
    if let Some(id) = resource["portal"]["id"].as_str() {
        return (
            RESOURCE_TYPE_PORTAL,
            id.to_string(),
            format!("{PORTALS}/{id}"),
        );
    }
    let id = resource["project"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a Resource names a portal or a project, got {resource}"));
    (
        RESOURCE_TYPE_PROJECT,
        id.to_string(),
        format!("{PROJECTS}/{id}"),
    )
}

/// `DescribeAccessPolicy` on `id`.
async fn describe_grant(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{ACCESS_POLICIES}/{id}"), None).await
}

/// The ids `ListAccessPolicies` reports for `filter`, in one full page.
async fn listed_ids(router: &MultiServiceRouter, label: &str, filter: &[String]) -> Vec<String> {
    let mut query = filter.to_vec();
    // The published `maxResults` ceiling, so one call reports the whole store.
    query.push("maxResults=250".to_string());
    let uri = format!("{ACCESS_POLICIES}?{}", query.join("&"));
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {label:?}: ListAccessPolicies expected 200, got {status} {body}"
    );
    let mut ids: Vec<String> = body["accessPolicySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: accessPolicySummaries in {body}"))
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect();
    ids.sort();
    ids
}

/// The ids of every access policy the store holds.
fn stored_grant_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the iotsitewise store");
    let mut ids: Vec<String> = state.lock().access_policies.keys().cloned().collect();
    ids.sort();
    ids
}

/// `ListTagsForResource` on `arn` — the reader a swept grant's ARN stops
/// resolving through.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags?resourceArn={arn}"), None).await
}

#[tokio::test]
async fn deleting_a_monitor_resource_removes_every_grant_that_names_it() {
    for case in sweep_cases() {
        let (router, engine) = router_with_engine();
        // The portal the delete reaches into, and a second one built exactly like
        // it that the delete does not name: nothing of the second may move.
        let target = build(&router, &case, "target").await;
        let bystander = build(&router, &case, "bystander").await;

        // What the delete addresses, what it must sweep, and what must survive it.
        let (delete_uri, expected_status, swept, mut survivors) = match case.sweep {
            Sweep::Portal => (
                format!("{PORTALS}/{}", target.portal),
                202,
                target.all_grants(),
                bystander.all_grants(),
            ),
            Sweep::Project => {
                let doomed = target
                    .projects
                    .first()
                    .expect("a project-delete row builds at least one project")
                    .clone();
                let mut survivors = bystander.all_grants();
                survivors.extend(target.portal_grants.iter().cloned());
                for grants in target.project_grants.iter().skip(1) {
                    survivors.extend(grants.iter().cloned());
                }
                (
                    format!("{PROJECTS}/{doomed}"),
                    204,
                    target
                        .project_grants
                        .first()
                        .cloned()
                        .unwrap_or_default(),
                    survivors,
                )
            }
        };
        survivors.sort_by(|left, right| left.id.cmp(&right.id));

        // Each survivor as it reads before the delete, so "untouched" is measured
        // against what was really stored.
        let mut before = Vec::new();
        for grant in &survivors {
            let (status, body) = describe_grant(&router, &grant.id).await;
            assert_eq!(
                status, 200,
                "case {:?}: {} must read back before the delete, got {status} {body}",
                case.label, grant.label
            );
            before.push(body);
        }

        let uri = match case.client_token {
            Some(token) => format!("{delete_uri}?clientToken={token}"),
            None => delete_uri.clone(),
        };
        let (status, answer) = call(&router, "DELETE", &uri, None).await;
        assert_eq!(
            status, expected_status,
            "case {:?}: the delete of {delete_uri} expected {expected_status}, \
             got {status} {answer}",
            case.label
        );

        // Every grant that named the removed resource is gone: its own reader
        // answers the modelled 404 naming the id the request supplied, and its ARN
        // stops carrying tags.
        for grant in &swept {
            let (status, body) = describe_grant(&router, &grant.id).await;
            assert_eq!(
                (status, error_code(&body)),
                (404, "ResourceNotFoundException"),
                "case {:?}: {} named the removed resource and must be gone, \
                 got {status} {body}",
                case.label,
                grant.label
            );
            assert!(
                body["message"].as_str().unwrap_or("").contains(&grant.id),
                "case {:?}: the refusal for {} must name {}, got {body}",
                case.label,
                grant.label,
                grant.id
            );
            let (status, tags) = list_tags(&router, &grant.arn).await;
            assert_eq!(
                (status, error_code(&tags)),
                (404, "ResourceNotFoundException"),
                "case {:?}: the ARN of {} must stop resolving, got {status} {tags}",
                case.label,
                grant.label
            );
        }

        // No listing reports one either — neither the whole enumeration nor the one
        // narrowed to the resource the swept grants were about.
        let mut expected_ids: Vec<String> = survivors.iter().map(|g| g.id.clone()).collect();
        expected_ids.sort();
        assert_eq!(
            listed_ids(&router, case.label, &[]).await,
            expected_ids,
            "case {:?}: the listing must report exactly the surviving grants",
            case.label
        );
        for grant in &swept {
            let (kind, id, reader) = resource_address(&grant.resource);
            let filter = vec![format!("resourceType={kind}"), format!("resourceId={id}")];
            assert_eq!(
                listed_ids(&router, case.label, &filter).await,
                Vec::<String>::new(),
                "case {:?}: no grant may be listed for the removed {kind} {id}",
                case.label
            );
            let (status, body) = call(&router, "GET", &reader, None).await;
            assert_eq!(
                (status, error_code(&body)),
                (404, "ResourceNotFoundException"),
                "case {:?}: the removed {kind} {id} must itself be gone, got {status} {body}",
                case.label
            );
        }

        // Every grant that still names a live resource survived, exactly as it was.
        for (grant, before) in survivors.iter().zip(before.iter()) {
            let (status, body) = describe_grant(&router, &grant.id).await;
            assert_eq!(
                status, 200,
                "case {:?}: {} names a live resource and must survive, got {status} {body}",
                case.label, grant.label
            );
            assert_eq!(
                &body, before,
                "case {:?}: {} must be untouched by the delete, got {body}",
                case.label, grant.label
            );
        }
        assert_eq!(
            stored_grant_ids(&engine),
            expected_ids,
            "case {:?}: the store must hold exactly the surviving grants",
            case.label
        );

        // The invariant the sweep exists for: every grant the store still holds can
        // be REWRITTEN, because the Monitor resource it names is one this service
        // holds — the same resolution `UpdateAccessPolicy` makes. A grant the delete
        // had left behind would answer 404 here on the only resource it carries.
        for grant in &survivors {
            let (_, _, reader) = resource_address(&grant.resource);
            let (status, body) = call(&router, "GET", &reader, None).await;
            assert_eq!(
                status, 200,
                "case {:?}: the resource {} of {} must still resolve, got {status} {body}",
                case.label, reader, grant.label
            );
            let (status, updated) = call(
                &router,
                "PUT",
                &format!("{ACCESS_POLICIES}/{}", grant.id),
                Some(json!({
                    "accessPolicyIdentity": grant.identity,
                    "accessPolicyResource": grant.resource,
                    "accessPolicyPermission": grant.permission,
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: a rewrite of {} must be served, got {status} {updated}",
                case.label, grant.label
            );
        }
    }
}

#[tokio::test]
async fn a_refused_delete_removes_no_grant() {
    let case = SweepCase {
        label: "untouched-by-a-refused-delete",
        sweep: Sweep::Portal,
        projects: 2,
        portal_grants: 1,
        grants_per_project: 2,
        tags: true,
        client_token: None,
    };
    let (router, engine) = router_with_engine();
    let stored = build(&router, &case, "only").await;
    let grants = stored.all_grants();
    let mut expected_ids: Vec<String> = grants.iter().map(|grant| grant.id.clone()).collect();
    expected_ids.sort();

    // Well-formed ids no create ever minted, addressed at both deletes: each is a
    // modelled 404 that removed no resource, so it may sweep no grant.
    let absent = [
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "00000000-0000-0000-0000-000000000001",
    ];
    for probe in absent {
        for path in [PORTALS, PROJECTS] {
            let (status, body) = call(&router, "DELETE", &format!("{path}/{probe}"), None).await;
            assert_eq!(
                (status, error_code(&body)),
                (404, "ResourceNotFoundException"),
                "case {probe:?} at {path}: expected a modelled 404, got {status} {body}"
            );
        }
    }

    for grant in &grants {
        let (status, body) = describe_grant(&router, &grant.id).await;
        assert_eq!(
            status, 200,
            "case {}: a refused delete must leave the grant in place, got {status} {body}",
            grant.label
        );
        let (status, tags) = list_tags(&router, &grant.arn).await;
        assert_eq!(
            status, 200,
            "case {}: a refused delete must leave the grant's tags in place, \
             got {status} {tags}",
            grant.label
        );
    }
    assert_eq!(
        listed_ids(&router, case.label, &[]).await,
        expected_ids,
        "a refused delete must leave every grant listed"
    );
    assert_eq!(
        stored_grant_ids(&engine),
        expected_ids,
        "a refused delete must leave every grant in the store"
    );
}
