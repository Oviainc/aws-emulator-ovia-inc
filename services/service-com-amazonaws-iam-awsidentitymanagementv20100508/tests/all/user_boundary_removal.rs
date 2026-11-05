// SPDX-License-Identifier: CC0-1.0

//! `DeleteUserPermissionsBoundary` end to end over the awsQuery wire: the write that
//! clears the boundary a user carries, and the two refusals the operation models for a
//! name it cannot serve.
//!
//! RULE under test: the call answers from the account's own user records — it clears the
//! boundary of a user the account holds and answers the empty HTTP 200 its modelled
//! `Unit` output is, which `GetUser` then confirms while the user itself is untouched; a
//! name no user is held under is `NoSuchEntity` (HTTP 404), the refusal naming the user
//! that is missing (`User <name> was not found.`); and a user the account holds that
//! carries no boundary has no such resource to delete, so that call is `NoSuchEntity` too,
//! the refusal describing the missing SCOPE and naming the principal by the unique id
//! `GetUser` reports for it, while the user stays reported.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! user's path depth, the case the request spells its name in and the alphabet that name
//! uses, whether the boundary names an AWS-managed or a customer-managed policy and the
//! path that policy is filed under, how many set-and-clear cycles the user has seen, the
//! form a name misses a held user by, and how a user came to carry no boundary (never
//! bounded, bounded and cleared already, or bounded by `CreateUser` itself and cleared).
//! No single request is the specification.
//!
//! The awsQuery harness (`router`, `call`, `element`, `elements`, `error_code`) is the one
//! `tests/all/user_lifecycle.rs` owns; the boundary fixtures (`create_user`,
//! `create_policy`, `put_boundary`, `reported_boundary`) are the ones the put suite beside
//! this one already owns.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, elements, error_code, router};
use crate::user_permissions_boundary::{
    create_policy, create_user, put_boundary, reported_boundary, AWS_MANAGED_POLICIES,
    PERMISSIONS_BOUNDARY_TYPE,
};

/// One `DeleteUserPermissionsBoundary` call: the only member the operation takes is
/// `UserName` (`smithy_query(query="operation",
/// operation="DeleteUserPermissionsBoundary")`).
async fn delete_boundary(router: &MultiServiceRouter, user_name: &str) -> (u16, String) {
    call(
        router,
        "DeleteUserPermissionsBoundary",
        &[("UserName", user_name)],
    )
    .await
}

/// The `Arn` `GetUser` reports for `user_name` — what shows the user ITSELF survived a
/// call that only cleared its boundary.
async fn reported_user_arn(router: &MultiServiceRouter, user_name: &str) -> String {
    let (status, body) = call(router, "GetUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: GetUser must report the user, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// The `UserId` `GetUser` reports for `user_name` — the unique id IAM names the principal
/// by when it refuses a boundary delete for a user that carries no boundary.
async fn reported_user_id(router: &MultiServiceRouter, user_name: &str) -> String {
    let (status, body) = call(router, "GetUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: GetUser must report the user, got {status}: {body}"
    );
    element(&body, "UserId")
}

/// RULE: the call clears the one boundary the named user carries and answers the empty
/// 200 its modelled `Unit` output is, so a later `GetUser` reports no boundary at all
/// while the user itself is untouched — whatever the user's path, whatever case the
/// request spells its name in, and whichever managed policy the boundary named.
#[tokio::test]
async fn clearing_a_boundary_is_served_and_get_user_then_reports_none() {
    let router = router();

    // The customer-managed policies the rows below bound a user with, created first so
    // the ARNs name policies this account actually holds.
    let customer_root = create_policy(&router, "user-cleared-at-root", "/").await;
    let customer_nested = create_policy(&router, "Cleared_2", "/boundaries/eng/").await;

    // Rows: (user name as created, user path, the name the DELETE spells, boundary ARN).
    // They differ along the user's path depth, the case the delete spells the name in
    // (IAM entity names "are not distinguished by case"), the name alphabet, and the
    // boundary policy's owner and path depth.
    let cases: &[(&str, &str, &str, &str)] = &[
        ("ClearedUser", "/", "ClearedUser", AWS_MANAGED_POLICIES[0]),
        (
            "nested-cleared",
            "/eng/tools/",
            "NESTED-CLEARED",
            AWS_MANAGED_POLICIES[2],
        ),
        (
            "user_with+every=allowed.char@-",
            "/eng/",
            "user_with+every=allowed.char@-",
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
        create_user(&router, created_name, path).await;

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
        let arn_before = reported_user_arn(&router, created_name).await;

        let (status, body) = delete_boundary(&router, delete_name).await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status}: {body}");
        assert!(
            elements(&body, "Code").is_empty(),
            "case {case}: a served delete carries no error code, got {body}"
        );
        // The modelled output is `smithy.api#Unit`, so the body is the awsQuery response
        // envelope and nothing else: no member of the user's record comes back.
        assert!(
            body.contains("DeleteUserPermissionsBoundaryResponse"),
            "case {case}: expected the operation's own empty response envelope, got {body}"
        );

        let reported = reported_boundary(&router, created_name).await;
        assert_eq!(
            reported, None,
            "case {case}: expected no boundary after the delete, got {reported:?}"
        );
        let arn_after = reported_user_arn(&router, created_name).await;
        assert_eq!(
            arn_after, arn_before,
            "case {case}: the delete must leave the user itself, got {arn_after:?}"
        );
    }
}

/// RULE: clearing a boundary is a write of the user's own record, not the consumption of
/// something the user can run out of — so a boundary can be set and cleared again as
/// often as a caller likes, each clear served and each read reporting what the last call
/// left.
#[tokio::test]
async fn a_boundary_can_be_set_and_cleared_again_however_often() {
    let router = router();
    let user = "RecycledBoundaryUser";
    create_user(&router, user, "/").await;
    let customer = create_policy(&router, "user-recycled", "/boundaries/").await;

    assert_eq!(
        reported_boundary(&router, user).await,
        None,
        "a user created without a boundary reports none"
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
        let (status, body) = put_boundary(&router, user, boundary).await;
        assert_eq!(
            status, 200,
            "case {case}: the put must succeed, got {status}: {body}"
        );
        let reported = reported_boundary(&router, user).await;
        assert_eq!(
            reported,
            Some((
                PERMISSIONS_BOUNDARY_TYPE.to_string(),
                (*boundary).to_string()
            )),
            "case {case}: expected the put boundary, got {reported:?}"
        );

        let (status, body) = delete_boundary(&router, user).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the clear to be served, got {status}: {body}"
        );
        let reported = reported_boundary(&router, user).await;
        assert_eq!(
            reported, None,
            "case {case}: expected no boundary after the clear, got {reported:?}"
        );
    }
}

/// RULE: `UserName` names a user the account holds, so a name no user is held under
/// refers to no entity at all — 404 `NoSuchEntity`, worded for the user that is missing,
/// and nothing else in the account changes.
#[tokio::test]
async fn clearing_the_boundary_of_a_name_no_user_is_held_under_is_refused() {
    let router = router();
    create_user(&router, "HeldBoundedUser", "/").await;
    let (status, body) = put_boundary(&router, "HeldBoundedUser", AWS_MANAGED_POLICIES[0]).await;
    assert_eq!(
        status, 200,
        "the fixture boundary must be set, got {status}: {body}"
    );

    // Rows spanning the forms a name can miss a held user by: never created, a near miss
    // of a held name, a held name with a separator the folded name does not have, and the
    // whole allowed alphabet.
    let unknown = [
        "NoSuchUser",
        "HeldBoundedUser2",
        "held_bounded_user",
        "another+name=entirely.x@-",
    ];
    for user_name in unknown {
        let (status, body) = delete_boundary(&router, user_name).await;
        assert_eq!(
            status, 404,
            "case {user_name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {user_name:?}: expected NoSuchEntity, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!("User {user_name} was not found."),
            "case {user_name:?}: expected the refusal to name the missing user, got \
             {message:?}"
        );
    }

    // The held user is untouched by every refusal: its boundary is still the fixture's.
    let reported = reported_boundary(&router, "HeldBoundedUser").await;
    assert_eq!(
        reported,
        Some((
            PERMISSIONS_BOUNDARY_TYPE.to_string(),
            AWS_MANAGED_POLICIES[0].to_string()
        )),
        "a refused delete must clear nothing, got {reported:?}"
    );
}

/// How a user in the suite below came to carry no boundary.
#[derive(Debug, Clone, Copy)]
enum HowUnbounded {
    /// The user was created plain and never bounded.
    NeverBounded,
    /// A put set a boundary and a first clear already took it away.
    ClearedAfterPut,
    /// `CreateUser` itself set the boundary and a first clear took it away.
    ClearedAfterCreateWithBoundary,
}

/// RULE: what this call deletes is the BOUNDARY, so a user the account holds that carries
/// none has no such resource to delete and the call is refused `NoSuchEntity` (HTTP 404)
/// rather than served — whether the user never carried one, a first clear already took it
/// away, or the boundary it was created with was cleared. The refusal describes the
/// missing BOUNDARY, not the user: `GetUser` reports the user throughout.
#[tokio::test]
async fn clearing_a_boundary_a_user_does_not_carry_is_refused() {
    let router = router();

    // Rows: (user name, path, the name the DELETE spells, how the user came to carry no
    // boundary). They differ along the user's path depth, the case the delete spells the
    // name in, and which write left the user unbounded.
    let cases: &[(&str, &str, &str, HowUnbounded)] = &[
        (
            "NeverBoundedUser",
            "/",
            "NeverBoundedUser",
            HowUnbounded::NeverBounded,
        ),
        (
            "never-bounded-nested",
            "/eng/tools/",
            "NEVER-BOUNDED-NESTED",
            HowUnbounded::NeverBounded,
        ),
        (
            "ClearedOnce",
            "/",
            "clearedonce",
            HowUnbounded::ClearedAfterPut,
        ),
        (
            "ClearedOnceNested",
            "/sales/",
            "ClearedOnceNested",
            HowUnbounded::ClearedAfterPut,
        ),
        (
            "CreatedWithBoundaryThenCleared",
            "/",
            "CreatedWithBoundaryThenCleared",
            HowUnbounded::ClearedAfterCreateWithBoundary,
        ),
    ];

    for (user_name, path, delete_name, how) in cases {
        let case = format!("{user_name:?} at {path:?}, {how:?}");
        match how {
            HowUnbounded::NeverBounded => create_user(&router, user_name, path).await,
            HowUnbounded::ClearedAfterPut => {
                create_user(&router, user_name, path).await;
                let (status, body) = put_boundary(&router, user_name, AWS_MANAGED_POLICIES[1]).await;
                assert_eq!(
                    status, 200,
                    "case {case}: the fixture put must succeed, got {status}: {body}"
                );
                let (status, body) = delete_boundary(&router, user_name).await;
                assert_eq!(
                    status, 200,
                    "case {case}: the first clear must be served, got {status}: {body}"
                );
            }
            HowUnbounded::ClearedAfterCreateWithBoundary => {
                let (status, body) = call(
                    &router,
                    "CreateUser",
                    &[
                        ("UserName", user_name),
                        ("Path", path),
                        ("PermissionsBoundary", AWS_MANAGED_POLICIES[3]),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case}: CreateUser must succeed, got {status}: {body}"
                );
                let (status, body) = delete_boundary(&router, user_name).await;
                assert_eq!(
                    status, 200,
                    "case {case}: the clear of the created boundary must be served, got \
                     {status}: {body}"
                );
            }
        }
        assert_eq!(
            reported_boundary(&router, user_name).await,
            None,
            "case {case}: the user must carry no boundary before the refused clear"
        );

        let (status, body) = delete_boundary(&router, delete_name).await;
        assert_eq!(status, 404, "case {case}: expected 404, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case}: expected NoSuchEntity, got {:?}: {body}",
            error_code(&body)
        );
        // The USER is not what is missing, so the refusal is not the one worded for an
        // absent user: it describes the SCOPE that is missing, naming the principal by the
        // unique id `GetUser` reports for it — and `GetUser` still reports the user.
        let message = element(&body, "Message");
        assert_ne!(
            message,
            format!("User {delete_name} was not found."),
            "case {case}: the refusal must describe the missing boundary, got {message:?}"
        );
        let user_id = reported_user_id(&router, user_name).await;
        assert_eq!(
            message,
            format!("Principal {user_id} does not have scope"),
            "case {case}: the refusal must name the principal by its unique id, got \
             {message:?}"
        );
        let arn = reported_user_arn(&router, user_name).await;
        assert!(
            arn.ends_with(&format!("{path}{user_name}")),
            "case {case}: the user itself must still be reported, got {arn:?}"
        );
    }
}
