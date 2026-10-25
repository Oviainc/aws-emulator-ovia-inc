// SPDX-License-Identifier: CC0-1.0

//! `ListManagedPoliciesInPermissionSet`, end to end over the `awsJson1_1` wire.
//!
//! THE RULE under test: the operation reports the AWS managed policies currently attached
//! to the permission set the request names ("Lists the Amazon Web Services managed policy
//! that is attached to a specified permission set") — each attachment exactly once, as an
//! `AttachedManagedPolicy` whose `Arn` is the attached ARN and whose `Name` is that ARN's
//! final `/`-separated segment, in ARN order, one page per call under the
//! `@paginated{inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults,
//! items: AttachedManagedPolicies}` contract, with a continuation token only while
//! attachments are left over. A permission set that carries no attachment is an EMPTY
//! page, while an `InstanceArn` or `PermissionSetArn` nothing is held under is a
//! `ResourceNotFoundException`, a `NextToken` this enumeration did not issue is
//! a `ValidationException`, and the whole permission-set surface stays
//! withdrawn on an account instance. Being a read, it changes nothing a later caller sees.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises
//! over: the NUMBER of attachments (none, one, several), the partition suffixes, nested
//! paths and name charset the model's `ManagedPolicyArn` pattern admits, its published
//! upper length bound (20..2048), the order the attachments were stored in, and every
//! `MaxResults` the model's `range(min: 1, max: 100)` admits around the collection size.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes, so the modelled constraints and the `awsJson1_1` envelope are exercised
//! rather than bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns, and the
//! calls run against an ORGANIZATION instance because the permission-set surface is
//! withdrawn on an account instance (asserted here too).
//!
//! The attachments themselves are seeded through the store, on the SAME record field the
//! attach operation writes: `AttachManagedPolicyToPermissionSet` is still a scaffold
//! (`issues/things-that-do-not-work/20261024T063249Z-sso-admin-attachmanagedpolicytopermissionset-stub.md`),
//! so until it is implemented no request can make an attachment. Everything this file then
//! asserts goes over the wire.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    account_instance_router, call, error_code, organization_engine, router_over, with_store,
    UNKNOWN_INSTANCE_ARN,
};

/// A pattern-valid `PermissionSetArn` that was never minted, for the not-found rows.
/// Shaped by the model's `PermissionSetArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso:::permissionSet/(sso)?ins-[a-zA-Z0-9-.]{16}/ps-[a-zA-Z0-9-./]{16}$`.
const UNKNOWN_PERMISSION_SET_ARN: &str =
    "arn:aws:sso:::permissionSet/ssoins-0000000000000000/ps-0000000000000000";

/// The fixed part of an AWS managed policy ARN, as the model's `ManagedPolicyArn` pattern
/// (`^arn:aws(-[a-z]{1,5}){0,3}:iam::aws:policy((/[A-Za-z0-9\.,\+@=_-]+)*)/([A-Za-z0-9\.,\+=@_-]+)$`)
/// requires: 24 characters, so the shortest value the pattern admits is longer than the
/// member's lower length bound of 20 and that bound is unreachable.
const MANAGED_POLICY_ARN_PREFIX: &str = "arn:aws:iam::aws:policy/";

/// `ManagedPolicyArn`'s published upper length bound (`length(min: 20, max: 2048)`).
const MANAGED_POLICY_ARN_MAX: usize = 2048;

/// An ARN of exactly [`MANAGED_POLICY_ARN_MAX`] characters: the bound is reached with a
/// long PATH, so the name segment stays inside `AttachedManagedPolicy$Name`'s own
/// `length(min: 1, max: 100)`.
fn max_length_managed_policy_arn() -> String {
    const NAME: &str = "/MaxLengthAccess";
    let path = "p".repeat(MANAGED_POLICY_ARN_MAX - MANAGED_POLICY_ARN_PREFIX.len() - NAME.len());
    format!("{MANAGED_POLICY_ARN_PREFIX}{path}{NAME}")
}

/// The managed policy ARNs the rows are built from: the forms the `ManagedPolicyArn`
/// pattern admits — a plain name, a nested path, a suffixed partition, the full name
/// charset `[A-Za-z0-9\.,\+=@_-]`, and the upper length bound.
fn policy_catalog() -> Vec<String> {
    vec![
        "arn:aws:iam::aws:policy/AdministratorAccess".to_string(),
        "arn:aws:iam::aws:policy/ReadOnlyAccess".to_string(),
        "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess".to_string(),
        "arn:aws-us-gov:iam::aws:policy/PowerUserAccess".to_string(),
        "arn:aws:iam::aws:policy/a.b,c+d=e@f_g-H9".to_string(),
        max_length_managed_policy_arn(),
    ]
}

/// The `Name` the rule derives for `arn`: its final `/`-separated segment, which the
/// `ManagedPolicyArn` pattern makes the policy's name.
fn expected_name(arn: &str) -> String {
    arn.rsplit_once('/')
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| arn.to_string())
}

/// A router over an organization instance, that instance's ARN, and the engine behind it.
fn organization_fixture() -> (MultiServiceRouter, String, Arc<engine::InMemoryEngine>) {
    let (engine, instance_arn) = organization_engine();
    (router_over(Arc::clone(&engine)), instance_arn, engine)
}

/// Create a permission set over the wire and return its minted ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
) -> String {
    let (status, created) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePermissionSet {name:?}: expected 200, got {status} {created}"
    );
    created["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreatePermissionSet {name:?} minted no ARN: {created}"))
        .to_string()
}

/// Seed `arns` as the attachments of `permission_set_arn`, in the order given.
///
/// The store is the attach operation's own channel (`PermissionSetRecord::managed_policies`);
/// see this module's header for why the seeding does not go over the wire.
fn seed_attachments(
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    permission_set_arn: &str,
    arns: &[String],
) {
    with_store(engine, |store| {
        let record = store
            .permission_set_mut(instance_arn, permission_set_arn)
            .unwrap_or_else(|| panic!("no permission set stored under {permission_set_arn}"));
        record.managed_policies = arns.to_vec();
    });
}

/// Walk the whole enumeration, following `NextToken`, and return every
/// `AttachedManagedPolicy` returned together with the number of pages it took.
async fn list_all(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    max_results: Option<i64>,
    case: &str,
) -> (Vec<Value>, usize) {
    let mut collected = Vec::new();
    let mut pages = 0usize;
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": permission_set_arn,
        });
        if let Some(max_results) = max_results {
            request["MaxResults"] = json!(max_results);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "ListManagedPoliciesInPermissionSet", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListManagedPoliciesInPermissionSet expected 200, got {status} {body}"
        );
        let page = body["AttachedManagedPolicies"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: AttachedManagedPolicies must be a list, got {body}")
            })
            .clone();
        if let Some(limit) = max_results {
            assert!(
                page.len() as i64 <= limit,
                "case {case:?}: a page must hold at most MaxResults {limit} items, got {}",
                page.len()
            );
        }
        pages += 1;
        collected.extend(page);
        match body["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
        assert!(
            pages <= 64,
            "case {case:?}: the enumeration must terminate, got {pages} pages"
        );
    }
    (collected, pages)
}

/// Assert `listed` reports exactly `expected` (a set of ARNs), each once, in ascending ARN
/// order, with `Name` derived from the ARN.
fn assert_reports_exactly(listed: &[Value], expected: &[String], case: &str) {
    let arns: Vec<String> = listed
        .iter()
        .map(|policy| {
            policy["Arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an entry carries no Arn: {policy}"))
                .to_string()
        })
        .collect();

    let mut expected_sorted = expected.to_vec();
    expected_sorted.sort();
    expected_sorted.dedup();
    assert_eq!(
        arns, expected_sorted,
        "case {case:?}: expected exactly {expected_sorted:?} in ARN order, got {arns:?}"
    );
    assert!(
        arns.windows(2).all(|pair| pair[0] < pair[1]),
        "case {case:?}: the enumeration must be strictly ascending by ARN, got {arns:?}"
    );
    for policy in listed {
        let arn = policy["Arn"].as_str().unwrap_or_default();
        let name = policy["Name"].as_str().unwrap_or_default();
        assert_eq!(
            name,
            expected_name(arn),
            "case {case:?}: Name must be the ARN's final segment for {arn:?}, got {name:?}"
        );
        assert!(
            !name.is_empty() && name.len() <= 100,
            "case {case:?}: Name must satisfy its 1..100 length for {arn:?}, got {name:?}"
        );
    }
}

/// THE RULE: every attachment the permission set holds is reported once, with its ARN and
/// the name that ARN carries — over a table spanning the collection sizes and the ARN forms
/// the model admits, and the order the attachments were stored in.
#[tokio::test]
async fn every_attachment_is_reported_once_with_its_arn_and_name() {
    let catalog = policy_catalog();
    let mut reversed = catalog.clone();
    reversed.reverse();
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("a permission set with no attachment", Vec::new()),
        ("one attachment", vec![catalog[0].clone()]),
        (
            "two attachments, stored in ARN order",
            vec![catalog[0].clone(), catalog[1].clone()],
        ),
        (
            "two attachments, stored in the reverse order",
            vec![catalog[1].clone(), catalog[0].clone()],
        ),
        ("a nested path", vec![catalog[2].clone()]),
        ("a suffixed partition", vec![catalog[3].clone()]),
        ("the full name charset", vec![catalog[4].clone()]),
        ("the 2048-character upper bound", vec![catalog[5].clone()]),
        ("every form at once", catalog.clone()),
        ("every form at once, stored reversed", reversed),
        (
            // The identity of an attachment is its ARN, so a permission set holds a given
            // managed policy at most once however often it was recorded.
            "the same policy recorded twice",
            vec![catalog[0].clone(), catalog[0].clone(), catalog[1].clone()],
        ),
    ];

    for (index, (label, attached)) in rows.iter().enumerate() {
        let (router, instance_arn, engine) = organization_fixture();
        let permission_set_arn =
            create_permission_set(&router, &instance_arn, &format!("Row{index}")).await;
        seed_attachments(&engine, &instance_arn, &permission_set_arn, attached);

        let (listed, pages) = list_all(&router, &instance_arn, &permission_set_arn, None, label)
            .await;
        assert_reports_exactly(&listed, attached, label);
        assert_eq!(
            pages, 1,
            "case {label:?}: {} attachments fit one default page, got {pages} pages",
            attached.len()
        );
    }
}

/// A permission set that carries no attachment answers an EMPTY page with no continuation
/// token — never a token-less error and never a page from another permission set.
#[tokio::test]
async fn a_permission_set_with_no_attachment_answers_an_empty_page() {
    let (router, instance_arn, engine) = organization_fixture();
    let empty = create_permission_set(&router, &instance_arn, "Empty").await;
    let filled = create_permission_set(&router, &instance_arn, "Filled").await;
    seed_attachments(&engine, &instance_arn, &filled, &policy_catalog());

    for max_results in [None, Some(1), Some(100)] {
        let case = format!("MaxResults {max_results:?}");
        let mut request = json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": empty,
        });
        if let Some(max_results) = max_results {
            request["MaxResults"] = json!(max_results);
        }
        let (status, body) = call(&router, "ListManagedPoliciesInPermissionSet", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["AttachedManagedPolicies"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a permission set with no attachment lists none, got {body}"
        );
        assert_eq!(
            body["NextToken"],
            Value::Null,
            "case {case:?}: with nothing left over there is no token, got {body}"
        );
    }
}

/// THE PAGINATION RULE: for ANY page size the model admits, following `NextToken` walks
/// exactly the stored attachments, once each, in the same ARN order, and the last page
/// carries no token.
#[tokio::test]
async fn pagination_walks_every_attachment_exactly_once_for_any_page_size() {
    let catalog = policy_catalog();
    // `MaxResults` is `range(min: 1, max: 100)`: both published bounds, every size around
    // the collection's own, and one past it.
    let page_sizes = [
        None,
        Some(1),
        Some(2),
        Some(3),
        Some(catalog.len() as i64 - 1),
        Some(catalog.len() as i64),
        Some(catalog.len() as i64 + 1),
        Some(100),
    ];

    for max_results in page_sizes {
        let case = format!("MaxResults {max_results:?}");
        let (router, instance_arn, engine) = organization_fixture();
        let permission_set_arn = create_permission_set(&router, &instance_arn, "Paged").await;
        seed_attachments(&engine, &instance_arn, &permission_set_arn, &catalog);

        let (listed, pages) =
            list_all(&router, &instance_arn, &permission_set_arn, max_results, &case).await;
        assert_reports_exactly(&listed, &catalog, &case);

        let expected_pages = match max_results {
            Some(limit) => {
                let limit = limit.clamp(1, 100) as usize;
                catalog.len().div_ceil(limit)
            }
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {} attachments at that page size take {expected_pages} pages, \
             got {pages}",
            catalog.len()
        );
    }
}

/// A resource nothing is held under is a `ResourceNotFoundException` — never an empty 200,
/// which would tell a client the permission set exists and carries nothing. The rows span
/// the unknown instance, the unknown permission set, a permission set of another instance
/// and one a `DeletePermissionSet` has removed.
///
/// The STATUS is the 400 this service answers every refusal of a request with, whatever
/// 4xx the error shape's `@httpError` declares (`crate::scaffolded::modeled_error_status`),
/// so the error CODE is what tells a missing resource from the other refusals.
#[tokio::test]
async fn an_unknown_instance_or_permission_set_is_not_found() {
    let (router, instance_arn, engine) = organization_fixture();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Present").await;
    seed_attachments(&engine, &instance_arn, &permission_set_arn, &policy_catalog());

    let deleted = create_permission_set(&router, &instance_arn, "Deleted").await;
    seed_attachments(&engine, &instance_arn, &deleted, &policy_catalog());
    let (status, body) = call(
        &router,
        "DeletePermissionSet",
        &json!({"InstanceArn": instance_arn, "PermissionSetArn": deleted}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeletePermissionSet expected 200, got {status} {body}"
    );

    let rows: Vec<(&str, String, String)> = vec![
        (
            "an InstanceArn no instance is held under",
            UNKNOWN_INSTANCE_ARN.to_string(),
            permission_set_arn.clone(),
        ),
        (
            "a PermissionSetArn that was never minted",
            instance_arn.clone(),
            UNKNOWN_PERMISSION_SET_ARN.to_string(),
        ),
        (
            "a permission set named under another instance",
            UNKNOWN_INSTANCE_ARN.to_string(),
            UNKNOWN_PERMISSION_SET_ARN.to_string(),
        ),
        (
            "a permission set a delete has removed",
            instance_arn.clone(),
            deleted,
        ),
    ];
    for (case, instance, permission_set) in rows {
        let (status, body) = call(
            &router,
            "ListManagedPoliciesInPermissionSet",
            &json!({"InstanceArn": instance, "PermissionSetArn": permission_set}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// A `NextToken` this ENUMERATION did not issue is refused `ValidationException` rather
/// than answered with an empty page, which would end a caller's walk early.
/// The rows span a value never issued, a token of another operation, a token of the SAME
/// operation on another permission set, and the empty token `Token`'s 0..2048 length admits.
#[tokio::test]
async fn a_token_this_enumeration_did_not_issue_is_refused() {
    let (router, instance_arn, engine) = organization_fixture();
    let catalog = policy_catalog();
    let first = create_permission_set(&router, &instance_arn, "First").await;
    let second = create_permission_set(&router, &instance_arn, "Second").await;
    seed_attachments(&engine, &instance_arn, &first, &catalog);
    seed_attachments(&engine, &instance_arn, &second, &catalog);

    // A real token of THIS operation, but issued for the other permission set.
    let (status, body) = call(
        &router,
        "ListManagedPoliciesInPermissionSet",
        &json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": second,
            "MaxResults": 1,
        }),
    )
    .await;
    assert_eq!(status, 200, "the seeding page expected 200, got {status} {body}");
    let other_scope_token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a truncated page must carry a token: {body}"))
        .to_string();

    // A real token of ANOTHER operation of this service: `ListPermissionSets` pages by
    // permission-set ARN, so its keys mean something else.
    let (status, body) = call(
        &router,
        "ListPermissionSets",
        &json!({"InstanceArn": instance_arn, "MaxResults": 1}),
    )
    .await;
    assert_eq!(status, 200, "ListPermissionSets expected 200, got {status} {body}");
    let other_operation_token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a truncated page must carry a token: {body}"))
        .to_string();

    let rows: Vec<(&str, String)> = vec![
        ("a value this service never issued", "not-a-token".to_string()),
        (
            "the empty token Token's 0..2048 length admits",
            String::new(),
        ),
        ("a token of another operation", other_operation_token),
        ("a token of another permission set", other_scope_token),
    ];
    for (case, token) in rows {
        let (status, body) = call(
            &router,
            "ListManagedPoliciesInPermissionSet",
            &json!({
                "InstanceArn": instance_arn,
                "PermissionSetArn": first,
                "NextToken": token,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

/// The permission-set surface stays WITHDRAWN on an account instance: the operation is
/// refused `ValidationException` (HTTP 400) there, which is the restriction and no longer
/// the scaffold's 501.
#[tokio::test]
async fn the_operation_is_withdrawn_on_an_account_instance() {
    let (router, instance_arn) = account_instance_router().await;

    for max_results in [None, Some(1), Some(100)] {
        let case = format!("MaxResults {max_results:?}");
        let mut request = json!({
            "InstanceArn": instance_arn,
            "PermissionSetArn": UNKNOWN_PERMISSION_SET_ARN,
        });
        if let Some(max_results) = max_results {
            request["MaxResults"] = json!(max_results);
        }
        let (status, body) = call(&router, "ListManagedPoliciesInPermissionSet", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 on an account instance, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

/// The operation is a READ: repeating it answers the same thing, and it leaves the stored
/// attachments exactly as they were, so nothing a later caller sees depends on who listed.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let (router, instance_arn, engine) = organization_fixture();
    let catalog = policy_catalog();
    let permission_set_arn = create_permission_set(&router, &instance_arn, "Stable").await;
    seed_attachments(&engine, &instance_arn, &permission_set_arn, &catalog);

    let request = json!({
        "InstanceArn": instance_arn,
        "PermissionSetArn": permission_set_arn,
        "MaxResults": 2,
    });
    let (status, first) = call(&router, "ListManagedPoliciesInPermissionSet", &request).await;
    assert_eq!(status, 200, "the first read expected 200, got {status} {first}");

    // Walk the whole enumeration in between, so any state a page consumed would show.
    let (listed, _) = list_all(&router, &instance_arn, &permission_set_arn, Some(1), "walked").await;
    assert_reports_exactly(&listed, &catalog, "walked");

    let (status, again) = call(&router, "ListManagedPoliciesInPermissionSet", &request).await;
    assert_eq!(status, 200, "the repeated read expected 200, got {status} {again}");
    assert_eq!(
        first, again,
        "a read must answer the same twice, got {first} then {again}"
    );

    let stored = with_store(&engine, |store| {
        store
            .permission_set(&instance_arn, &permission_set_arn)
            .map(|record| record.managed_policies.clone())
            .unwrap_or_default()
    });
    assert_eq!(
        stored, catalog,
        "a read must leave the stored attachments untouched, got {stored:?}"
    );
}
