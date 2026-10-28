// SPDX-License-Identifier: CC0-1.0

//! `DeleteRolePermissionsBoundary` end to end over the awsQuery wire: the write that
//! clears the boundary `PutRolePermissionsBoundary` set, and the two refusals the
//! operation models for a name it cannot serve.
//!
//! RULE under test: the call answers from the account's own role records — it clears the
//! boundary of a role the account holds and answers the empty HTTP 200 its modelled
//! `Unit` output is, which `GetRole` then confirms; a name no role is held under is
//! `NoSuchEntity` (HTTP 404), the refusal naming the role that is missing; a role that
//! carries no boundary has no such resource to delete, so that call is refused
//! `NoSuchEntity` too while the role stays reported; and a role filed under the reserved
//! service-linked role path is a protected resource only the service it is linked to may
//! change, so the call is refused `UnmodifiableEntity` (HTTP 400) in the one sentence
//! this service words that protection with — decided BEFORE anything about the role's
//! boundary, which such a role never carries.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! role's path depth, the case the request spells its name in and the alphabet that name
//! uses, whether the boundary names an AWS-managed or a customer-managed policy and the
//! path that policy is filed under, how many set-and-clear cycles the role has seen, the
//! form a name misses a held role by, the service a protected role is linked to and
//! whether its name carries a custom suffix, and whether that protected role is one this
//! account created or one its owning service keeps provisioned under the name AWS
//! publishes. No single request is the specification.
//!
//! The awsQuery harness (`router`, `call`, `element`, `elements`, `error_code`) is the one
//! `tests/all/user_lifecycle.rs` owns; the boundary fixtures (`create_role`,
//! `create_policy`, `put_boundary`, `reported_boundary`) and the protected-role fixtures
//! (`service_linked_role`, `protected`, `LINKED_SERVICES`) are the ones the put suite and
//! the protected-change suite beside this one already own.

use smithy_server_runtime::MultiServiceRouter;

use crate::protected_role_refusals::{protected, service_linked_role, LINKED_SERVICES};
use crate::role_permissions_boundary::{
    create_policy, create_role, put_boundary, reported_boundary, AWS_MANAGED_POLICIES,
    PERMISSIONS_BOUNDARY_TYPE,
};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// One `DeleteRolePermissionsBoundary` call: the only member the operation takes is
/// `RoleName` (`smithy_query(query="operation",
/// operation="DeleteRolePermissionsBoundary")`).
async fn delete_boundary(router: &MultiServiceRouter, role_name: &str) -> (u16, String) {
    call(
        router,
        "DeleteRolePermissionsBoundary",
        &[("RoleName", role_name)],
    )
    .await
}

/// The `Arn` `GetRole` reports for `role_name` — what shows the role ITSELF survived a
/// call that only cleared its boundary.
async fn reported_role_arn(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = call(router, "GetRole", &[("RoleName", role_name)]).await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: GetRole must report the role, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// RULE: the call clears the one boundary the named role carries and answers the empty
/// 200 its modelled `Unit` output is, so a later `GetRole` reports no boundary at all
/// while the role itself is untouched — whatever the role's path, whatever case the
/// request spells its name in, and whichever managed policy the boundary named.
#[tokio::test]
async fn clearing_a_boundary_is_served_and_get_role_then_reports_none() {
    let router = router();

    // The customer-managed policies the rows below bound a role with, created first so
    // the ARNs name policies this account actually holds.
    let customer_root = create_policy(&router, "cleared-at-root", "/").await;
    let customer_nested = create_policy(&router, "Cleared_2", "/boundaries/eng/").await;

    // Rows: (role name as created, role path, the name the DELETE spells, boundary ARN).
    // They differ along the role's path depth, the case the delete spells the name in
    // (IAM entity names "are not distinguished by case"), the name alphabet, and the
    // boundary policy's owner and path depth.
    let cases: &[(&str, &str, &str, &str)] = &[
        ("ClearedRole", "/", "ClearedRole", AWS_MANAGED_POLICIES[0]),
        (
            "nested-cleared",
            "/eng/tools/",
            "NESTED-CLEARED",
            AWS_MANAGED_POLICIES[2],
        ),
        (
            "role_with+every=allowed.char@-",
            "/eng/",
            "role_with+every=allowed.char@-",
            AWS_MANAGED_POLICIES[3],
        ),
        ("CustomerCleared", "/", "customercleared", &customer_root),
        (
            "NestedPolicyCleared",
            "/sales/west/",
            "NestedPolicyCleared",
            &customer_nested,
        ),
    ];

    for (created_name, path, delete_name, boundary) in cases {
        let case = format!("{created_name:?} at {path:?} <- {boundary:?}");
        create_role(&router, created_name, path).await;

        let (status, body) = put_boundary(&router, created_name, boundary).await;
        assert_eq!(
            status, 200,
            "case {case}: the fixture put must succeed, got {status}: {body}"
        );
        let reported = reported_boundary(&router, created_name).await;
        assert_eq!(
            reported,
            Some((
                PERMISSIONS_BOUNDARY_TYPE.to_string(),
                (*boundary).to_string()
            )),
            "case {case}: the fixture boundary must be set, got {reported:?}"
        );
        let arn_before = reported_role_arn(&router, created_name).await;

        let (status, body) = delete_boundary(&router, delete_name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status}: {body}"
        );
        assert!(
            elements(&body, "Code").is_empty(),
            "case {case}: a served delete carries no error code, got {body}"
        );
        // The modelled output is `smithy.api#Unit`, so the body is the awsQuery response
        // envelope and nothing else: no member of the role's record comes back.
        assert!(
            body.contains("DeleteRolePermissionsBoundaryResponse"),
            "case {case}: expected the operation's own empty response envelope, got {body}"
        );

        let reported = reported_boundary(&router, created_name).await;
        assert_eq!(
            reported, None,
            "case {case}: expected no boundary after the delete, got {reported:?}"
        );
        let arn_after = reported_role_arn(&router, created_name).await;
        assert_eq!(
            arn_after, arn_before,
            "case {case}: the delete must leave the role itself, got {arn_after:?}"
        );
    }
}

/// RULE: clearing a boundary is a write of the role's own record, not the consumption of
/// something the role can run out of — so a boundary can be set and cleared again as
/// often as a caller likes, each clear served and each read reporting what the last call
/// left.
#[tokio::test]
async fn a_boundary_can_be_set_and_cleared_again_however_often() {
    let router = router();
    let role = "RecycledBoundaryRole";
    create_role(&router, role, "/").await;
    let customer = create_policy(&router, "recycled", "/boundaries/").await;

    assert_eq!(
        reported_boundary(&router, role).await,
        None,
        "a role created without a boundary reports none"
    );

    // Rows: the ARN each cycle sets before clearing it — an AWS-managed policy, a REPEAT
    // of the same one, a policy this account holds, and one filed under a deeper path.
    let cycles = [
        AWS_MANAGED_POLICIES[0],
        AWS_MANAGED_POLICIES[0],
        customer.as_str(),
        AWS_MANAGED_POLICIES[3],
    ];
    for (index, boundary) in cycles.iter().enumerate() {
        let case = format!("cycle {index} <- {boundary:?}");
        let (status, body) = put_boundary(&router, role, boundary).await;
        assert_eq!(
            status, 200,
            "case {case}: the put must succeed, got {status}: {body}"
        );
        let reported = reported_boundary(&router, role).await;
        assert_eq!(
            reported,
            Some((PERMISSIONS_BOUNDARY_TYPE.to_string(), (*boundary).to_string())),
            "case {case}: expected the put boundary, got {reported:?}"
        );

        let (status, body) = delete_boundary(&router, role).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the clear to be served, got {status}: {body}"
        );
        let reported = reported_boundary(&router, role).await;
        assert_eq!(
            reported, None,
            "case {case}: expected no boundary after the clear, got {reported:?}"
        );
    }
}

/// RULE: `RoleName` names a role the account holds, so a name no role is held under
/// refers to no entity at all — 404 `NoSuchEntity`, worded for the role that is missing,
/// and nothing else in the account changes.
#[tokio::test]
async fn clearing_the_boundary_of_a_name_no_role_is_held_under_is_refused() {
    let router = router();
    create_role(&router, "HeldRole", "/").await;
    let (status, body) = put_boundary(&router, "HeldRole", AWS_MANAGED_POLICIES[0]).await;
    assert_eq!(
        status, 200,
        "the fixture boundary must be set, got {status}: {body}"
    );

    // Rows spanning the forms a name can miss a held role by: never created, a near miss
    // of a held name, a held name with a separator the folded name does not have, the
    // whole allowed alphabet, and a name carrying the service-linked role prefix that no
    // service publishes — which shows the protected refusal below turns on the role's
    // PATH, not on the shape of its name.
    let unknown = [
        "NoSuchRole",
        "HeldRole2",
        "held_role",
        "another+name=entirely.x@-",
        "AWSServiceRoleForNoServiceAtAll",
    ];
    for role_name in unknown {
        let (status, body) = delete_boundary(&router, role_name).await;
        assert_eq!(
            status, 404,
            "case {role_name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {role_name:?}: expected NoSuchEntity, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!("The role with name {role_name} cannot be found."),
            "case {role_name:?}: expected the refusal to name the missing role, got \
             {message:?}"
        );
    }

    // The held role is untouched by every refusal: its boundary is still the fixture's.
    let reported = reported_boundary(&router, "HeldRole").await;
    assert_eq!(
        reported,
        Some((
            PERMISSIONS_BOUNDARY_TYPE.to_string(),
            AWS_MANAGED_POLICIES[0].to_string()
        )),
        "a refused delete must clear nothing, got {reported:?}"
    );
}

/// RULE: what this call deletes is the BOUNDARY, so a role the account holds that carries
/// none has no such resource to delete and the call is refused `NoSuchEntity` (HTTP 404)
/// rather than served — whether the role never carried one or a first clear already took
/// it away. What is pinned here is the modelled status and awsQuery code; the role itself
/// is reported throughout, which is what shows the refusal is about the boundary.
#[tokio::test]
async fn clearing_a_boundary_a_role_does_not_carry_is_refused() {
    let router = router();

    // Rows: (role name, path, the name the DELETE spells, whether a boundary was ever
    // set). They differ along the role's path depth, the case the delete spells the name
    // in, and how the role came to carry no boundary — never bounded, or bounded and
    // cleared already.
    let cases: &[(&str, &str, &str, bool)] = &[
        ("NeverBounded", "/", "NeverBounded", false),
        ("never-bounded-nested", "/eng/tools/", "NEVER-BOUNDED-NESTED", false),
        ("ClearedOnce", "/", "clearedonce", true),
        ("ClearedOnceNested", "/sales/", "ClearedOnceNested", true),
    ];

    for (role_name, path, delete_name, was_bounded) in cases {
        let case = format!("{role_name:?} at {path:?}, ever bounded: {was_bounded}");
        create_role(&router, role_name, path).await;
        if *was_bounded {
            let (status, body) = put_boundary(&router, role_name, AWS_MANAGED_POLICIES[1]).await;
            assert_eq!(
                status, 200,
                "case {case}: the fixture put must succeed, got {status}: {body}"
            );
            let (status, body) = delete_boundary(&router, role_name).await;
            assert_eq!(
                status, 200,
                "case {case}: the first clear must be served, got {status}: {body}"
            );
        }
        assert_eq!(
            reported_boundary(&router, role_name).await,
            None,
            "case {case}: the role must carry no boundary before the refused clear"
        );

        let (status, body) = delete_boundary(&router, delete_name).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case}: expected NoSuchEntity, got {:?}: {body}",
            error_code(&body)
        );

        // The ROLE is not what is missing: `GetRole` still reports it.
        let arn = reported_role_arn(&router, role_name).await;
        assert!(
            arn.ends_with(&format!("{path}{role_name}")),
            "case {case}: the role itself must still be reported, got {arn:?}"
        );
    }
}

/// RULE: a role a service links to itself is a protected resource only that service may
/// modify, so clearing its boundary is refused 400 `UnmodifiableEntity` in the one fixed
/// sentence this service states that protection with — the role name quoted as the
/// REQUEST spelled it, and nothing of the service the role is linked to. The protection
/// is decided before the role's boundary is looked at: such a role carries none, and the
/// answer is still the protected refusal rather than "no such entity".
#[tokio::test]
async fn clearing_a_service_linked_roles_boundary_is_refused_as_unmodifiable() {
    let router = router();

    for (principal, custom_suffix) in LINKED_SERVICES {
        let minted = service_linked_role(&router, principal, *custom_suffix).await;
        // Rows spanning the spelling the request names the role by: as minted,
        // upper-cased and lower-cased.
        let spellings = [
            ("as minted", minted.clone()),
            ("upper-cased", minted.to_uppercase()),
            ("lower-cased", minted.to_lowercase()),
        ];
        for (spelling_case, spelling) in &spellings {
            let case = format!("{principal} / {spelling_case}");
            let (status, body) = delete_boundary(&router, spelling).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a protected role's boundary must not be cleared, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnmodifiableEntity",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                protected(spelling),
                "case {case:?}: expected the service's one protected-role sentence, got \
                 {message:?}"
            );
            assert!(
                !message.contains(*principal),
                "case {case:?}: the sentence must not name the service the role is linked \
                 to, got {message:?}"
            );
        }

        // The role is still reported, and still carries no boundary of the account's.
        let reported = reported_boundary(&router, &minted).await;
        assert_eq!(
            reported, None,
            "case {principal:?}: a refused delete must change nothing, got {reported:?}"
        );
    }
}

/// The role names AWS PUBLISHES for a service's own service-linked role, which this
/// account does not have to create for `GetRole` to report them.
///
/// Source: `state::PUBLISHED_SERVICE_LINKED_ROLE_NAMES`, the catalog
/// `tests/all/service_provisioned_roles.rs` pins against the services' own "Using
/// service-linked roles" pages. The rows span a service's own role, a role whose
/// AWS-assigned label drops the hyphen of the principal's, and one suffixed with the
/// resource type it scales.
const PUBLISHED_PROTECTED_ROLES: &[&str] = &[
    "AWSServiceRoleForECS",
    "AWSServiceRoleForComputeOptimizer",
    "AWSServiceRoleForApplicationAutoScaling_DynamoDBTable",
];

/// RULE: the protection is about the ROLE, so it holds for a role a service keeps
/// provisioned under its published name just as for one this account created through
/// `CreateServiceLinkedRole` — the same 400 `UnmodifiableEntity` and the same sentence.
#[tokio::test]
async fn clearing_the_boundary_of_a_role_a_service_keeps_provisioned_is_refused() {
    let router = router();

    for role_name in PUBLISHED_PROTECTED_ROLES {
        let (status, body) = delete_boundary(&router, role_name).await;
        assert_eq!(
            status, 400,
            "case {role_name:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "UnmodifiableEntity",
            "case {role_name:?}: expected UnmodifiableEntity, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            protected(role_name),
            "case {role_name:?}: expected the service's one protected-role sentence, got \
             {message:?}"
        );

        // The role the service keeps provisioned is reported, and carries no boundary —
        // which is what shows the refusal came from the protection rather than from the
        // name resolving to nothing.
        let reported = reported_boundary(&router, role_name).await;
        assert_eq!(
            reported, None,
            "case {role_name:?}: a refused delete must change nothing, got {reported:?}"
        );
    }
}
