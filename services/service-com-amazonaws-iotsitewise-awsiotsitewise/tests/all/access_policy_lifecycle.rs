// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise Monitor access-policy slice
//! (`CreateAccessPolicy` -> `DescribeAccessPolicy` -> `UpdateAccessPolicy`),
//! driven over HTTP through the same `MultiServiceRouter` the emulator boots
//! with, so every assertion is about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a created access policy is persisted and reads back exactly as it was
//!   created — its identity, its Monitor resource and its permission — with the
//!   ARN form `CreateAccessPolicy` documents, and a read never changes what the
//!   next caller sees;
//! * an update REPLACES every member it carries (all of them are required) and
//!   leaves the policy's identity and provenance alone: same id, same ARN, same
//!   tags, same creation date, with the last-update date moved to the instant of
//!   the write;
//! * an identity or a Monitor resource that does not name EXACTLY ONE
//!   alternative is refused `InvalidRequestException`, on the create and on the
//!   update alike, and stores nothing;
//! * an id that resolves to no stored access policy, and a policy about a
//!   Monitor resource this store does not hold — in EITHER form a `Resource`
//!   takes, a portal or a project — are modelled 404s rather than fabricated
//!   answers.
//!
//! The one refusal these suites do NOT exercise is the `MAX_ACCESS_POLICIES`
//! ceiling: the bound is 1000 policies, so covering it would mean a thousand
//! creates in a single test — the same ceiling on the sibling resource maps is
//! left uncovered for the same reason.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`) is the one
//! the asset suite already has; this suite reuses it rather than growing a second.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, is_modelled_id, router, REGION};
use crate::project_lifecycle::{create_body as project_create_body, PROJECTS};

/// Where every access-policy request goes, as `OPERATIONS.md` reports the routes:
/// `POST /access-policies`, `GET|PUT /access-policies/{accessPolicyId}`.
const ACCESS_POLICIES: &str = "/access-policies";

/// Where the portal a policy can be about is created: `POST /portals`.
const PORTALS: &str = "/portals";

/// The `Identity` alternatives, one per member the shape declares. Source:
/// `smithy_query(query="shape", shape="Identity")` — `user`, `group`, `iamUser`,
/// `iamRole`, the whole member list — with each id/ARN inside the modelled
/// `IdentityId` (len 1..256, `^\S+$`) and `IamArn` bounds.
///
/// `pub(crate)` because the sibling Monitor suites drive the same identity forms;
/// they share this one catalogue rather than growing a second.
pub(crate) fn identity_catalog() -> Vec<(&'static str, Value)> {
    vec![
        ("user", json!({ "user": { "id": "94effa50-e1b4-4a3c-9a5a-8bf2e0da6a21" } })),
        ("group", json!({ "group": { "id": "eb1b5f70-9b2b-4e1d-bc40-5b5f8f4df0c2" } })),
        (
            "iamUser",
            json!({ "iamUser": { "arn": "arn:aws:iam::123456789012:user/monitor-viewer" } }),
        ),
        (
            "iamRole",
            json!({ "iamRole": { "arn": "arn:aws:iam::123456789012:role/monitor-admin" } }),
        ),
    ]
}

/// The `Permission` values. Source: `smithy_query(query="shape",
/// shape="Permission")` — `ADMINISTRATOR` | `VIEWER`, the enum's whole list.
pub(crate) const PERMISSIONS: [&str; 2] = ["ADMINISTRATOR", "VIEWER"];

/// The `Resource` alternatives. Source: `smithy_query(query="shape",
/// shape="Resource")` — `portal` | `project`, the whole member list.
const RESOURCE_KINDS: [&str; 2] = ["portal", "project"];

/// A modelled `ID` built from `seed`: a lowercase hyphenated UUID that is not the
/// all-zero one the pattern refuses. Used for the ids this suite expects to
/// resolve to nothing, and for the resources a request refused before anything is
/// read names without ever reaching the store.
fn modelled_id(seed: u32) -> String {
    format!("{seed:08x}-1c2d-4e5f-8a9b-0c1d2e3f4a5b")
}

/// The `accessPolicyResource` of `kind`: the portal form about `portal_id`, the
/// project form about `project_id`. Both name a resource the store really holds,
/// because an access policy is about a Monitor resource that exists.
fn resource_json(kind: &str, portal_id: &str, project_id: &str) -> Value {
    match kind {
        "portal" => json!({ "portal": { "id": portal_id } }),
        "project" => json!({ "project": { "id": project_id } }),
        other => panic!("unknown resource kind {other:?}"),
    }
}

/// One row of the create-and-read-back table.
#[derive(Debug)]
struct PolicyCase {
    label: String,
    identity: Value,
    resource: Value,
    permission: &'static str,
    /// The `tags` the create carries, if any.
    tags: Option<Value>,
    /// The `clientToken` the create carries, if any.
    client_token: Option<String>,
}

/// The rows: the full cross product of the three catalogues above — every
/// `Identity` alternative, each `Permission` value and each `Resource`
/// alternative — with tags on every third row and a `clientToken` on every
/// second, so the table also spans "carries the optional members" and "carries
/// none of them".
fn policy_cases(portal_id: &str, project_id: &str) -> Vec<PolicyCase> {
    let mut cases = Vec::new();
    for (identity_label, identity) in identity_catalog() {
        for permission in PERMISSIONS {
            for kind in RESOURCE_KINDS {
                let at = cases.len() as u32;
                cases.push(PolicyCase {
                    label: format!("{identity_label}-{permission}-{kind}"),
                    identity: identity.clone(),
                    resource: resource_json(kind, portal_id, project_id),
                    permission,
                    tags: at.is_multiple_of(3).then(|| {
                        json!({ "stage": "test", "slot": format!("{at}") })
                    }),
                    client_token: (!at.is_multiple_of(2))
                        .then(|| format!("client-token-{at:0>36}")),
                });
            }
        }
    }
    cases
}

/// The create body one row sends.
fn create_body(case: &PolicyCase) -> Value {
    let mut body = Map::new();
    body.insert(
        "accessPolicyIdentity".to_string(),
        case.identity.clone(),
    );
    body.insert(
        "accessPolicyResource".to_string(),
        case.resource.clone(),
    );
    body.insert(
        "accessPolicyPermission".to_string(),
        Value::String(case.permission.to_string()),
    );
    if let Some(tags) = case.tags.clone() {
        body.insert("tags".to_string(), tags);
    }
    if let Some(token) = case.client_token.clone() {
        body.insert("clientToken".to_string(), Value::String(token));
    }
    Value::Object(body)
}

/// Create a portal and return its id: the Monitor resource a portal-scoped
/// policy is about, and the portal a project is created in.
pub(crate) async fn create_portal(router: &MultiServiceRouter, name: &str) -> String {
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
    body["portalId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Create a project in `portal_id` and return its id: the OTHER Monitor resource
/// a policy can be about, and the one every project-scoped row of this suite
/// names.
///
/// `pub(crate)` beside [`create_portal`] because the sibling access-policy suites
/// need the same two Monitor resources, so they share one definition rather than
/// drifting apart. The create body comes from the project suite, which owns it.
pub(crate) async fn create_project(
    router: &MultiServiceRouter,
    portal_id: &str,
    name: &str,
) -> String {
    let (status, created) = call(
        router,
        "POST",
        PROJECTS,
        Some(project_create_body(portal_id, name, None)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: CreateProject expected 201, got {status} {created}"
    );
    let id = created["projectId"].as_str().unwrap_or_default().to_string();
    assert!(
        is_modelled_id(&id),
        "case {name:?}: projectId must be a modelled UUID, got {created}"
    );
    id
}

/// `CreateAccessPolicy` with `body`.
async fn create_policy(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", ACCESS_POLICIES, Some(body)).await
}

/// `DescribeAccessPolicy` on `id`.
async fn describe_policy(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{ACCESS_POLICIES}/{id}"), None).await
}

/// `UpdateAccessPolicy` on `id` with `body`.
async fn update_policy(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("{ACCESS_POLICIES}/{id}"),
        Some(body),
    )
    .await
}

/// `ListTagsForResource` on `arn`, the reader the create's `tags` are proved
/// through.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags?resourceArn={arn}"), None).await
}

#[tokio::test]
async fn created_access_policy_reads_back_through_describe_for_every_input_shape() {
    let router = router();
    let portal_id = create_portal(&router, "portal-for-access-policies").await;
    let project_id = create_project(&router, &portal_id, "project-for-access-policies").await;
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    for case in policy_cases(&portal_id, &project_id) {
        let body = create_body(&case);
        let (status, created) = create_policy(&router, body.clone()).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201, got {status} {created}",
            case.label
        );

        let id = created["accessPolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_id(&id),
            "case {:?}: accessPolicyId must be a modelled UUID, got {created}",
            case.label
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {:?}: accessPolicyId {id} was already minted for another policy",
            case.label
        );
        let arn = created["accessPolicyArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            arn,
            format!(
                "arn:aws:iotsitewise:{REGION}:{}:access-policy/{id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {:?}: unexpected accessPolicyArn in {created}",
            case.label
        );

        let (status, described) = describe_policy(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeAccessPolicy expected 200, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["accessPolicyId"].as_str().unwrap_or_default(),
            id,
            "case {:?}: the read reported another id in {described}",
            case.label
        );
        assert_eq!(
            described["accessPolicyArn"].as_str().unwrap_or_default(),
            arn,
            "case {:?}: the read reported another ARN in {described}",
            case.label
        );
        for member in [
            "accessPolicyIdentity",
            "accessPolicyResource",
            "accessPolicyPermission",
        ] {
            assert_eq!(
                &described[member], &body[member],
                "case {:?}: {member} must read back as it was sent, got {described}",
                case.label
            );
        }

        let creation = described["accessPolicyCreationDate"]
            .as_f64()
            .unwrap_or_default();
        let last_update = described["accessPolicyLastUpdateDate"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            creation > 0.0,
            "case {:?}: accessPolicyCreationDate must be a real instant, got {described}",
            case.label
        );
        assert_eq!(
            creation, last_update,
            "case {:?}: an untouched policy's lastUpdateDate is its creationDate, got {described}",
            case.label
        );

        // Reading does not change what the next caller sees.
        let (status, again) = describe_policy(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the second read expected 200, got {status} {again}",
            case.label
        );
        assert_eq!(
            again, described,
            "case {:?}: DescribeAccessPolicy answered differently on the second read: {again}",
            case.label
        );

        // The tags the create carried are persisted on the policy, and a policy
        // created without tags carries an empty set rather than another policy's.
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource expected 200, got {status} {listed}",
            case.label
        );
        let expected_tags = case.tags.clone().unwrap_or_else(|| json!({}));
        assert_eq!(
            listed["tags"], expected_tags,
            "case {:?}: unexpected tag set in {listed}",
            case.label
        );
    }
}

#[tokio::test]
async fn update_replaces_every_caller_owned_member_and_keeps_the_policy_s_identity() {
    let router = router();
    let portal_id = create_portal(&router, "portal-for-access-policy-updates").await;
    let project_id = create_project(&router, &portal_id, "project-for-access-policy-updates").await;
    let cases = policy_cases(&portal_id, &project_id);

    for (at, case) in cases.iter().enumerate() {
        let created_body = create_body(case);
        let (status, created) = create_policy(&router, created_body.clone()).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201, got {status} {created}",
            case.label
        );
        let id = created["accessPolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, before) = describe_policy(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the read before the update expected 200, got {status} {before}",
            case.label
        );

        // The update states the policy's NEW identity, resource and permission in
        // full: the next row's, so every write differs from what it replaces along
        // each of the three members at once.
        let next = &cases[(at + 1) % cases.len()];
        let update = json!({
            "accessPolicyIdentity": next.identity.clone(),
            "accessPolicyResource": next.resource.clone(),
            "accessPolicyPermission": next.permission,
        });
        // Far enough apart that the two instants cannot render as one timestamp.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (status, updated) = update_policy(&router, &id, update.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateAccessPolicy expected 200, got {status} {updated}",
            case.label
        );

        let (status, after) = describe_policy(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the read after the update expected 200, got {status} {after}",
            case.label
        );
        for member in [
            "accessPolicyIdentity",
            "accessPolicyResource",
            "accessPolicyPermission",
        ] {
            assert_eq!(
                &after[member], &update[member],
                "case {:?}: {member} must read back as the update stated it, got {after}",
                case.label
            );
        }
        for member in ["accessPolicyId", "accessPolicyArn", "accessPolicyCreationDate"] {
            assert_eq!(
                &after[member], &before[member],
                "case {:?}: the update must leave {member} alone, got {after}",
                case.label
            );
        }
        let before_update = before["accessPolicyLastUpdateDate"]
            .as_f64()
            .unwrap_or_default();
        let after_update = after["accessPolicyLastUpdateDate"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            after_update > before_update,
            "case {:?}: the update must move lastUpdateDate past {before_update}, got {after}",
            case.label
        );

        // The tags the create carried are the create's, not the update's to
        // change: the update carries no `tags` member at all.
        let arn = created["accessPolicyArn"].as_str().unwrap_or_default();
        let (status, listed) = list_tags(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource expected 200, got {status} {listed}",
            case.label
        );
        assert_eq!(
            listed["tags"],
            case.tags.clone().unwrap_or_else(|| json!({})),
            "case {:?}: the update must leave the tag set alone, got {listed}",
            case.label
        );
    }
}

/// One row of the refusal table: the label, the `accessPolicyIdentity` and the
/// `accessPolicyResource` the request sends, and how many alternatives each of
/// them names.
fn malformed_choice_cases() -> Vec<(&'static str, Value, Value)> {
    let portalish = json!({ "portal": { "id": modelled_id(0x51) } });
    let user = json!({ "user": { "id": "idc-user-for-refusals" } });
    vec![
        ("identity-names-none", json!({}), portalish.clone()),
        (
            "identity-names-two",
            json!({
                "user": { "id": "idc-user-a" },
                "group": { "id": "idc-group-b" },
            }),
            portalish.clone(),
        ),
        (
            "identity-names-all-four",
            json!({
                "user": { "id": "idc-user-c" },
                "group": { "id": "idc-group-d" },
                "iamUser": { "arn": "arn:aws:iam::123456789012:user/one" },
                "iamRole": { "arn": "arn:aws:iam::123456789012:role/two" },
            }),
            portalish,
        ),
        ("resource-names-none", user.clone(), json!({})),
        (
            "resource-names-both",
            user,
            json!({
                "portal": { "id": modelled_id(0x52) },
                "project": { "id": modelled_id(0x53) },
            }),
        ),
    ]
}

#[tokio::test]
async fn an_identity_or_resource_that_is_not_exactly_one_alternative_is_refused() {
    let router = router();
    let portal_id = create_portal(&router, "portal-for-access-policy-refusals").await;

    // A stored policy the update rows below address, so a refusal cannot be the
    // policy's absence.
    let (status, created) = create_policy(
        &router,
        json!({
            "accessPolicyIdentity": { "user": { "id": "idc-user-seed" } },
            "accessPolicyResource": { "portal": { "id": portal_id } },
            "accessPolicyPermission": "VIEWER",
        }),
    )
    .await;
    assert_eq!(status, 201, "the seed create expected 201, got {status} {created}");
    let id = created["accessPolicyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (_, before) = describe_policy(&router, &id).await;

    for (label, identity, resource) in malformed_choice_cases() {
        let body = json!({
            "accessPolicyIdentity": identity,
            "accessPolicyResource": resource,
            "accessPolicyPermission": "ADMINISTRATOR",
        });

        let (status, refused) = create_policy(&router, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {label:?}: CreateAccessPolicy expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {label:?}: unexpected error code in {refused}"
        );

        let (status, refused) = update_policy(&router, &id, body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: UpdateAccessPolicy expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {label:?}: unexpected error code in {refused}"
        );
    }

    // A refused write stores nothing: the seeded policy is exactly as it was.
    let (status, after) = describe_policy(&router, &id).await;
    assert_eq!(status, 200, "the seeded policy expected 200, got {status} {after}");
    assert_eq!(
        after, before,
        "a refused create or update must leave the stored policy alone, got {after}"
    );
}

#[tokio::test]
async fn an_id_that_resolves_to_nothing_is_a_modelled_not_found() {
    let router = router();
    let portal_id = create_portal(&router, "portal-for-access-policy-404s").await;
    let identity = json!({ "iamRole": { "arn": "arn:aws:iam::123456789012:role/absent" } });

    // An access-policy id no create minted: the read and the write both answer the
    // modelled 404, naming the id the request supplied.
    for unknown in [modelled_id(0x90), modelled_id(0x91)] {
        let (status, refused) = describe_policy(&router, &unknown).await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: DescribeAccessPolicy expected 404, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {unknown:?}: unexpected error code in {refused}"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            format!("Access Policy with ID {unknown} not found"),
            "case {unknown:?}: unexpected refusal message in {refused}"
        );

        let (status, refused) = update_policy(
            &router,
            &unknown,
            json!({
                "accessPolicyIdentity": identity.clone(),
                "accessPolicyResource": { "portal": { "id": portal_id } },
                "accessPolicyPermission": "VIEWER",
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: UpdateAccessPolicy expected 404, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {unknown:?}: unexpected error code in {refused}"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            format!("Access Policy with ID {unknown} not found"),
            "case {unknown:?}: unexpected refusal message in {refused}"
        );
    }

    // A policy is about a Monitor resource, so a portal this store does not hold
    // is the same modelled 404 — on the create and on the update of a policy that
    // does exist.
    let (status, created) = create_policy(
        &router,
        json!({
            "accessPolicyIdentity": identity.clone(),
            "accessPolicyResource": { "portal": { "id": portal_id } },
            "accessPolicyPermission": "VIEWER",
        }),
    )
    .await;
    assert_eq!(status, 201, "the seed create expected 201, got {status} {created}");
    let id = created["accessPolicyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for absent_portal in [modelled_id(0x92), modelled_id(0x93)] {
        let body = json!({
            "accessPolicyIdentity": identity.clone(),
            "accessPolicyResource": { "portal": { "id": absent_portal } },
            "accessPolicyPermission": "ADMINISTRATOR",
        });
        let expected = format!("Portal with ID {absent_portal} not found");

        let (status, refused) = create_policy(&router, body.clone()).await;
        assert_eq!(
            status, 404,
            "case {absent_portal:?}: CreateAccessPolicy expected 404, got {status} {refused}"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            expected,
            "case {absent_portal:?}: unexpected refusal message in {refused}"
        );

        let (status, refused) = update_policy(&router, &id, body).await;
        assert_eq!(
            status, 404,
            "case {absent_portal:?}: UpdateAccessPolicy expected 404, got {status} {refused}"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            expected,
            "case {absent_portal:?}: unexpected refusal message in {refused}"
        );
    }
}

/// `DeletePortal` on `id`, the way a portal this store once held stops being held:
/// `DELETE /portals/{portalId} -> 202`, as `OPERATIONS.md` reports the route.
async fn delete_portal(router: &MultiServiceRouter, id: &str) {
    let (status, body) = call(router, "DELETE", &format!("{PORTALS}/{id}"), None).await;
    assert_eq!(
        status, 202,
        "DeletePortal on {id} expected 202, got {status} {body}"
    );
}

/// `DeleteProject` on `id`: `DELETE /projects/{projectId} -> 204`.
async fn delete_project(router: &MultiServiceRouter, id: &str) {
    let (status, body) = call(router, "DELETE", &format!("{PROJECTS}/{id}"), None).await;
    assert_eq!(
        status, 204,
        "DeleteProject on {id} expected 204, got {status} {body}"
    );
}

/// How many access policies the store holds, read through `ListAccessPolicies`
/// (`GET /access-policies`) — the only way to see that a refused write stored
/// nothing at all, and well under the listing's default page size here.
async fn stored_policy_count(router: &MultiServiceRouter) -> usize {
    let (status, listed) = call(router, "GET", ACCESS_POLICIES, None).await;
    assert_eq!(
        status, 200,
        "ListAccessPolicies expected 200, got {status} {listed}"
    );
    listed["accessPolicySummaries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// The resource kind a not-found refusal names each `Resource` alternative by.
/// Source: every not-found refusal of this service states the absence as
/// `<Resource> with ID <id> not found`, with `Portal` and `Project` as the kinds
/// of the two alternatives `smithy_query(query="shape", shape="Resource")`
/// declares.
fn resource_kind_label(form: &str) -> &'static str {
    match form {
        "portal" => "Portal",
        "project" => "Project",
        other => panic!("unknown resource kind {other:?}"),
    }
}

/// One row of the absent-Monitor-resource table: how the request names the
/// resource (`portal` or `project`), the identifier it names it by, and why that
/// identifier resolves to nothing.
#[derive(Debug)]
struct AbsentResourceCase {
    label: String,
    form: &'static str,
    id: String,
}

impl AbsentResourceCase {
    /// The `accessPolicyResource` this row sends.
    fn resource(&self) -> Value {
        resource_json(self.form, &self.id, &self.id)
    }

    /// The refusal message this row must draw.
    fn expected_message(&self) -> String {
        format!(
            "{} with ID {} not found",
            resource_kind_label(self.form),
            self.id
        )
    }
}

#[tokio::test]
async fn a_monitor_resource_the_store_does_not_hold_is_a_modelled_not_found_in_either_form() {
    let router = router();
    let portal_id = create_portal(&router, "portal-the-store-holds").await;
    let project_id = create_project(&router, &portal_id, "project-the-store-holds").await;

    // A portal and a project the store really held and holds no longer, plus a
    // project that went with its portal: an absence the store arrived at by a
    // delete rather than by never having minted the id.
    let emptied_portal = create_portal(&router, "portal-deleted-since").await;
    let project_of_emptied_portal =
        create_project(&router, &emptied_portal, "project-of-the-deleted-portal").await;
    delete_portal(&router, &emptied_portal).await;
    let deleted_project = create_project(&router, &portal_id, "project-deleted-since").await;
    delete_project(&router, &deleted_project).await;

    // The rows span both alternatives a `Resource` takes and every way an
    // identifier of one comes to resolve to nothing: never minted, minted and
    // deleted, taken away with its portal, and an id the OTHER map holds — which
    // is the same id SHAPE, so only the map consulted can tell the rows apart.
    let cases = vec![
        AbsentResourceCase {
            label: "portal-never-minted".to_string(),
            form: "portal",
            id: modelled_id(0xa0),
        },
        AbsentResourceCase {
            label: "project-never-minted".to_string(),
            form: "project",
            id: modelled_id(0xa1),
        },
        AbsentResourceCase {
            label: "portal-deleted-since".to_string(),
            form: "portal",
            id: emptied_portal,
        },
        AbsentResourceCase {
            label: "project-deleted-since".to_string(),
            form: "project",
            id: deleted_project,
        },
        AbsentResourceCase {
            label: "project-taken-with-its-portal".to_string(),
            form: "project",
            id: project_of_emptied_portal,
        },
        AbsentResourceCase {
            label: "a-projects-id-in-the-portal-form".to_string(),
            form: "portal",
            id: project_id.clone(),
        },
        AbsentResourceCase {
            label: "a-portals-id-in-the-project-form".to_string(),
            form: "project",
            id: portal_id.clone(),
        },
    ];

    // A stored policy the update rows address, so a refusal cannot be the policy's
    // own absence.
    let identity = json!({ "iamRole": { "arn": "arn:aws:iam::123456789012:role/monitor-admin" } });
    let (status, created) = create_policy(
        &router,
        json!({
            "accessPolicyIdentity": identity.clone(),
            "accessPolicyResource": { "portal": { "id": portal_id } },
            "accessPolicyPermission": "VIEWER",
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "the seed create expected 201, got {status} {created}"
    );
    let id = created["accessPolicyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, before) = describe_policy(&router, &id).await;
    assert_eq!(
        status, 200,
        "the seeded policy expected 200, got {status} {before}"
    );
    let policies_before = stored_policy_count(&router).await;

    for case in &cases {
        let body = json!({
            "accessPolicyIdentity": identity.clone(),
            "accessPolicyResource": case.resource(),
            "accessPolicyPermission": "ADMINISTRATOR",
        });
        let expected = case.expected_message();

        let (status, refused) = create_policy(&router, body.clone()).await;
        assert_eq!(
            status, 404,
            "case {:?}: CreateAccessPolicy expected 404, got {status} {refused}",
            case.label
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {:?}: unexpected error code in {refused}",
            case.label
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            expected,
            "case {:?}: unexpected refusal message in {refused}",
            case.label
        );

        let (status, refused) = update_policy(&router, &id, body).await;
        assert_eq!(
            status, 404,
            "case {:?}: UpdateAccessPolicy expected 404, got {status} {refused}",
            case.label
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {:?}: unexpected error code in {refused}",
            case.label
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            expected,
            "case {:?}: unexpected refusal message in {refused}",
            case.label
        );

        let (status, unchanged) = describe_policy(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the seeded policy expected 200, got {status} {unchanged}",
            case.label
        );
        assert_eq!(
            unchanged, before,
            "case {:?}: a refused update must leave the stored policy alone, got {unchanged}",
            case.label
        );
    }

    // A refused create stored nothing: the store holds exactly the policies it
    // held before the table ran.
    let policies_after = stored_policy_count(&router).await;
    assert_eq!(
        policies_after, policies_before,
        "a refused create must store no access policy, got {policies_after} \
         where {policies_before} were held"
    );

    // The same check ADMITS what the store does hold, in either form: the project
    // every refused row above names a neighbour of is a Monitor resource a policy
    // can be created about and updated to.
    for form in RESOURCE_KINDS {
        let resource = resource_json(form, &portal_id, &project_id);
        let (status, accepted) = create_policy(
            &router,
            json!({
                "accessPolicyIdentity": identity.clone(),
                "accessPolicyResource": resource.clone(),
                "accessPolicyPermission": "ADMINISTRATOR",
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {form:?}: a policy about a stored Monitor resource expected 201, \
             got {status} {accepted}"
        );
        let accepted_id = accepted["accessPolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, described) = describe_policy(&router, &accepted_id).await;
        assert_eq!(
            status, 200,
            "case {form:?}: DescribeAccessPolicy expected 200, got {status} {described}"
        );
        assert_eq!(
            described["accessPolicyResource"], resource,
            "case {form:?}: the read must report the Monitor resource the create named, \
             got {described}"
        );

        let (status, updated) = update_policy(
            &router,
            &id,
            json!({
                "accessPolicyIdentity": identity.clone(),
                "accessPolicyResource": resource.clone(),
                "accessPolicyPermission": "VIEWER",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {form:?}: UpdateAccessPolicy expected 200, got {status} {updated}"
        );
        let (status, described) = describe_policy(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {form:?}: the read after the update expected 200, got {status} {described}"
        );
        assert_eq!(
            described["accessPolicyResource"], resource,
            "case {form:?}: the read must report the Monitor resource the update named, \
             got {described}"
        );
    }
}
