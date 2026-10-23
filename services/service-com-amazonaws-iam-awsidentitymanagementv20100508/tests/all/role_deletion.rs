// SPDX-License-Identifier: CC0-1.0

//! `DeleteRole`, end to end over the awsQuery wire.
//!
//! RULE: a delete RELEASES the stored role the request names, so the name resolves
//! nowhere afterwards — `GetRole` and a second `DeleteRole` both answer the modelled
//! `NoSuchEntityException` — while every other role the account holds is untouched;
//! and a name the account never held is refused rather than reported as deleted.
//!
//! Releasing the role matters to more than IAM: a role is what another service's
//! resource is built on (a Transfer Family user's `Role` is one such ARN), so a
//! client's teardown cannot complete while the role it created cannot be deleted.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! role's name alphabet and length, its path depth, the case the delete spells the
//! name in, and which position in the account's role set is released. The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns, so every suite in this crate
//! builds and reads a request the same way.

use crate::user_lifecycle::{call, element, error_code, router};

/// A trust policy every fixture create sends: an IAM policy document, which is all
/// `CreateRole` requires of one.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"transfer.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The fixture roles: `(RoleName, Path)` rows spanning the whole `^[\w+=,.@-]+$` name
/// alphabet, the shortest and longest names `roleNameType` admits (1 and 64), and the
/// root, one-segment and nested paths `pathType` admits.
fn roles() -> Vec<(String, &'static str)> {
    vec![
        ("sim2real-probe-transfer-role".to_string(), "/"),
        ("r".to_string(), "/"),
        ("Role_7+x=y,z.w@example-1".to_string(), "/service-role/"),
        ("nested".to_string(), "/eng/tools/transfer/"),
        ("l".repeat(64), "/"),
    ]
}

/// Create every fixture role on a fresh router, and answer it.
async fn with_fixture_roles() -> smithy_server_runtime::MultiServiceRouter {
    let router = router();
    for (role_name, path) in roles() {
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
                ("Path", path),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: the fixture create must succeed, got {status}: {body}"
        );
    }
    router
}

/// RULE: deleting a held role releases exactly that role — it resolves nowhere
/// afterwards, and every role still held reads back unchanged.
///
/// The rows are the fixture roles, released one after another, so each delete is taken
/// against a different remaining set (all of them, then all but one, …, then the last).
/// The delete spells the name in UPPER case, because IAM entity names "are not
/// distinguished by case".
#[tokio::test]
async fn deleting_a_role_releases_that_role_and_leaves_the_others() {
    let router = with_fixture_roles().await;

    // The ids the creates minted, so "unchanged" is asserted against a stored value
    // rather than against the fact that a read answered at all.
    let mut held: Vec<(String, String)> = Vec::new();
    for (role_name, _) in roles() {
        let (status, body) = call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: the fixture role must read back, got {status}: {body}"
        );
        held.push((role_name, element(&body, "RoleId")));
    }

    while let Some((role_name, _)) = held.first().cloned() {
        let spelling = role_name.to_ascii_uppercase();
        let (status, body) = call(&router, "DeleteRole", &[("RoleName", spelling.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: expected DeleteRole 200 for a held role spelled \
             {spelling:?}, got {status}: {body}"
        );

        // The released name resolves nowhere: neither a read nor a second delete.
        for action in ["GetRole", "DeleteRole"] {
            let (status, body) = call(&router, action, &[("RoleName", role_name.as_str())]).await;
            assert_eq!(
                status, 404,
                "case {role_name:?}: expected {action} 404 after the delete, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {role_name:?}: expected {action} to answer the modelled awsQuery code, \
                 got {body}"
            );
        }

        held.remove(0);
        for (still_held, role_id) in &held {
            let (status, body) =
                call(&router, "GetRole", &[("RoleName", still_held.as_str())]).await;
            assert_eq!(
                status, 200,
                "case {role_name:?}: releasing it released {still_held:?} too, got {status}: \
                 {body}"
            );
            assert_eq!(
                element(&body, "RoleId"),
                *role_id,
                "case {role_name:?}: releasing it changed the stored {still_held:?}, got {body}"
            );
        }
    }
}

/// RULE: a name the account does not hold is refused with `NoSuchEntity` (HTTP 404)
/// rather than reported as deleted, and the refusal changes nothing.
///
/// The rows are names the account never held: one unrelated, two near-misses of a held
/// name, the held name with one character appended, and a name at the longest length
/// `roleNameType` admits.
#[tokio::test]
async fn deleting_a_role_the_account_does_not_hold_is_not_found() {
    let router = router();
    let (status, created) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "held"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture create must succeed, got {status}: {created}"
    );
    let role_id = element(&created, "RoleId");

    let longest = "l".repeat(64);
    for absent in ["nobody", "hel", "helds", "held1", longest.as_str()] {
        let (status, body) = call(&router, "DeleteRole", &[("RoleName", absent)]).await;
        assert_eq!(
            status, 404,
            "case {absent:?}: expected DeleteRole 404 for an unheld name, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // The refusals released nothing: the one held role is still there, with the id
    // its create minted.
    let (status, body) = call(&router, "GetRole", &[("RoleName", "held")]).await;
    assert_eq!(
        status, 200,
        "the held role must survive the refused deletes, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "RoleId"),
        role_id,
        "the refused deletes changed the held role, got {body}"
    );
}

/// RULE: a role that still holds a policy is NOT deleted — "before you delete a role,
/// remove the … inline policies [and] attached managed policies" — so the delete is
/// refused with `DeleteConflict` (HTTP 409) and the role is still there afterwards,
/// while a role that holds none is released.
///
/// The rows differ along how many policies the role holds (one, two, three), the path
/// the managed policy's ARN carries (the account root, a service-role path, a
/// job-function path), and WHICH KIND of policy holds the role back — a managed one
/// attached to it, an inline one embedded in it, or both.
#[tokio::test]
async fn a_role_that_still_holds_a_policy_is_not_deleted() {
    // AWS-managed policy ARNs, spanning the paths the managed-policy ARN grammar
    // admits — entries of the AWS Managed Policy Reference, as this crate's
    // `tests/all/role_attached_policies.rs` uses.
    let policies = [
        "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess",
        "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole",
        "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
    ];
    // An inline policy document: a JSON object carrying a `Statement`, which is what
    // `PutRolePolicy` requires of one.
    const INLINE_POLICY: &str =
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

    // (case, how many managed policies to attach, how many inline policies to embed).
    let cases: [(&str, usize, usize); 5] = [
        ("one attached managed policy", 1, 0),
        ("three attached managed policies", 3, 0),
        ("one embedded inline policy", 0, 1),
        ("two embedded inline policies", 0, 2),
        ("both kinds at once", 2, 1),
    ];

    for (case, attach, embed) in cases {
        let router = router();
        let role_name = format!("holder-{}-{attach}-{embed}", case.len());
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture create must succeed, got {status}: {body}"
        );
        let role_id = element(&body, "RoleId");

        for policy_arn in policies.iter().take(attach) {
            let (status, body) = call(
                &router,
                "AttachRolePolicy",
                &[
                    ("RoleName", role_name.as_str()),
                    ("PolicyArn", policy_arn),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: attaching {policy_arn:?} must succeed, got {status}: {body}"
            );
        }
        for index in 0..embed {
            let policy_name = format!("inline-{index}");
            let (status, body) = call(
                &router,
                "PutRolePolicy",
                &[
                    ("RoleName", role_name.as_str()),
                    ("PolicyName", policy_name.as_str()),
                    ("PolicyDocument", INLINE_POLICY),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: embedding {policy_name:?} must succeed, got {status}: {body}"
            );
        }

        let (status, body) = call(&router, "DeleteRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected DeleteRole 409 while the role still holds a policy, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "DeleteConflict",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // The refused delete released nothing: the role is still held, unchanged.
        let (status, body) = call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the role must survive the refused delete, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "RoleId"),
            role_id,
            "case {case:?}: the refused delete changed the stored role, got {body}"
        );

        // A role holding nothing, in the same account, is released as usual: it is
        // the POLICY that blocks the delete, not the account's state in general.
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", "unattached"),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the unattached fixture create must succeed, got {status}: {body}"
        );
        let (status, body) = call(&router, "DeleteRole", &[("RoleName", "unattached")]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a role holding no policy to be released, got {status}: \
             {body}"
        );
    }
}

/// RULE: a service-linked role is a protected resource — "only the service that
/// depends on the service-linked role can modify or delete the role on your behalf" —
/// so a caller's delete is refused with `UnmodifiableEntity` (HTTP 400) and the role
/// is still held afterwards, while an ordinary role of the account is released.
///
/// The rows are service principals whose service-linked role paths differ in label
/// count and case, plus one created with a `CustomSuffix`, so the refusal is asserted
/// on the role's PATH rather than on one derived name.
#[tokio::test]
async fn a_service_linked_role_is_not_deleted_by_its_account() {
    // (the service principal, the optional CustomSuffix, the role name the create
    // names) — the naming this crate's `tests/all/service_linked_roles.rs` pins
    // (AWS's published name for the service, or the derived fallback when AWS
    // publishes none), written out so this suite asserts against a name rather than
    // restating the rule.
    let cases: [(&str, Option<&str>, &str); 4] = [
        (
            "networkmonitor.amazonaws.com",
            None,
            "AWSServiceRoleForNetworkmonitor",
        ),
        (
            "ops.apigateway.amazonaws.com",
            None,
            "AWSServiceRoleForAPIGateway",
        ),
        (
            "autoscaling.amazonaws.com",
            Some("debug"),
            "AWSServiceRoleForAutoScaling_debug",
        ),
        (
            "Mediaconvert.amazonaws.com",
            None,
            "AWSServiceRoleForMediaconvert",
        ),
    ];

    for (principal, suffix, role_name) in cases {
        let router = router();
        let mut params = vec![("AWSServiceName", principal)];
        if let Some(suffix) = suffix {
            params.push(("CustomSuffix", suffix));
        }
        let (status, created) = call(&router, "CreateServiceLinkedRole", &params).await;
        assert_eq!(
            status, 200,
            "case {principal:?}: the fixture create must succeed, got {status}: {created}"
        );
        assert_eq!(
            element(&created, "RoleName"),
            role_name,
            "case {principal:?}: expected the role name the create names, got {created}"
        );
        let role_id = element(&created, "RoleId");

        // However the delete spells the protected role's name, it is refused.
        for spelling in [role_name.to_string(), role_name.to_ascii_uppercase()] {
            let (status, body) =
                call(&router, "DeleteRole", &[("RoleName", spelling.as_str())]).await;
            assert_eq!(
                status, 400,
                "case {principal:?}/{spelling:?}: expected DeleteRole 400 for a \
                 service-linked role, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnmodifiableEntity",
                "case {principal:?}/{spelling:?}: expected the modelled awsQuery code, \
                 got {body}"
            );
        }

        // The refused delete released nothing: the protected role is still held.
        let (status, body) = call(&router, "GetRole", &[("RoleName", role_name)]).await;
        assert_eq!(
            status, 200,
            "case {principal:?}: the protected role must survive the refused delete, \
             got {status}: {body}"
        );
        assert_eq!(
            element(&body, "RoleId"),
            role_id,
            "case {principal:?}: the refused delete changed the stored role, got {body}"
        );

        // An ordinary role of the same account IS released: it is the role's
        // service-linked path that protects it, not the account's state in general.
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", "ordinary"),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {principal:?}: the ordinary fixture create must succeed, got {status}: {body}"
        );
        let (status, body) = call(&router, "DeleteRole", &[("RoleName", "ordinary")]).await;
        assert_eq!(
            status, 200,
            "case {principal:?}: expected an ordinary role to be released, got {status}: {body}"
        );
    }
}

/// RULE: a released name is free again — a create may take it, and the new role is a
/// NEW role (its own minted id), not the released one coming back.
///
/// The rows are the fixture names, each released and then re-created.
#[tokio::test]
async fn a_released_name_can_be_created_again_as_a_new_role() {
    let router = with_fixture_roles().await;

    for (role_name, path) in roles() {
        let (status, before) = call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: the fixture role must read back, got {status}: {before}"
        );
        let first_id = element(&before, "RoleId");

        let (status, body) = call(&router, "DeleteRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: expected DeleteRole 200, got {status}: {body}"
        );

        // The name is free: a create takes it, with a different path to show the new
        // role is built from the new request rather than restored.
        let (status, recreated) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
                ("Path", "/recreated/"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: expected a released name to be free, got {status}: {recreated}"
        );
        assert_ne!(
            element(&recreated, "RoleId"),
            first_id,
            "case {role_name:?}: expected a NEW role id after the release, got the old {first_id:?}"
        );
        assert_eq!(
            element(&recreated, "Path"),
            "/recreated/",
            "case {role_name:?}: expected the re-created role to carry the new path, not the \
             released {path:?}, got {recreated}"
        );
    }
}
