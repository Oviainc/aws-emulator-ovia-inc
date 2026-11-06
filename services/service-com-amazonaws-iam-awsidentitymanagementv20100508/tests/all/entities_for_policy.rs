// SPDX-License-Identifier: CC0-1.0

//! `ListEntitiesForPolicy` — the entities ONE managed policy relates to — end to
//! end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour (status, awsQuery error code, XML members) of a
//! relation that was PERSISTED by `AttachRolePolicy` or by a create carrying a
//! `PermissionsBoundary` — never a handler called directly. The awsQuery harness is
//! the one `tests/all/user_lifecycle.rs` owns and the other suites in this crate
//! share.
//!
//! The suites are TABLES: the fixture spans several policies (at different paths),
//! roles (different paths, name cases and relation kinds) and users, and each row of
//! a suite differs along what the rule generalises over — the entity type filter,
//! the usage filter, the path prefix, the page size, the form of a refused ARN or
//! marker — so no single recorded request is the specification.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s
/// own modelled example carries. Every fixture policy needs one to exist at all.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy with a single statement, which every fixture role needs.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"config.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The customer-managed policies the fixture account holds: `(PolicyName, Path)`.
/// The paths span the account root, a one-segment path and a nested path, so a rule
/// about a policy's ARN is exercised at more than one depth.
const POLICIES: &[(&str, &str)] = &[
    ("ReadItems", "/"),
    ("WriteItems", "/app/"),
    ("AuditItems", "/app/audit/"),
];

/// An AWS-managed policy ARN: an entry of the AWS Managed Policy Reference, owned
/// by AWS rather than by this account, so the account holds no record of it.
const AWS_MANAGED_POLICY: &str = "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess";

/// The `EntityType` values `EntityFilter` accepts, from the model
/// (`smithy_query(query="shape", shape="EntityType")`: User, Role, Group,
/// LocalManagedPolicy, AWSManagedPolicy — the same set
/// `crate::state`'s service records as `ENTITY_TYPE_VALUES`).
const ENTITY_FILTERS: &[&str] = &[
    "User",
    "Role",
    "Group",
    "LocalManagedPolicy",
    "AWSManagedPolicy",
];

/// The `PolicyUsageType` values `PolicyUsageFilter` accepts, from the model
/// (`smithy_query(query="shape", shape="PolicyUsageType")`).
const POLICY_USAGE_FILTERS: &[&str] = &["PermissionsPolicy", "PermissionsBoundary"];

/// The fixture roles: `(RoleName, Path, indices of POLICIES attached to the role as
/// permissions policies, index of the POLICIES entry used as the role's permissions
/// boundary)`.
///
/// The rows differ along every member the rule generalises over: the path (root,
/// one segment, nested), the name's case and alphabet, how many policies the role is
/// attached to (none, one, two), and which KIND of relation it holds (attachment
/// only, boundary only, both, neither).
const ROLES: &[(&str, &str, &[usize], Option<usize>)] = &[
    ("AppRole", "/", &[0], None),
    ("auditRole", "/app/audit/", &[0, 1], Some(2)),
    ("MixedCase.Role-2", "/app/", &[2], None),
    ("bounded_role", "/app/", &[], Some(0)),
    ("UnrelatedRole", "/", &[], None),
];

/// The fixture users: `(UserName, Path, index of the POLICIES entry used as the
/// user's permissions boundary)`. The boundary is the one relation a user row here
/// varies; a user's ATTACHED managed policies are the subject of
/// `tests/all/user_attached_policies.rs`, which drives the same reader.
const USERS: &[(&str, &str, Option<usize>)] = &[
    ("alice", "/", Some(0)),
    ("Bob.9", "/app/", Some(2)),
    ("carol_7", "/app/audit/", None),
];

/// The fixture account: the router every call is driven through, plus the unique id
/// each create MINTED, so a list can be checked against the STORED id rather than
/// against a value recomputed while reading.
struct Fixture {
    router: MultiServiceRouter,
    role_ids: BTreeMap<&'static str, String>,
    user_ids: BTreeMap<&'static str, String>,
}

/// The ARN of the fixture policy at `index`: a customer-managed policy's ARN is
/// `arn:aws:iam::<account>:policy<path><name>`.
fn policy_arn(index: usize) -> String {
    let (name, path) = POLICIES[index];
    format!("arn:aws:iam::{}:policy{path}{name}", account_id())
}

/// Create every fixture policy, role (with its attachments and boundary) and user.
async fn fixture() -> Fixture {
    let router = router();

    for (name, path) in POLICIES {
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &[
                ("PolicyName", name),
                ("Path", path),
                ("PolicyDocument", POLICY_DOCUMENT),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?} at {path:?}: CreatePolicy must succeed, got {status}: {body}"
        );
    }

    let mut role_ids = BTreeMap::new();
    for (name, path, attached, boundary) in ROLES {
        let mut params = vec![
            param("RoleName", name),
            param("Path", path),
            param("AssumeRolePolicyDocument", TRUST_POLICY),
        ];
        if let Some(index) = boundary {
            params.push(param("PermissionsBoundary", &policy_arn(*index)));
        }
        let (status, body) = call(&router, "CreateRole", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {name:?} at {path:?}: CreateRole must succeed, got {status}: {body}"
        );
        role_ids.insert(*name, element(&body, "RoleId"));

        for index in *attached {
            let (status, body) = call(
                &router,
                "AttachRolePolicy",
                &[("RoleName", name), ("PolicyArn", &policy_arn(*index))],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?} <- {:?}: AttachRolePolicy must succeed, got {status}: {body}",
                policy_arn(*index)
            );
        }
    }

    let mut user_ids = BTreeMap::new();
    for (name, path, boundary) in USERS {
        let mut params = vec![param("UserName", name), param("Path", path)];
        if let Some(index) = boundary {
            params.push(param("PermissionsBoundary", &policy_arn(*index)));
        }
        let (status, body) = call(&router, "CreateUser", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {name:?} at {path:?}: CreateUser must succeed, got {status}: {body}"
        );
        user_ids.insert(*name, element(&body, "UserId"));
    }

    Fixture {
        router,
        role_ids,
        user_ids,
    }
}

/// One `ListEntitiesForPolicy` call, with any filtering or paging members the row
/// exercises.
async fn list(
    router: &MultiServiceRouter,
    policy_arn: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![("PolicyArn", policy_arn)];
    params.extend_from_slice(extra);
    call(router, "ListEntitiesForPolicy", &params).await
}

/// The fixture roles the policy at `policy` relates under `usage` and
/// `path_prefix`, in the order the service enumerates them: ascending by the
/// case-folded name, which is the key the role store is ordered on.
///
/// This is RULE (b) restated over the fixture table: a role is related when the
/// policy is among its ATTACHED managed policies (the `PermissionsPolicy` usage) or
/// is its permissions BOUNDARY, and only when its own path carries the prefix.
fn expected_roles(policy: usize, usage: Option<&str>, path_prefix: &str) -> Vec<String> {
    let mut names: Vec<String> = ROLES
        .iter()
        .filter(|(_, path, attached, boundary)| {
            let by_policy = usage != Some("PermissionsBoundary") && attached.contains(&policy);
            let by_boundary = usage != Some("PermissionsPolicy") && *boundary == Some(policy);
            (by_policy || by_boundary) && path.starts_with(path_prefix)
        })
        .map(|(name, ..)| (*name).to_string())
        .collect();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

/// The fixture users the policy at `policy` relates under `usage` and
/// `path_prefix`, in the order the service enumerates them. Only the boundary
/// relation holds for a user of THIS fixture (see [`USERS`]).
fn expected_users(policy: usize, usage: Option<&str>, path_prefix: &str) -> Vec<String> {
    let mut names: Vec<String> = USERS
        .iter()
        .filter(|(_, path, boundary)| {
            usage != Some("PermissionsPolicy")
                && *boundary == Some(policy)
                && path.starts_with(path_prefix)
        })
        .map(|(name, ..)| (*name).to_string())
        .collect();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

/// RULE: `ListEntitiesForPolicy` reports exactly the entities the account relates
/// to the named managed policy — every role the policy is attached to and every role
/// or user whose permissions boundary it is — each under its STORED name and unique
/// id, and reports nothing for a policy nothing relates to.
#[tokio::test]
async fn reports_every_entity_a_policy_relates_with_its_stored_id() {
    let fixture = fixture().await;

    for (index, (name, path)) in POLICIES.iter().enumerate() {
        let case = format!("{name} at {path}");
        let arn = policy_arn(index);
        let (status, body) = list(&fixture.router, &arn, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );

        let roles = elements(&body, "RoleName");
        assert_eq!(
            roles,
            expected_roles(index, None, "/"),
            "case {case:?}: expected the roles {:?}, got {roles:?}",
            expected_roles(index, None, "/")
        );
        let users = elements(&body, "UserName");
        assert_eq!(
            users,
            expected_users(index, None, "/"),
            "case {case:?}: expected the users {:?}, got {users:?}",
            expected_users(index, None, "/")
        );
        // This suite creates no group, so no group carries an attachment to any
        // fixture policy and none can be related to one (a group's attachments are
        // asserted by `tests/all/group_attached_policies.rs`).
        assert!(
            elements(&body, "GroupName").is_empty(),
            "case {case:?}: no group carries an attachment, so none can be reported: {body}"
        );

        // Each entity is reported under the id its create MINTED.
        for (role, id) in roles.iter().zip(elements(&body, "RoleId")) {
            assert_eq!(
                Some(&id),
                fixture.role_ids.get(role.as_str()),
                "case {case:?}: expected role {role:?} under its stored RoleId {:?}, got {id:?}",
                fixture.role_ids.get(role.as_str())
            );
        }
        for (user, id) in users.iter().zip(elements(&body, "UserId")) {
            assert_eq!(
                Some(&id),
                fixture.user_ids.get(user.as_str()),
                "case {case:?}: expected user {user:?} under its stored UserId {:?}, got {id:?}",
                fixture.user_ids.get(user.as_str())
            );
        }

        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {case:?}: a page holding every related entity is not truncated: {body}"
        );
        assert!(
            elements(&body, "Marker").is_empty(),
            "case {case:?}: a final page must carry no Marker: {body}"
        );

        // A read must not change what a later caller sees.
        let (again_status, again) = list(&fixture.router, &arn, &[]).await;
        assert_eq!(
            again_status, 200,
            "case {case:?}: the second read expected 200, got {again_status}: {again}"
        );
        assert_eq!(
            elements(&again, "RoleName"),
            roles,
            "case {case:?}: a read changed what the next caller sees: {again}"
        );
        assert_eq!(
            elements(&again, "UserName"),
            users,
            "case {case:?}: a read changed what the next caller sees: {again}"
        );
    }
}

/// RULE: a policy the account does not hold relates nothing, and a policy it holds
/// that nothing points at reports three empty lists rather than a refusal.
#[tokio::test]
async fn a_policy_nothing_relates_reports_empty_lists() {
    let fixture = fixture().await;
    let (status, created) = call(
        &fixture.router,
        "CreatePolicy",
        &[
            ("PolicyName", "RelatedToNothing"),
            ("PolicyDocument", POLICY_DOCUMENT),
        ],
    )
    .await;
    assert_eq!(status, 200, "the fixture create must succeed: {created}");
    let arn = element(&created, "Arn");

    let (status, body) = list(&fixture.router, &arn, &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    for tag in ["RoleName", "UserName", "GroupName"] {
        assert!(
            elements(&body, tag).is_empty(),
            "case {tag:?}: an unrelated policy reports no entity, got {:?}: {body}",
            elements(&body, tag)
        );
    }
    assert_eq!(
        element(&body, "IsTruncated"),
        "false",
        "an empty page is not truncated: {body}"
    );
}

/// RULE: `EntityFilter` narrows the answer to the named entity TYPE — "when
/// EntityFilter is Role, only the roles that are attached to the specified policy
/// are returned" — and a value of the shared `EntityType` enum that names no user,
/// group or role (`LocalManagedPolicy`, `AWSManagedPolicy`) relates no entity.
#[tokio::test]
async fn entity_filter_selects_only_that_entity_type() {
    let fixture = fixture().await;

    for (index, (policy, _)) in POLICIES.iter().enumerate() {
        let arn = policy_arn(index);
        for filter in ENTITY_FILTERS {
            let case = format!("{policy} filtered by {filter}");
            let (status, body) = list(&fixture.router, &arn, &[("EntityFilter", filter)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {body}"
            );

            let expected_roles = match *filter {
                "Role" => expected_roles(index, None, "/"),
                _ => Vec::new(),
            };
            let expected_users = match *filter {
                "User" => expected_users(index, None, "/"),
                _ => Vec::new(),
            };
            assert_eq!(
                elements(&body, "RoleName"),
                expected_roles,
                "case {case:?}: expected the roles {expected_roles:?}, got {:?}",
                elements(&body, "RoleName")
            );
            assert_eq!(
                elements(&body, "UserName"),
                expected_users,
                "case {case:?}: expected the users {expected_users:?}, got {:?}",
                elements(&body, "UserName")
            );
            assert!(
                elements(&body, "GroupName").is_empty(),
                "case {case:?}: no group carries an attachment, so none can be reported: {body}"
            );
        }
    }
}

/// RULE: `PolicyUsageFilter` selects WHICH USE of the policy relates an entity —
/// `PermissionsPolicy` the entities it is attached to, `PermissionsBoundary` the
/// entities whose permissions boundary it is — and an absent filter relates both.
#[tokio::test]
async fn policy_usage_filter_separates_attachment_from_permissions_boundary() {
    let fixture = fixture().await;

    for (index, (policy, _)) in POLICIES.iter().enumerate() {
        let arn = policy_arn(index);
        for usage in [None, Some("PermissionsPolicy"), Some("PermissionsBoundary")] {
            let case = format!("{policy} used as {usage:?}");
            let extra: Vec<(&str, &str)> = usage
                .map(|usage| vec![("PolicyUsageFilter", usage)])
                .unwrap_or_default();
            let (status, body) = list(&fixture.router, &arn, &extra).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {body}"
            );

            let roles = expected_roles(index, usage, "/");
            let users = expected_users(index, usage, "/");
            assert_eq!(
                elements(&body, "RoleName"),
                roles,
                "case {case:?}: expected the roles {roles:?}, got {:?}",
                elements(&body, "RoleName")
            );
            assert_eq!(
                elements(&body, "UserName"),
                users,
                "case {case:?}: expected the users {users:?}, got {:?}",
                elements(&body, "UserName")
            );
        }
    }

    // The two usages PARTITION the answer: together they report exactly what the
    // unfiltered call does, whatever the policy.
    for (index, (policy, _)) in POLICIES.iter().enumerate() {
        let mut split = expected_roles(index, Some("PermissionsPolicy"), "/");
        split.extend(expected_roles(index, Some("PermissionsBoundary"), "/"));
        split.sort_by_key(|name| name.to_ascii_lowercase());
        split.dedup();
        assert_eq!(
            split,
            expected_roles(index, None, "/"),
            "case {policy:?}: the two usages must partition the related roles"
        );
    }
}

/// RULE: `PathPrefix` filters by the related ENTITY's own path — "if it is not
/// included, it defaults to a slash (/), listing all entities" — so a prefix no
/// entity's path carries reports nothing.
#[tokio::test]
async fn path_prefix_filters_by_the_entity_path() {
    let fixture = fixture().await;

    // The prefixes span the account root, every path depth the fixture entities
    // live at, and one no entity carries.
    for prefix in ["/", "/app/", "/app/audit/", "/unused/"] {
        for (index, (policy, _)) in POLICIES.iter().enumerate() {
            let case = format!("{policy} under {prefix}");
            let (status, body) = list(&fixture.router, &policy_arn(index), &[("PathPrefix", prefix)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {body}"
            );

            let roles = expected_roles(index, None, prefix);
            let users = expected_users(index, None, prefix);
            assert_eq!(
                elements(&body, "RoleName"),
                roles,
                "case {case:?}: expected the roles {roles:?}, got {:?}",
                elements(&body, "RoleName")
            );
            assert_eq!(
                elements(&body, "UserName"),
                users,
                "case {case:?}: expected the users {users:?}, got {:?}",
                elements(&body, "UserName")
            );
        }
    }
}

/// RULE: `MaxItems` is a real page size over the entities of ONE policy and the
/// `Marker` it hands out resumes strictly after the position it names, so walking
/// the pages yields every related entity exactly once whatever the page size.
#[tokio::test]
async fn pages_over_every_related_entity_whatever_the_page_size() {
    let fixture = fixture().await;

    for (index, (policy, _)) in POLICIES.iter().enumerate() {
        let arn = policy_arn(index);
        let expected_roles = expected_roles(index, None, "/");
        let expected_users = expected_users(index, None, "/");
        let total = expected_roles.len() + expected_users.len();

        for page_size in 1..=(total + 2).max(2) {
            let case = format!("{policy} at page size {page_size}");
            let size = page_size.to_string();
            let mut roles = Vec::new();
            let mut users = Vec::new();
            let mut marker: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut extra = vec![("MaxItems", size.as_str())];
                if let Some(token) = &marker {
                    extra.push(("Marker", token.as_str()));
                }
                let (status, body) = list(&fixture.router, &arn, &extra).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status}: {body}"
                );
                let page_roles = elements(&body, "RoleName");
                let page_users = elements(&body, "UserName");
                assert!(
                    page_roles.len() + page_users.len() <= page_size,
                    "case {case:?}: a page must hold at most {page_size} entities, got {}: {body}",
                    page_roles.len() + page_users.len()
                );
                roles.extend(page_roles);
                users.extend(page_users);
                pages += 1;
                assert!(
                    pages <= total + 1,
                    "case {case:?}: paging must terminate, walked {pages} pages"
                );
                if element(&body, "IsTruncated") == "true" {
                    marker = Some(element(&body, "Marker"));
                } else {
                    assert!(
                        elements(&body, "Marker").is_empty(),
                        "case {case:?}: a final page must carry no Marker: {body}"
                    );
                    break;
                }
            }

            assert_eq!(
                roles, expected_roles,
                "case {case:?}: expected the roles {expected_roles:?} exactly once, got {roles:?}"
            );
            assert_eq!(
                users, expected_users,
                "case {case:?}: expected the users {expected_users:?} exactly once, got {users:?}"
            );
        }
    }
}

/// RULE: a `Marker` names a position in ONE policy's entity order, so a token this
/// service did not issue for that policy — a foreign string, or one it issued while
/// paging another policy — is refused with 400 `InvalidInput` rather than read as
/// "start from the beginning".
#[tokio::test]
async fn refuses_a_marker_it_did_not_issue_for_that_policy() {
    let fixture = fixture().await;
    // The policy at index 0 relates several entities, so a one-item page of it is
    // truncated and hands out a marker.
    let paged = policy_arn(0);
    let (status, first_page) = list(&fixture.router, &paged, &[("MaxItems", "1")]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {first_page}");
    assert_eq!(
        element(&first_page, "IsTruncated"),
        "true",
        "a one-item page of several related entities is truncated: {first_page}"
    );
    let marker = element(&first_page, "Marker");

    // It resumes THAT policy strictly after the first entity …
    let (status, resumed) = list(
        &fixture.router,
        &paged,
        &[("Marker", marker.as_str()), ("MaxItems", "1000")],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {resumed}");
    let mut resumed_names = elements(&resumed, "RoleName");
    resumed_names.extend(elements(&resumed, "UserName"));
    let mut tail = expected_roles(0, None, "/");
    tail.extend(expected_users(0, None, "/"));
    let tail: Vec<String> = tail.into_iter().skip(1).collect();
    assert_eq!(
        resumed_names, tail,
        "expected the marker to resume after the first entity ({tail:?}), got {resumed_names:?}"
    );

    // … and names no position in another policy's order, nor does a token this
    // service never issued.
    for token in [marker.as_str(), "not-a-marker", "aWFtOnVzZXJzOmFsaWNl", "////"] {
        let (status, body) = list(&fixture.router, &policy_arn(1), &[("Marker", token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {token:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
    }
}

/// RULE: `PolicyArn` must name a managed policy that exists — a string that cannot be
/// used as a managed-policy ARN is refused 400, under the code of the layer that took
/// the form (`ValidationError` for a value that is not an ARN at all, this operation's
/// modelled `InvalidInput` for an ARN naming another service or resource type), and a
/// well-formed ARN the account holds no policy under with 404 `NoSuchEntity`.
#[tokio::test]
async fn refuses_an_arn_that_names_no_managed_policy() {
    let fixture = fixture().await;
    let account = account_id();

    // The malformed forms span each part of the ARN grammar: the wrong service, a
    // region segment (IAM is global), the wrong resource type, an empty path
    // segment, a missing name, and a string that is no ARN at all. Each is padded
    // to `arnType`'s 20-character floor, which the framework refuses before the
    // handler.
    for (arn, expected_code) in [
        ("arn:aws:s3:::policy/ReadItems".to_string(), "InvalidInput"),
        (
            format!("arn:aws:iam:us-east-1:{account}:policy/ReadItems"),
            "InvalidInput",
        ),
        (format!("arn:aws:iam::{account}:role/ReadItems"), "InvalidInput"),
        (
            format!("arn:aws:iam::{account}:policy//ReadItems"),
            "InvalidInput",
        ),
        (format!("arn:aws:iam::{account}:policy/"), "InvalidInput"),
        ("not-an-arn-at-all-but-long".to_string(), "ValidationError"),
    ] {
        let (status, body) = list(&fixture.router, &arn, &[]).await;
        assert_eq!(
            status, 400,
            "case {arn:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {arn:?}: expected {expected_code}, got {:?}: {body}",
            error_code(&body)
        );
    }

    // Well-formed customer-managed ARNs the account holds no policy under: an
    // unheld name, a held name at another path, and another account's policy.
    for arn in [
        format!("arn:aws:iam::{account}:policy/NeverCreated"),
        format!("arn:aws:iam::{account}:policy/other/ReadItems"),
        "arn:aws:iam::210987654321:policy/ReadItems".to_string(),
    ] {
        let (status, body) = list(&fixture.router, &arn, &[]).await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {arn:?}: expected NoSuchEntity, got {:?}: {body}",
            error_code(&body)
        );
    }
}

/// RULE: an AWS-managed policy is owned by AWS rather than by the account, so it is
/// never "an entity that does not exist" — it relates exactly the account's entities
/// that point at it.
#[tokio::test]
async fn an_aws_managed_policy_relates_the_roles_it_is_attached_to() {
    let fixture = fixture().await;

    // Before anything points at it, the AWS-managed policy relates nothing — and
    // that is a 200, not a 404.
    let (status, body) = list(&fixture.router, AWS_MANAGED_POLICY, &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert!(
        elements(&body, "RoleName").is_empty(),
        "expected no related role yet, got {:?}: {body}",
        elements(&body, "RoleName")
    );

    // Attaching it to two fixture roles relates exactly those, in name-key order.
    for role in ["AppRole", "UnrelatedRole"] {
        let (status, body) = call(
            &fixture.router,
            "AttachRolePolicy",
            &[("RoleName", role), ("PolicyArn", AWS_MANAGED_POLICY)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {role:?}: the fixture attach must succeed, got {status}: {body}"
        );
    }

    let (status, body) = list(&fixture.router, AWS_MANAGED_POLICY, &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        elements(&body, "RoleName"),
        vec!["AppRole".to_string(), "UnrelatedRole".to_string()],
        "expected exactly the two attached roles, got {:?}: {body}",
        elements(&body, "RoleName")
    );
}

/// RULE: each filter names ONE value of its modelled enum, so a value the enum does
/// not declare is refused as a validation error on that member rather than read as a
/// filter that selects nothing.
#[tokio::test]
async fn refuses_a_filter_value_the_model_does_not_declare() {
    let fixture = fixture().await;
    let arn = policy_arn(0);

    for (member, value) in [
        ("EntityFilter", "role"),
        ("EntityFilter", "InstanceProfile"),
        ("EntityFilter", ""),
        ("PolicyUsageFilter", "permissionsPolicy"),
        ("PolicyUsageFilter", "Boundary"),
    ] {
        let case = format!("{member}={value:?}");
        let (status, body) = list(&fixture.router, &arn, &[(member, value)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the awsQuery validation code, got {:?}: {body}",
            error_code(&body)
        );
    }

    // Every value the enums DO declare is accepted.
    for (member, values) in [
        ("EntityFilter", ENTITY_FILTERS),
        ("PolicyUsageFilter", POLICY_USAGE_FILTERS),
    ] {
        for value in values.iter() {
            let (status, body) = list(&fixture.router, &arn, &[(member, value)]).await;
            assert_eq!(
                status, 200,
                "case {member}={value:?}: expected 200, got {status}: {body}"
            );
        }
    }
}
