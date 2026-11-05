// SPDX-License-Identifier: CC0-1.0

//! `ListPolicies`, end to end over the awsQuery wire, cross-read through `GetPolicy`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is
//! the HTTP behaviour (status, awsQuery error code, XML members) of policies that were
//! PERSISTED by `CreatePolicy` — or that AWS owns — never a handler called directly.
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share.
//!
//! The rule being asserted: a managed-policy listing enumerates the policies an
//! account can see — the ones it holds and the ones AWS owns — reports for each the
//! same record its own `GetPolicy` read reports minus the attributes the listing
//! withholds, selects within that one enumeration order by `Scope`, `PathPrefix`,
//! `OnlyAttached` and `PolicyUsageFilter`, and pages over it with `MaxItems` and a
//! resumable `Marker`.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! policy's owner (the account or AWS), the path depth, the name's case and alphabet,
//! the page size, the filter value, the number of attachments and boundary uses, and
//! the tag count. No single request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s own
/// documentation describes. None of these assertions is about the document itself.
const DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy with a single statement — every role an attachment case needs one
/// to exist at all.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"glue.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The fixture policy set: names spanning the whole `^[\w+=,.@-]+$` alphabet
/// (`policyNameType`) and both cases, and paths spanning the account root, a
/// one-segment path and a nested path, so a rule about paths is exercised at more than
/// one depth.
const POLICIES: &[(&str, &str)] = &[
    ("alpha", "/"),
    ("Beta", "/eng/"),
    ("gamma_7", "/eng/tools/"),
    ("delta.x", "/sales/"),
    ("Epsilon+1", "/eng/"),
    ("zeta=2", "/"),
];

/// The ARN a fixture policy is created under: `arn:aws:iam::<account>:policy<path><name>`
/// — IAM is global, so the region segment is EMPTY.
fn local_arn(path: &str, policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy{path}{policy_name}", account_id())
}

/// The fixture policies' ARNs, ascending — the enumeration order a listing of the
/// account's own policies walks.
fn local_arns() -> Vec<String> {
    let mut arns: Vec<String> = POLICIES
        .iter()
        .map(|(name, path)| local_arn(path, name))
        .collect();
    arns.sort();
    arns
}

/// Every AWS-managed policy this emulator carries, ascending by ARN.
///
/// SOURCE: `state::AWS_MANAGED_POLICIES`, the catalog the service itself serves an
/// `arn:aws:iam::aws:policy/` read from, so the listing is asserted over EVERY entry
/// rather than the handful a fixture might cite.
fn aws_arns() -> Vec<String> {
    let mut arns: Vec<String> = state::AWS_MANAGED_POLICIES
        .iter()
        .map(|entry| entry.arn())
        .collect();
    arns.sort();
    arns
}

/// Every policy the account can see, ascending by ARN: its own and AWS's.
fn all_arns() -> Vec<String> {
    let mut arns = local_arns();
    arns.extend(aws_arns());
    arns.sort();
    arns
}

/// Create one customer managed policy, asserting the fixture create succeeded.
async fn create_policy(router: &MultiServiceRouter, policy_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", policy_name),
            ("Path", path),
            ("PolicyDocument", DOCUMENT),
            ("Description", "a fixture policy"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: the fixture CreatePolicy must succeed, got {status}: {body}"
    );
}

/// Create every fixture policy, answering the router they were created in.
async fn with_fixture_policies() -> MultiServiceRouter {
    let router = router();
    for (name, path) in POLICIES {
        create_policy(&router, name, path).await;
    }
    router
}

/// The ARNs a `ListPolicies` response reports, in document order.
fn listed_arns(body: &str) -> Vec<String> {
    elements(body, "Arn")
}

/// One row of a selector table: what the row is called, the request parameters it
/// adds, and the ARNs the listing must report for it.
struct FilterCase {
    name: &'static str,
    params: Vec<(String, String)>,
    expected: Vec<String>,
}

/// One `ListPolicies` call with the row's parameters.
async fn list_policies(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, "ListPolicies", &borrowed(params)).await
}

/// Every ARN a listing reports across ALL its pages, following the `Marker` the
/// service hands out, asserting each page is a 200 and the pagination terminates.
async fn list_every_page(
    router: &MultiServiceRouter,
    params: &[(String, String)],
    case: &str,
) -> Vec<String> {
    let mut seen = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut page_params = params.to_vec();
        if let Some(token) = &marker {
            page_params.push(param("Marker", token));
        }
        let (status, body) = list_policies(router, &page_params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        seen.extend(listed_arns(&body));
        pages += 1;
        assert!(
            pages <= all_arns().len() + 1,
            "case {case:?}: the pagination never ended, saw {seen:?}"
        );
        if element(&body, "IsTruncated") == "true" {
            marker = Some(element(&body, "Marker"));
        } else {
            assert_eq!(
                elements(&body, "Marker"),
                Vec::<String>::new(),
                "case {case:?}: the last page must carry no Marker, got {:?}",
                elements(&body, "Marker")
            );
            break;
        }
    }
    seen
}

/// One policy as a `ListPolicies` page reports it, read off the members every
/// reported policy carries exactly once — so position identifies the policy.
#[derive(Debug, PartialEq, Eq)]
struct ListedPolicy {
    policy_name: String,
    policy_id: String,
    arn: String,
    path: String,
    default_version_id: String,
    attachment_count: String,
    permissions_boundary_usage_count: String,
    is_attachable: String,
    create_date: String,
    update_date: String,
}

/// The policies a `ListPolicies` (or `GetPolicy`) response reports, in document order.
fn listed_policies(body: &str) -> Vec<ListedPolicy> {
    let arns = elements(body, "Arn");
    let names = elements(body, "PolicyName");
    let ids = elements(body, "PolicyId");
    let paths = elements(body, "Path");
    let versions = elements(body, "DefaultVersionId");
    let attachments = elements(body, "AttachmentCount");
    let boundaries = elements(body, "PermissionsBoundaryUsageCount");
    let attachable = elements(body, "IsAttachable");
    let created = elements(body, "CreateDate");
    let updated = elements(body, "UpdateDate");
    for (label, found) in [
        ("PolicyName", names.len()),
        ("PolicyId", ids.len()),
        ("Path", paths.len()),
        ("DefaultVersionId", versions.len()),
        ("AttachmentCount", attachments.len()),
        ("PermissionsBoundaryUsageCount", boundaries.len()),
        ("IsAttachable", attachable.len()),
        ("CreateDate", created.len()),
        ("UpdateDate", updated.len()),
    ] {
        assert_eq!(
            found,
            arns.len(),
            "every reported policy must carry one <{label}>: got {found} for {} policies: {body}",
            arns.len()
        );
    }
    (0..arns.len())
        .map(|index| ListedPolicy {
            policy_name: names[index].clone(),
            policy_id: ids[index].clone(),
            arn: arns[index].clone(),
            path: paths[index].clone(),
            default_version_id: versions[index].clone(),
            attachment_count: attachments[index].clone(),
            permissions_boundary_usage_count: boundaries[index].clone(),
            is_attachable: attachable[index].clone(),
            create_date: created[index].clone(),
            update_date: updated[index].clone(),
        })
        .collect()
}

/// RULE: a resource-listing operation enumerates the resources that exist, reporting
/// for each the record a single-resource read of it returns, never a value recomputed
/// from the listing request.
#[tokio::test]
async fn list_policies_reports_the_record_every_policy_read_returns() {
    let router = with_fixture_policies().await;

    // Rows: the account's own policies, AWS's catalog, and both together — the three
    // scopes a policy's owner can put it in.
    for (scope, expected) in [
        ("Local", local_arns()),
        ("AWS", aws_arns()),
        ("All", all_arns()),
    ] {
        let params = vec![param("Scope", scope), param("MaxItems", "1000")];
        let (status, body) = list_policies(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {scope:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            listed_arns(&body),
            expected,
            "case {scope:?}: expected {expected:?}, got {:?}",
            listed_arns(&body)
        );

        // Each row of this inner table is one reported policy: what the listing says
        // about it must be what its own `GetPolicy` read says.
        for listed in listed_policies(&body) {
            let case = listed.arn.clone();
            let (status, fetched) =
                call(&router, "GetPolicy", &[("PolicyArn", case.as_str())]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetPolicy must find the policy, got {status}: {fetched}"
            );
            let stored = listed_policies(&fetched);
            assert_eq!(
                stored.len(),
                1,
                "case {case:?}: expected GetPolicy to report one policy, got {stored:?}"
            );
            assert_eq!(
                listed, stored[0],
                "case {case:?}: expected the listing to report the stored record, got {listed:?}"
            );
            assert!(
                case.ends_with(&format!("policy{}{}", listed.path, listed.policy_name)),
                "case {case:?}: expected the ARN to spell the policy's path and name, got {listed:?}"
            );
        }
    }
}

/// RULE: a listing of an empty collection is an empty list with `IsTruncated` false
/// and no `Marker`, not an error — and a scope holding nothing is empty even while
/// another scope holds policies.
#[tokio::test]
async fn a_scope_holding_no_policy_is_an_empty_list() {
    let router = router();
    // Rows span the parameters a caller may add to a listing that matches nothing: an
    // account that has created no policy, and a path prefix no policy sits under.
    let cases: &[&[(&str, &str)]] = &[
        &[("Scope", "Local")],
        &[("Scope", "Local"), ("MaxItems", "1")],
        &[("Scope", "Local"), ("MaxItems", "1000")],
        &[("Scope", "Local"), ("PathPrefix", "/")],
        &[("Scope", "All"), ("PathPrefix", "/nobody/")],
        &[("PathPrefix", "/nobody/"), ("MaxItems", "5")],
        &[("OnlyAttached", "true")],
    ];
    for params in cases {
        let (status, body) = call(&router, "ListPolicies", params).await;
        assert_eq!(
            status, 200,
            "case {params:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            listed_arns(&body),
            Vec::<String>::new(),
            "case {params:?}: expected no policies, got {:?}",
            listed_arns(&body)
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {params:?}: expected IsTruncated false, got {:?}",
            element(&body, "IsTruncated")
        );
        assert_eq!(
            elements(&body, "Marker"),
            Vec::<String>::new(),
            "case {params:?}: an untruncated page carries no Marker, got {:?}",
            elements(&body, "Marker")
        );
    }
}

/// RULE: `Scope` selects WHOSE managed policies the listing reports — the account's
/// own, AWS's, or both — and an absent `Scope` means both, exactly as `All` does.
#[tokio::test]
async fn list_policies_filters_by_scope() {
    let router = with_fixture_policies().await;

    // Rows: one per value `policyScopeType` declares
    // (`smithy_query(query="shape", shape="policyScopeType")` — AWS, All, Local), plus
    // the absent parameter.
    let cases = vec![
        FilterCase {
            name: "absent",
            params: vec![],
            expected: all_arns(),
        },
        FilterCase {
            name: "All",
            params: vec![param("Scope", "All")],
            expected: all_arns(),
        },
        FilterCase {
            name: "Local",
            params: vec![param("Scope", "Local")],
            expected: local_arns(),
        },
        FilterCase {
            name: "AWS",
            params: vec![param("Scope", "AWS")],
            expected: aws_arns(),
        },
    ];

    for FilterCase {
        name: case,
        params,
        expected,
    } in cases
    {
        let seen = list_every_page(&router, &params, case).await;
        assert_eq!(
            seen, expected,
            "case {case:?}: expected {expected:?}, got {seen:?}"
        );
    }

    // Every AWS-managed policy the emulator serves by ARN is also reported by the
    // listing: the two readers resolve the same catalog, so neither can hold a policy
    // the other does not.
    let aws = list_every_page(&router, &[param("Scope", "AWS")], "AWS").await;
    for entry in state::AWS_MANAGED_POLICIES {
        let arn = entry.arn();
        assert!(
            aws.contains(&arn),
            "case {:?}: expected the AWS-managed listing to report {arn:?}, got {aws:?}",
            entry.policy_name
        );
        let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", arn.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {arn:?}: GetPolicy must serve the listed policy, got {status}: {body}"
        );
    }
}

/// RULE: `PathPrefix` selects exactly the policies whose path begins with it, and an
/// absent `PathPrefix` "defaults to a slash (/), listing all policies".
#[tokio::test]
async fn list_policies_filters_by_path_prefix() {
    let router = with_fixture_policies().await;

    // Rows span the prefix depth: the root, a one-segment prefix, a nested prefix, a
    // prefix selecting exactly one policy, and one selecting none.
    let cases: &[&str] = &[
        "/",
        "/eng/",
        "/eng/tools/",
        "/sales/",
        "/service-role/",
        "/nobody/",
    ];

    for prefix in cases {
        // The expectation is derived from the prefix itself, so the row says what the
        // rule says rather than repeating a hand-copied answer.
        let expected: Vec<String> = all_arns()
            .into_iter()
            .filter(|arn| {
                let body = arn
                    .split_once(":policy")
                    .map(|(_, path_and_name)| path_and_name)
                    .expect("a policy ARN");
                body.starts_with(prefix)
            })
            .collect();
        let params = vec![param("PathPrefix", prefix)];
        let seen = list_every_page(&router, &params, prefix).await;
        assert_eq!(
            seen, expected,
            "case {prefix:?}: expected {expected:?}, got {seen:?}"
        );
    }

    // `/service-role/` must select something: AWS files part of its catalog there, so
    // the nested-prefix row is not vacuous.
    let service_role = list_every_page(
        &router,
        &[param("PathPrefix", "/service-role/")],
        "/service-role/",
    )
    .await;
    assert!(
        !service_role.is_empty(),
        "expected AWS's /service-role/ policies to be listed, got {service_role:?}"
    );

    // The default prefix lists every policy, exactly as "/" does.
    let defaulted = list_every_page(&router, &[], "absent").await;
    let rooted = list_every_page(&router, &[param("PathPrefix", "/")], "/").await;
    assert_eq!(
        defaulted, rooted,
        "expected an absent PathPrefix to default to \"/\", got {defaulted:?}"
    );
}

/// RULE: a list operation enumerates the collection in one total order, honours
/// `MaxItems` as a real page size, and hands out a `Marker` a follow-up call resumes
/// from without repeating or skipping an entry.
#[tokio::test]
async fn list_policies_pages_through_every_policy_exactly_once() {
    let router = with_fixture_policies().await;
    let expected = all_arns();

    // Rows span the page size: the published minimum, sizes that do and do not divide
    // the set, a size equal to the set, the published maximum, and the absent-parameter
    // default. `maxItemsType` is range 1..1000
    // (`smithy_query(query="operation", operation="ListPolicies")`).
    let page_sizes: &[Option<i32>] = &[
        Some(1),
        Some(2),
        Some(5),
        Some(expected.len() as i32),
        Some(1000),
        None,
    ];

    for page_size in page_sizes {
        let case = format!("MaxItems={page_size:?}");
        let mut params = Vec::new();
        if let Some(size) = page_size {
            params.push(param("MaxItems", &size.to_string()));
        }

        // Each page honours the size, which `list_every_page` cannot see: walk the
        // pages here so the per-page bound is asserted too.
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let mut page_params = params.clone();
            if let Some(token) = &marker {
                page_params.push(param("Marker", token));
            }
            let (status, body) = list_policies(&router, &page_params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {body}"
            );
            let page = listed_arns(&body);
            if let Some(size) = page_size {
                assert!(
                    page.len() <= *size as usize,
                    "case {case:?}: expected at most {size} policies on a page, got {}",
                    page.len()
                );
            }
            seen.extend(page);
            assert!(
                seen.len() <= expected.len(),
                "case {case:?}: the pagination repeated an entry, saw {seen:?}"
            );
            if element(&body, "IsTruncated") == "true" {
                marker = Some(element(&body, "Marker"));
            } else {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every policy exactly once in ARN order, got {seen:?}"
        );
    }
}

/// RULE: `MaxItems` outside its published range is refused before the operation runs,
/// so a page size the model forbids never reaches the store.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = with_fixture_policies().await;
    // One past each published bound of `maxItemsType` (range 1..1000).
    for out_of_range in ["0", "1001", "-1"] {
        let (status, body) = call(&router, "ListPolicies", &[("MaxItems", out_of_range)]).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400, got {status}: {body}"
        );
    }
    // And each bound itself is accepted.
    for in_range in ["1", "1000"] {
        let (status, body) = call(&router, "ListPolicies", &[("MaxItems", in_range)]).await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected 200, got {status}: {body}"
        );
    }
}

/// RULE: a `Marker` this service did not issue for this collection names no position
/// in it, so the call is refused rather than silently answered from the beginning.
#[tokio::test]
async fn a_marker_this_collection_did_not_issue_is_refused() {
    let router = with_fixture_policies().await;

    // Rows: a token that is not base64 at all, and two that decode to bytes carrying
    // none of this service's marker structure.
    for marker in ["not-a-marker", "zz", "0g"] {
        let (status, body) = call(&router, "ListPolicies", &[("Marker", marker)]).await;
        assert_eq!(
            status, 400,
            "case {marker:?}: expected a foreign marker to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {marker:?}: expected the marker to be refused as invalid, got {:?}",
            error_code(&body)
        );
    }

    // A marker issued while paging another collection is not a position in this one.
    let (status, body) = call(&router, "CreateUser", &[("UserName", "paging-user")]).await;
    assert_eq!(status, 200, "the fixture CreateUser must succeed: {body}");
    let (status, body) = call(&router, "ListUsers", &[("MaxItems", "1")]).await;
    assert_eq!(status, 200, "expected a user page, got {status}: {body}");
    let (status, users_page) = call(&router, "ListPolicies", &[("MaxItems", "1")]).await;
    assert_eq!(
        status, 200,
        "expected a truncated policy page, got {status}: {users_page}"
    );
    let policy_marker = element(&users_page, "Marker");
    let (status, body) = call(&router, "ListUsers", &[("Marker", policy_marker.as_str())]).await;
    assert_eq!(
        status, 400,
        "expected a policy marker to be refused by ListUsers, got {status}: {body}"
    );

    // A marker names a POSITION in the order: resuming from it yields exactly the
    // policies that follow the one the page was cut at.
    let first = element(&users_page, "Arn");
    let expected: Vec<String> = all_arns().into_iter().filter(|arn| *arn > first).collect();
    let seen = list_every_page(&router, &[param("Marker", &policy_marker)], "resumed").await;
    assert_eq!(
        seen, expected,
        "expected the order after the marker's position, got {seen:?}"
    );
}

/// The policies attached to an identity, and the one used as an identity's permissions
/// boundary, as the fixture arranges them.
struct Usage {
    /// The ARNs attached to a user or a role — the policies in use as permissions
    /// POLICIES.
    attached: Vec<String>,
    /// The ARNs set as a user's or a role's permissions boundary.
    bounding: Vec<String>,
}

/// Arrange the fixture policies into uses: one attached to a role, one attached to a
/// user, an AWS-managed policy attached to the role, and one set as the user's
/// permissions boundary — so every row of the usage and attachment tables has both a
/// matching and a non-matching policy.
async fn with_fixture_usage() -> (MultiServiceRouter, Usage) {
    let router = with_fixture_policies().await;
    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "listing-role"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(status, 200, "the fixture CreateRole must succeed: {body}");
    let (status, body) = call(&router, "CreateUser", &[("UserName", "listing-user")]).await;
    assert_eq!(status, 200, "the fixture CreateUser must succeed: {body}");

    let role_policy = local_arn("/", "alpha");
    let user_policy = local_arn("/eng/", "Beta");
    let aws_policy = local_boundary_free_aws_arn();
    let boundary = local_arn("/sales/", "delta.x");

    for (action, name_param, name, arn) in [
        (
            "AttachRolePolicy",
            "RoleName",
            "listing-role",
            role_policy.clone(),
        ),
        (
            "AttachRolePolicy",
            "RoleName",
            "listing-role",
            aws_policy.clone(),
        ),
        (
            "AttachUserPolicy",
            "UserName",
            "listing-user",
            user_policy.clone(),
        ),
    ] {
        let (status, body) = call(
            &router,
            action,
            &[(name_param, name), ("PolicyArn", arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {arn:?}: the fixture {action} must succeed, got {status}: {body}"
        );
    }

    let (status, body) = call(
        &router,
        "PutUserPermissionsBoundary",
        &[
            ("UserName", "listing-user"),
            ("PermissionsBoundary", boundary.as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture PutUserPermissionsBoundary must succeed, got {status}: {body}"
    );

    let mut attached = vec![role_policy, user_policy, aws_policy];
    attached.sort();
    (
        router,
        Usage {
            attached,
            bounding: vec![boundary],
        },
    )
}

/// The ARN of an AWS-managed policy a role may be attached to: the first catalog entry
/// at the account root, which is not under the path AWS reserves for service-linked
/// roles.
///
/// SOURCE: `state::AWS_MANAGED_POLICIES`, so the row names a catalog entry rather than
/// a hand-copied ARN.
fn local_boundary_free_aws_arn() -> String {
    state::AWS_MANAGED_POLICIES
        .iter()
        .find(|entry| entry.path == "/")
        .expect("the catalog carries a policy at the account root")
        .arn()
}

/// RULE: `OnlyAttached` true reports only the policies attached to an identity —
/// "when OnlyAttached is false, or when the parameter is not included, all policies
/// are returned".
#[tokio::test]
async fn only_attached_reports_only_the_policies_an_identity_carries() {
    let (router, usage) = with_fixture_usage().await;

    // Rows: the flag absent, explicitly false, and true — the three forms a request
    // takes for a member the model defaults to false.
    let cases = vec![
        FilterCase {
            name: "absent",
            params: vec![],
            expected: all_arns(),
        },
        FilterCase {
            name: "false",
            params: vec![param("OnlyAttached", "false")],
            expected: all_arns(),
        },
        FilterCase {
            name: "true",
            params: vec![param("OnlyAttached", "true")],
            expected: usage.attached.clone(),
        },
    ];

    for FilterCase {
        name: case,
        params,
        expected,
    } in cases
    {
        let seen = list_every_page(&router, &params, case).await;
        assert_eq!(
            seen, expected,
            "case {case:?}: expected {expected:?}, got {seen:?}"
        );
    }

    // `OnlyAttached` composes with the other selectors: it narrows within the same
    // order rather than replacing it.
    let local_attached: Vec<String> = usage
        .attached
        .iter()
        .filter(|arn| arn.contains(account_id()))
        .cloned()
        .collect();
    let seen = list_every_page(
        &router,
        &[param("OnlyAttached", "true"), param("Scope", "Local")],
        "attached and local",
    )
    .await;
    assert_eq!(
        seen, local_attached,
        "case \"attached and local\": expected {local_attached:?}, got {seen:?}"
    );
}

/// RULE: `PolicyUsageFilter` selects WHICH USE of a policy the listing is about — a
/// policy attached to an identity is in use as a permissions policy, one an identity
/// carries as its boundary is in use to set a permissions boundary — and an absent
/// filter reports every policy whatever it is used for.
#[tokio::test]
async fn the_usage_filter_separates_attachment_from_permissions_boundary() {
    let (router, usage) = with_fixture_usage().await;

    // Rows: one per value `PolicyUsageType` declares
    // (`smithy_query(query="shape", shape="PolicyUsageType")` — PermissionsBoundary,
    // PermissionsPolicy), plus the absent parameter.
    let cases = vec![
        FilterCase {
            name: "absent",
            params: vec![],
            expected: all_arns(),
        },
        FilterCase {
            name: "PermissionsPolicy",
            params: vec![param("PolicyUsageFilter", "PermissionsPolicy")],
            expected: usage.attached.clone(),
        },
        FilterCase {
            name: "PermissionsBoundary",
            params: vec![param("PolicyUsageFilter", "PermissionsBoundary")],
            expected: usage.bounding.clone(),
        },
    ];

    for FilterCase {
        name: case,
        params,
        expected,
    } in cases
    {
        let seen = list_every_page(&router, &params, case).await;
        assert_eq!(
            seen, expected,
            "case {case:?}: expected {expected:?}, got {seen:?}"
        );
    }

    // A value the enum does not declare is refused before the operation runs.
    for rejected in ["permissionsPolicy", "Boundary", ""] {
        let (status, body) =
            call(&router, "ListPolicies", &[("PolicyUsageFilter", rejected)]).await;
        assert_eq!(
            status, 400,
            "case {rejected:?}: expected 400, got {status}: {body}"
        );
    }
}

/// One row of the withheld-attribute table: a policy carrying the tags and the
/// description the row names.
struct SubsetCase {
    name: &'static str,
    tags: Vec<(String, String)>,
    description: Option<&'static str>,
}

/// RULE: an IAM resource-listing operation reports a SUBSET of the resource's
/// attributes — the ones the documentation names are withheld from the listing and
/// available only from the single-resource read — while every other stored attribute
/// is reported.
///
/// `ListPolicies`: "this operation does not return tags, even though they are an
/// attribute of the returned object. To view all of the information for a customer
/// manged policy, see GetPolicy." And `Policy$Description`
/// (`smithy_query(query="doc", shape="Policy", member="Description")`): "this element
/// is included in the response to the GetPolicy operation. It is not included in the
/// response to the ListPolicies operation."
#[tokio::test]
async fn a_listing_withholds_the_attributes_its_documentation_excludes() {
    // Rows span the tag count (none, one, several, the `tagListType` bound of 50) and
    // whether the policy carries a `Description`, so the rule is asserted on a policy
    // that HAS something to withhold and on one that does not.
    let fifty: Vec<(String, String)> = (0..50)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();
    let cases = vec![
        SubsetCase {
            name: "bare",
            tags: vec![],
            description: None,
        },
        SubsetCase {
            name: "described",
            tags: vec![],
            description: Some("a described policy"),
        },
        SubsetCase {
            name: "one-tag",
            tags: vec![("env".to_string(), "prod".to_string())],
            description: Some("a tagged and described policy"),
        },
        SubsetCase {
            name: "three-tags",
            tags: vec![
                ("env".to_string(), "dev".to_string()),
                ("team".to_string(), "eng".to_string()),
                ("cost".to_string(), "42".to_string()),
            ],
            description: None,
        },
        SubsetCase {
            name: "fifty-tags",
            tags: fifty,
            description: Some("a heavily tagged policy"),
        },
    ];

    for SubsetCase {
        name: case,
        tags,
        description,
    } in cases
    {
        let router = router();
        let policy_name = format!("policy_{}", case.replace('-', "_"));
        let mut params = vec![
            param("PolicyName", &policy_name),
            param("PolicyDocument", DOCUMENT),
        ];
        if let Some(description) = description {
            params.push(param("Description", description));
        }
        for (position, (key, value)) in tags.iter().enumerate() {
            params.push(param(&format!("Tags.member.{}.Key", position + 1), key));
            params.push(param(&format!("Tags.member.{}.Value", position + 1), value));
        }
        let (status, body) = call(&router, "CreatePolicy", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreatePolicy must succeed, got {status}: {body}"
        );
        let arn = element(&body, "Arn");

        // `GetPolicy` is the read that reports everything, including the tags and the
        // description.
        let (status, fetched) = call(&router, "GetPolicy", &[("PolicyArn", arn.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPolicy must find the policy, got {status}: {fetched}"
        );
        let expected_tags: std::collections::BTreeMap<String, String> =
            tags.iter().cloned().collect();
        assert_eq!(
            reported_tags(&fetched),
            expected_tags,
            "case {case:?}: expected GetPolicy to report the stored tags, got {:?}",
            reported_tags(&fetched)
        );
        assert_eq!(
            elements(&fetched, "Description"),
            description.map(str::to_string).into_iter().collect::<Vec<_>>(),
            "case {case:?}: expected GetPolicy to report the stored description, got {:?}",
            elements(&fetched, "Description")
        );

        let (status, listed) = call(&router, "ListPolicies", &[("Scope", "Local")]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {listed}"
        );
        assert_eq!(
            listed_arns(&listed),
            vec![arn.clone()],
            "case {case:?}: expected the one persisted policy, got {:?}",
            listed_arns(&listed)
        );
        // Asserted on the XML ELEMENT each withheld member serializes to, so an
        // unrelated member whose name merely contains the word cannot satisfy or break
        // the check.
        for withheld in ["<Tags>", "<Description>"] {
            assert!(
                !listed.contains(withheld),
                "case {case:?}: the listing must withhold {withheld}, got {listed}"
            );
        }
        if !tags.is_empty() {
            assert!(
                fetched.contains("<Tags>"),
                "case {case:?}: GetPolicy is the read that reports the tags, got {fetched}"
            );
        }
        if description.is_some() {
            assert!(
                fetched.contains("<Description>"),
                "case {case:?}: GetPolicy is the read that reports the description, got {fetched}"
            );
        }

        // Every other stored attribute IS reported.
        assert_eq!(
            listed_policies(&listed)[0],
            listed_policies(&fetched)[0],
            "case {case:?}: expected the listing to report the stored record"
        );
    }
}

/// RULE: a listing is a REPORT — it changes nothing a later caller sees, however many
/// times and however it is paged or filtered.
#[tokio::test]
async fn listing_policies_changes_nothing_a_later_caller_sees() {
    let (router, _usage) = with_fixture_usage().await;
    let (_, before) = call(&router, "ListPolicies", &[("MaxItems", "1000")]).await;
    let baseline = listed_policies(&before);

    for params in [
        vec![param("MaxItems", "1")],
        vec![param("Scope", "Local")],
        vec![param("Scope", "AWS"), param("MaxItems", "3")],
        vec![param("PathPrefix", "/eng/")],
        vec![param("PathPrefix", "/nobody/")],
        vec![param("OnlyAttached", "true")],
        vec![param("PolicyUsageFilter", "PermissionsBoundary")],
    ] {
        let (status, body) = list_policies(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {params:?}: expected 200, got {status}: {body}"
        );
    }

    let (_, after) = call(&router, "ListPolicies", &[("MaxItems", "1000")]).await;
    assert_eq!(
        listed_policies(&after),
        baseline,
        "expected the repeated listings to leave the store alone"
    );
}
