// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteAccessPolicy`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The create-and-read-back rules of the access-policy slice are pinned by
//! `access_policy_lifecycle`; this suite is about what the DELETE does, and pins:
//!
//! * a delete removes the ONE policy its id names: that policy stops existing —
//!   a read of it, and a second delete of it, are the modelled 404 — and the tags
//!   it carried go with it, so its ARN resolves to nothing;
//! * every other stored policy is untouched by it, and the store itself holds no
//!   record for a deleted policy, so the delete removed records rather than
//!   hiding them from the read;
//! * an id that resolves to no stored policy is the modelled 404 from the delete
//!   as well, naming the id the REQUEST supplied, and removes nothing.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`,
//! `is_modelled_id`) is the one the asset suite already has; this suite reuses it
//! rather than growing a second.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine, REGION};

/// Where every access-policy request goes, as `OPERATIONS.md` reports the route:
/// `POST /access-policies`, `GET`/`DELETE /access-policies/{accessPolicyId}`.
const ACCESS_POLICIES: &str = "/access-policies";

/// Where every portal request goes — the Monitor resource a portal-scoped policy
/// is about.
const PORTALS: &str = "/portals";

/// A project id of the modelled `ID` form, for a policy about a project.
///
/// Nothing in this service creates a project yet, so a project-scoped policy
/// names one the store cannot resolve; the policy is still a resource of its own,
/// and the delete of it is this suite's subject.
const PROJECT_ID: &str = "11111111-2222-4333-8444-555555555555";

/// Create a portal and return its id, so a policy can be about a Monitor
/// resource this store really holds.
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

/// `DescribeAccessPolicy` on `id`.
async fn describe_policy(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{ACCESS_POLICIES}/{id}"), None).await
}

/// The ARN form `CreateAccessPolicy` documents
/// (`arn:${Partition}:iotsitewise:${Region}:${Account}:access-policy/${AccessPolicyId}`),
/// for the account and region every request in this suite is signed for.
fn expected_arn(id: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:access-policy/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One row of the delete table: a policy to create, and how the delete of it is
/// addressed.
#[derive(Debug)]
struct DeleteCase {
    label: &'static str,
    /// The `Identity` the policy is about. Exactly one member, which is the rule
    /// the shape states (smithy_query query="shape" shape="Identity": `user`,
    /// `group`, `iamUser`, `iamRole`).
    identity: Value,
    /// Whether the policy is about the portal this test created (`true`) or about
    /// a project (`false`) — the two members of `Resource`.
    about_portal: bool,
    /// A `Permission` value; the rows span the enum's whole value list
    /// (smithy_query query="shape" shape="Permission": `ADMINISTRATOR` |
    /// `VIEWER`).
    permission: &'static str,
    /// The tags the create carries, if any: what the delete has to take with the
    /// policy.
    tags: Option<Value>,
    /// Whether the delete sends the optional `clientToken` query member.
    delete_with_client_token: bool,
}

/// The rows. They differ along every dimension of the policy being deleted: each
/// `Identity` member, both `Resource` members, both `Permission` values, tags of
/// several sizes (including none), and the delete addressed with and without its
/// optional `clientToken`.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "sso-user-about-portal-administrator",
            identity: json!({ "user": { "id": "u-0123456789abcdef" } }),
            about_portal: true,
            permission: "ADMINISTRATOR",
            tags: None,
            delete_with_client_token: false,
        },
        DeleteCase {
            label: "sso-group-about-portal-viewer",
            identity: json!({ "group": { "id": "g-0123456789abcdef" } }),
            about_portal: true,
            permission: "VIEWER",
            tags: Some(json!({ "stage": "test" })),
            delete_with_client_token: true,
        },
        DeleteCase {
            label: "iam-user-about-portal-administrator",
            identity: json!({
                "iamUser": { "arn": "arn:aws:iam::123456789012:user/floor-one-operator" },
            }),
            about_portal: true,
            permission: "ADMINISTRATOR",
            tags: Some(json!({ "team": "ops", "env": "edge", "owner": "sitewise" })),
            delete_with_client_token: false,
        },
        DeleteCase {
            label: "iam-role-about-project-viewer",
            identity: json!({
                "iamRole": { "arn": "arn:aws:iam::123456789012:role/floor-one-viewer" },
            }),
            about_portal: false,
            permission: "VIEWER",
            tags: None,
            delete_with_client_token: true,
        },
        DeleteCase {
            label: "sso-user-about-project-administrator",
            identity: json!({ "user": { "id": "u-fedcba9876543210" } }),
            about_portal: false,
            permission: "ADMINISTRATOR",
            tags: Some(json!({ "stage": "prod" })),
            delete_with_client_token: false,
        },
    ]
}

/// The body one row's create sends.
fn create_body(case: &DeleteCase, portal_id: &str) -> Value {
    let resource = if case.about_portal {
        json!({ "portal": { "id": portal_id } })
    } else {
        json!({ "project": { "id": PROJECT_ID } })
    };
    let mut body = json!({
        "accessPolicyIdentity": case.identity.clone(),
        "accessPolicyResource": resource,
        "accessPolicyPermission": case.permission,
    });
    if let Some(tags) = case.tags.as_ref() {
        body["tags"] = tags.clone();
    }
    body
}

/// A `clientToken` within the modelled bounds (len 36..64, `^\S{36,64}$`).
fn client_token(label: &str) -> String {
    let token = format!("{label}-0123456789abcdef0123456789abcdef0123456789");
    token[..48].to_string()
}

#[tokio::test]
async fn a_delete_removes_the_one_policy_it_names_and_leaves_every_other_one() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "portal-for-access-policy-deletes").await;

    // Every row is created first, so each delete happens with siblings in the
    // store to be left alone.
    let cases = delete_cases();
    let mut ids: Vec<String> = Vec::new();
    for case in &cases {
        let (status, created) = call(
            &router,
            "POST",
            ACCESS_POLICIES,
            Some(create_body(case, &portal_id)),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: CreateAccessPolicy expected 201, got {status} {created}",
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
        ids.push(id);
    }

    for (at, case) in cases.iter().enumerate() {
        let id = &ids[at];
        let (status, before) = describe_policy(&router, id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the policy must read back before the delete, got {status} {before}",
            case.label
        );

        let uri = if case.delete_with_client_token {
            format!(
                "{ACCESS_POLICIES}/{id}?clientToken={}",
                client_token(case.label)
            )
        } else {
            format!("{ACCESS_POLICIES}/{id}")
        };
        let (status, answer) = call(&router, "DELETE", &uri, None).await;
        assert_eq!(
            status, 204,
            "case {:?}: DeleteAccessPolicy expected 204, got {status} {answer}",
            case.label
        );
        // The answer carries no members: `DeleteAccessPolicyResponse` models none.
        assert!(
            answer.as_object().is_none_or(serde_json::Map::is_empty),
            "case {:?}: a 204 carries no members, got {answer}",
            case.label
        );

        // The policy stopped existing: the read of it, and a second delete of it,
        // are the modelled 404 naming the id the request supplied.
        for method in ["GET", "DELETE"] {
            let (status, body) =
                call(&router, method, &format!("{ACCESS_POLICIES}/{id}"), None).await;
            assert_eq!(
                status, 404,
                "case {:?}: {method} after the delete expected 404, got {status} {body}",
                case.label
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {:?}: unexpected error code from {method} in {body}",
                case.label
            );
            assert!(
                body["message"].as_str().unwrap_or("").contains(id),
                "case {:?}: the refusal from {method} must name the requested id, got {body}",
                case.label
            );
        }

        // The tags went with the policy, so its ARN resolves to nothing.
        let arn = expected_arn(id);
        let (status, tags) = call(&router, "GET", &format!("/tags?resourceArn={arn}"), None).await;
        assert_eq!(
            status, 404,
            "case {:?}: a deleted policy's ARN resolves to nothing, got {status} {tags}",
            case.label
        );

        // The store holds no record for it either: the delete removed the record
        // rather than hiding it from the read.
        let held = service_state(&*engine)
            .expect("the IoT SiteWise store")
            .lock()
            .access_policies
            .contains_key(id);
        assert!(
            !held,
            "case {:?}: the store must hold no record for a deleted policy",
            case.label
        );

        // Every policy this delete did not name is still there, unchanged.
        for (other_at, other) in cases.iter().enumerate().skip(at + 1) {
            let (status, described) = describe_policy(&router, &ids[other_at]).await;
            assert_eq!(
                status, 200,
                "case {:?}: deleting {:?} must leave this policy readable, got {status} {described}",
                other.label, case.label
            );
            assert_eq!(
                described["accessPolicyIdentity"], other.identity,
                "case {:?}: deleting {:?} changed this policy's identity: {described}",
                other.label, case.label
            );
            assert_eq!(
                described["accessPolicyPermission"]
                    .as_str()
                    .unwrap_or_default(),
                other.permission,
                "case {:?}: deleting {:?} changed this policy's permission: {described}",
                other.label,
                case.label
            );
        }
    }

    let held = service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .access_policies
        .len();
    assert_eq!(
        held, 0,
        "every created policy was deleted, so the store must hold none, it holds {held}"
    );
}

#[tokio::test]
async fn a_delete_of_an_id_no_policy_carries_is_a_modelled_not_found() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "portal-for-access-policy-refusals").await;
    let (status, created) = call(
        &router,
        "POST",
        ACCESS_POLICIES,
        Some(json!({
            "accessPolicyIdentity": { "user": { "id": "u-refusals" } },
            "accessPolicyResource": { "portal": { "id": portal_id } },
            "accessPolicyPermission": "VIEWER",
        })),
    )
    .await;
    assert_eq!(status, 201, "the probe create got {status} {created}");
    let id = created["accessPolicyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Ids within the modelled `ID` form that no stored policy carries: the all-`f`
    // one, and a near neighbour of the one that does exist.
    let mut neighbour: Vec<char> = id.chars().collect();
    let at = neighbour
        .iter()
        .position(|ch| *ch != '-' && *ch != '0')
        .expect("a non-zero character in the id");
    neighbour[at] = '0';
    let neighbour: String = neighbour.into_iter().collect();

    for case in ["ffffffff-ffff-ffff-ffff-ffffffffffff", neighbour.as_str()] {
        assert!(
            is_modelled_id(case),
            "case {case:?}: the probe id must itself be modelled"
        );
        let (status, body) =
            call(&router, "DELETE", &format!("{ACCESS_POLICIES}/{case}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        // The refusal names the identifier the REQUEST supplied, not the one
        // policy that does exist.
        let message = body["message"].as_str().unwrap_or("");
        assert!(
            message.contains(case) && !message.contains(&id),
            "case {case:?}: the refusal must name the requested id only, got {body}"
        );
    }

    // The refused deletes left the stored policy where it was.
    let (status, described) = describe_policy(&router, &id).await;
    assert_eq!(
        status, 200,
        "the refusals must leave the stored policy readable, got {status} {described}"
    );
    let held = service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .access_policies
        .len();
    assert_eq!(
        held, 1,
        "a refused delete removes nothing, so the store must still hold one policy, it holds {held}"
    );
}
