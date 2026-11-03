// SPDX-License-Identifier: CC0-1.0

//! `DeletePolicy`, end to end over the awsQuery wire.
//!
//! RULE: a delete RELEASES the customer managed policy the request's ARN resolves to —
//! the policy and its default version — so that ARN resolves nowhere afterwards
//! (`GetPolicy` and a second `DeletePolicy` both answer `NoSuchEntity`, and
//! `ListPolicies` stops reporting it) while every other policy the account holds is
//! untouched; an ARN that is not a managed-policy ARN is an invalid input, an ARN of
//! AWS's own managed-policy namespace names a policy the account cannot remove, and a
//! policy still related to an entity — attached to it, or used to set its permissions
//! boundary — is refused rather than half-deleted.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! policy's path depth, the name's alphabet and length, which position in the
//! account's policy set is released, the kind and the number of the relations that
//! hold a policy back, and the form of a rejected ARN. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns, so every suite in this crate builds and reads a
//! request the same way.

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A managed policy document: a JSON object carrying a `Statement`, which is what
/// `CreatePolicy` requires of one.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy every fixture `CreateRole` sends.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"transfer.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// A customer managed policy's ARN is `arn:aws:iam::<account>:policy<path><name>` —
/// IAM is global, so the region segment is EMPTY.
fn customer_policy_arn(path: &str, policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy{path}{policy_name}", account_id())
}

/// The fixture policies: `(PolicyName, Path)` rows spanning the `^[\w+=,.@-]+$` name
/// alphabet, the shortest name `policyNameType` admits (1) and a long one, and the
/// root, one-segment and nested paths `policyPathType` admits.
fn policies() -> Vec<(String, &'static str)> {
    vec![
        ("sim2real-probe-policy".to_string(), "/"),
        ("p".to_string(), "/"),
        ("Policy_7+x=y,z.w@example-1".to_string(), "/service-role/"),
        ("nested".to_string(), "/eng/tools/iam/"),
        ("l".repeat(96), "/"),
    ]
}

/// Create one customer managed policy and answer the ARN its create reported.
async fn create_policy(
    router: &smithy_server_runtime::MultiServiceRouter,
    policy_name: &str,
    path: &str,
) -> String {
    let params = vec![
        param("PolicyName", policy_name),
        param("PolicyDocument", POLICY_DOCUMENT),
        param("Path", path),
    ];
    let (status, body) = call(router, "CreatePolicy", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {policy_name:?} at {path:?}: the fixture create must succeed, got {status}: {body}"
    );
    let arn = element(&body, "Arn");
    assert_eq!(
        arn,
        customer_policy_arn(path, policy_name),
        "case {policy_name:?} at {path:?}: the create must answer the policy's own ARN, got {body}"
    );
    arn
}

/// Create every fixture policy on a fresh router, and answer it with the ARNs.
async fn with_fixture_policies() -> (smithy_server_runtime::MultiServiceRouter, Vec<String>) {
    let router = router();
    let mut arns = Vec::new();
    for (policy_name, path) in policies() {
        arns.push(create_policy(&router, &policy_name, path).await);
    }
    (router, arns)
}

/// The ARNs `ListPolicies` reports for the policies this account created.
async fn listed_customer_policy_arns(
    router: &smithy_server_runtime::MultiServiceRouter,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListPolicies",
        &[("Scope", "Local"), ("MaxItems", "1000")],
    )
    .await;
    assert_eq!(
        status, 200,
        "ListPolicies must succeed, got {status}: {body}"
    );
    elements(&body, "Arn")
}

/// RULE: deleting a held policy releases exactly that policy — its ARN resolves
/// nowhere afterwards, it is no longer listed, and every policy still held reads back
/// unchanged.
///
/// The rows are the fixture policies, released one after another, so each delete is
/// taken against a different remaining set (all of them, then all but one, …, then the
/// last).
#[tokio::test]
async fn deleting_a_policy_releases_that_policy_and_leaves_the_others() {
    let (router, arns) = with_fixture_policies().await;

    // The ids the creates minted, so "unchanged" is asserted against a stored value
    // rather than against the fact that a read answered at all.
    let mut held: Vec<(String, String)> = Vec::new();
    for arn in &arns {
        let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", arn.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {arn:?}: the fixture policy must read back, got {status}: {body}"
        );
        held.push((arn.clone(), element(&body, "PolicyId")));
    }

    while let Some((arn, _)) = held.first().cloned() {
        let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", arn.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {arn:?}: expected DeletePolicy 200 for a held policy, got {status}: {body}"
        );

        // The released ARN resolves nowhere: neither a read nor a second delete.
        for action in ["GetPolicy", "DeletePolicy", "ListPolicyVersions"] {
            let (status, body) = call(&router, action, &[("PolicyArn", arn.as_str())]).await;
            assert_eq!(
                status, 404,
                "case {arn:?}: expected {action} 404 after the delete, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {arn:?}: expected {action} to answer the modelled awsQuery code, got {body}"
            );
        }

        held.remove(0);
        let listed = listed_customer_policy_arns(&router).await;
        assert!(
            !listed.contains(&arn),
            "case {arn:?}: the released policy is still listed, got {listed:?}"
        );
        for (still_held, policy_id) in &held {
            assert!(
                listed.contains(still_held),
                "case {arn:?}: releasing it unlisted {still_held:?} too, got {listed:?}"
            );
            let (status, body) =
                call(&router, "GetPolicy", &[("PolicyArn", still_held.as_str())]).await;
            assert_eq!(
                status, 200,
                "case {arn:?}: releasing it released {still_held:?} too, got {status}: {body}"
            );
            assert_eq!(
                element(&body, "PolicyId"),
                *policy_id,
                "case {arn:?}: releasing it changed the stored {still_held:?}, got {body}"
            );
        }
    }
}

/// RULE: an ARN the account holds no policy under is refused with `NoSuchEntity`
/// (HTTP 404) rather than reported as deleted, and the refusal changes nothing — the
/// same resolution a read of that ARN performs.
///
/// The rows are managed-policy ARNs the account does not hold: an unrelated name, a
/// near-miss of a held name, the held name under another path, the held name in
/// another account, and the held name in another case (an ARN is matched exactly,
/// unlike the entity name a request names directly).
#[tokio::test]
async fn deleting_an_arn_the_account_holds_no_policy_under_is_not_found() {
    let router = router();
    let arn = create_policy(&router, "held", "/").await;
    let (status, created) = call(&router, "GetPolicy", &[("PolicyArn", arn.as_str())]).await;
    assert_eq!(
        status, 200,
        "the fixture policy must read back, got {status}: {created}"
    );
    let policy_id = element(&created, "PolicyId");

    let absent = [
        customer_policy_arn("/", "nobody"),
        customer_policy_arn("/", "helds"),
        customer_policy_arn("/eng/", "held"),
        "arn:aws:iam::210987654321:policy/held".to_string(),
        customer_policy_arn("/", "HELD"),
    ];
    for candidate in absent {
        // The code and the status are IAM's for this error class whichever operation
        // resolved the ARN; the SENTENCE belongs to the operation's family, so the
        // delete and the read of the policy resource word the absence one way and the
        // version listing, whose subject is the versions in force for the identities
        // the policy is attached to, words it the attachment family's way.
        for action in ["DeletePolicy", "GetPolicy", "ListPolicyVersions"] {
            let (status, body) = call(&router, action, &[("PolicyArn", candidate.as_str())]).await;
            assert_eq!(
                status, 404,
                "case {candidate:?}: expected {action} 404 for an unheld ARN, got {status}: \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {candidate:?}: expected {action} to answer the modelled awsQuery code, \
                 got {body}"
            );
            let message = element(&body, "Message");
            // Fixed text `Policy ` and ` was not found.` for the policy resource, and
            // `Policy ` and ` does not exist or is not attachable.` for the version
            // listing, with the ARN taken from the request.
            let expected = match action {
                "ListPolicyVersions" => {
                    format!("Policy {candidate} does not exist or is not attachable.")
                }
                _ => format!("Policy {candidate} was not found."),
            };
            assert_eq!(
                message, expected,
                "case {candidate:?}: expected IAM's wording for a policy {action} could not \
                 find, got {message:?}"
            );
        }
    }

    // The refusals released nothing: the one held policy is still there, with the id
    // its create minted.
    let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", arn.as_str())]).await;
    assert_eq!(
        status, 200,
        "the held policy must survive the refused deletes, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "PolicyId"),
        policy_id,
        "the refused deletes changed the held policy, got {body}"
    );
}

/// RULE: a value that is not a managed-policy ARN at all is an "invalid … value …
/// supplied for an input parameter" — `InvalidInput` (HTTP 400) — which is a different
/// refusal from an ARN the account does not hold.
///
/// The rows are values that satisfy `arnType`'s own 20..2048 length constraint (so the
/// handler, not the modelled constraint, is what refuses them) and span the ways a
/// value can fail the managed-policy ARN grammar: no ARN at all, another entity kind
/// (role, user, instance profile), an ARN carrying a region segment (IAM is global),
/// and a `policy` resource with no name after its path.
#[tokio::test]
async fn deleting_a_value_that_is_not_a_managed_policy_arn_is_invalid_input() {
    let router = router();
    let held = create_policy(&router, "held", "/").await;

    let rejected = [
        "this-is-not-an-arn-at-all".to_string(),
        format!("arn:aws:iam::{}:role/some-role", account_id()),
        format!("arn:aws:iam::{}:user/some-user", account_id()),
        format!("arn:aws:iam::{}:instance-profile/some-profile", account_id()),
        format!("arn:aws:iam:us-east-1:{}:policy/held", account_id()),
        format!("arn:aws:iam::{}:policy/", account_id()),
    ];
    for candidate in rejected {
        let (status, body) =
            call(&router, "DeletePolicy", &[("PolicyArn", candidate.as_str())]).await;
        assert_eq!(
            status, 400,
            "case {candidate:?}: expected DeletePolicy 400 for a value that is not a \
             managed-policy ARN, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {candidate:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // The refusals released nothing, and a well-formed ARN of a held policy still
    // deletes: it is the ARN's FORM that is refused above.
    let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", held.as_str())]).await;
    assert_eq!(
        status, 200,
        "expected the held policy to be released, got {status}: {body}"
    );
}

/// How a row clears the relation that holds its policy back, so the suite can assert
/// that the SAME delete then succeeds.
#[derive(Debug, Clone, Copy)]
enum Clear {
    /// Detach the policy from the roles it was attached to (`DetachRolePolicy`).
    DetachRoles,
    /// Delete the role that carried the policy as its permissions boundary — the
    /// boundary is a property of that role, so releasing the role releases the use.
    DeleteRole,
    /// Delete the user that carried the policy as its permissions boundary.
    DeleteUser,
    /// Nothing in this crate can clear the relation yet (`DetachUserPolicy` is still a
    /// scaffolded 501), so the row asserts only the refusal.
    Nothing,
}

/// RULE: a policy an entity still relates to — attached to it, or used to set its
/// permissions boundary — is NOT deleted: "before you can delete a managed policy, you
/// must first detach the policy from all users, groups, and roles that it is attached
/// to", so the delete is refused with `DeleteConflict` (HTTP 409), the policy is still
/// there afterwards, and the same delete succeeds once the relation is gone.
///
/// The rows differ along the KIND of relation (an attachment to a role, to a user, a
/// role's permissions boundary, a user's), along how many entities hold the policy
/// (one, two, three), and along the path the policy's own ARN carries.
#[tokio::test]
async fn a_policy_an_entity_still_relates_to_is_not_deleted() {
    // (case, path of the policy, how to relate it, how many entities, how to clear).
    let cases: [(&str, &str, &str, usize, Clear); 6] = [
        ("one role attachment", "/", "attach-role", 1, Clear::DetachRoles),
        (
            "three role attachments",
            "/service-role/",
            "attach-role",
            3,
            Clear::DetachRoles,
        ),
        ("one user attachment", "/", "attach-user", 1, Clear::Nothing),
        (
            "two user attachments",
            "/eng/tools/iam/",
            "attach-user",
            2,
            Clear::Nothing,
        ),
        ("a role's permissions boundary", "/", "role-boundary", 1, Clear::DeleteRole),
        (
            "a user's permissions boundary",
            "/boundaries/",
            "user-boundary",
            1,
            Clear::DeleteUser,
        ),
    ];

    for (case, path, relation, entities, clear) in cases {
        let router = router();
        let policy_arn = create_policy(&router, "related", path).await;
        let (status, created) = call(
            &router,
            "GetPolicy",
            &[("PolicyArn", policy_arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture policy must read back, got {status}: {created}"
        );
        let policy_id = element(&created, "PolicyId");

        // The entities the row relates the policy to, each created first.
        let mut names = Vec::new();
        for index in 0..entities {
            let name = format!("related-{index}");
            let (create_action, params): (&str, Vec<(String, String)>) = match relation {
                "attach-role" | "role-boundary" => (
                    "CreateRole",
                    vec![
                        param("RoleName", &name),
                        param("AssumeRolePolicyDocument", TRUST_POLICY),
                    ],
                ),
                _ => ("CreateUser", vec![param("UserName", &name)]),
            };
            let (status, body) = call(&router, create_action, &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture {create_action} must succeed, got {status}: {body}"
            );

            let (relate_action, params): (&str, Vec<(String, String)>) = match relation {
                "attach-role" => (
                    "AttachRolePolicy",
                    vec![param("RoleName", &name), param("PolicyArn", &policy_arn)],
                ),
                "attach-user" => (
                    "AttachUserPolicy",
                    vec![param("UserName", &name), param("PolicyArn", &policy_arn)],
                ),
                "role-boundary" => (
                    "PutRolePermissionsBoundary",
                    vec![
                        param("RoleName", &name),
                        param("PermissionsBoundary", &policy_arn),
                    ],
                ),
                _ => (
                    "PutUserPermissionsBoundary",
                    vec![
                        param("UserName", &name),
                        param("PermissionsBoundary", &policy_arn),
                    ],
                ),
            };
            let (status, body) = call(&router, relate_action, &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {relate_action} must succeed, got {status}: {body}"
            );
            names.push(name);
        }

        let (status, body) = call(
            &router,
            "DeletePolicy",
            &[("PolicyArn", policy_arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected DeletePolicy 409 while an entity still relates to the \
             policy, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "DeleteConflict",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );

        // The refused delete released nothing: the policy is still held, unchanged.
        let (status, body) = call(
            &router,
            "GetPolicy",
            &[("PolicyArn", policy_arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the policy must survive the refused delete, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "PolicyId"),
            policy_id,
            "case {case:?}: the refused delete changed the stored policy, got {body}"
        );

        // A policy nothing relates to, in the same account, is released as usual: it
        // is the RELATION that blocks the delete, not the account's state in general.
        let unrelated = create_policy(&router, "unrelated", "/").await;
        let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", unrelated.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a policy nothing relates to to be released, got {status}: \
             {body}"
        );

        // Once the relation is gone the SAME delete succeeds, so the refusal was the
        // relation and nothing else.
        let clearing: Vec<(&str, Vec<(String, String)>)> = match clear {
            Clear::DetachRoles => names
                .iter()
                .map(|name| {
                    (
                        "DetachRolePolicy",
                        vec![param("RoleName", name), param("PolicyArn", &policy_arn)],
                    )
                })
                .collect(),
            Clear::DeleteRole => names
                .iter()
                .map(|name| ("DeleteRole", vec![param("RoleName", name)]))
                .collect(),
            Clear::DeleteUser => names
                .iter()
                .map(|name| ("DeleteUser", vec![param("UserName", name)]))
                .collect(),
            Clear::Nothing => Vec::new(),
        };
        if clearing.is_empty() {
            continue;
        }
        for (action, params) in clearing {
            let (status, body) = call(&router, action, &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {action} must succeed, got {status}: {body}"
            );
        }
        let (status, body) = call(
            &router,
            "DeletePolicy",
            &[("PolicyArn", policy_arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeletePolicy 200 once nothing relates to the policy, \
             got {status}: {body}"
        );
        let (status, body) = call(
            &router,
            "GetPolicy",
            &[("PolicyArn", policy_arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: the released policy must resolve nowhere, got {status}: {body}"
        );
    }
}

/// RULE: an ARN in AWS's own managed-policy namespace (`arn:aws:iam::aws:policy/…`)
/// names a policy AWS owns, which an account cannot delete, so such an ARN is an
/// out-of-range value for this parameter — `InvalidInput` (HTTP 400) — and the catalog
/// entry is untouched.
///
/// The rows are entries of the AWS Managed Policy Reference this crate's catalog
/// carries (`state::AWS_MANAGED_POLICIES`, cross-read by
/// `tests/all/aws_managed_policy_catalog.rs`), at the root and at the `/service-role/`
/// path, plus one AWS-namespace ARN the catalog does not carry — the refusal does not
/// depend on whether the entry is carried.
#[tokio::test]
async fn an_aws_managed_policy_is_not_deleted_by_the_account() {
    let router = router();

    // (the ARN, whether this emulator's catalog carries that entry).
    let cases: [(&str, bool); 4] = [
        ("arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess", true),
        ("arn:aws:iam::aws:policy/AdministratorAccess", true),
        (
            "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole",
            true,
        ),
        ("arn:aws:iam::aws:policy/NotCarriedByThisCatalog", false),
    ];

    for (policy_arn, carried) in cases {
        let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", policy_arn)]).await;
        assert_eq!(
            status, 400,
            "case {policy_arn:?}: expected DeletePolicy 400 for an AWS-owned policy, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {policy_arn:?}: expected the modelled awsQuery code, got {body}"
        );
        let message = element(&body, "Message");
        assert!(
            message.contains(policy_arn),
            "case {policy_arn:?}: the refusal must name the policy it refuses, got {message:?}"
        );

        // A carried entry still reads back: the refused delete removed nothing from
        // the catalog every account sees.
        if carried {
            let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", policy_arn)]).await;
            assert_eq!(
                status, 200,
                "case {policy_arn:?}: the AWS-owned policy must survive the refused delete, \
                 got {status}: {body}"
            );
            assert_eq!(
                element(&body, "Arn"),
                policy_arn,
                "case {policy_arn:?}: expected the catalog entry's own ARN, got {body}"
            );
        }
    }
}

/// RULE: a released name is free again — a create may take it, and the new policy is a
/// NEW policy (its own minted id and its own members), not the released one coming
/// back.
///
/// The rows are the fixture policies, each released and then re-created under a
/// different path, so the re-created policy is built from the new request.
#[tokio::test]
async fn a_released_policy_name_can_be_created_again_as_a_new_policy() {
    let (router, arns) = with_fixture_policies().await;

    for (arn, (policy_name, path)) in arns.into_iter().zip(policies()) {
        let (status, before) = call(&router, "GetPolicy", &[("PolicyArn", arn.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: the fixture policy must read back, got {status}: {before}"
        );
        let first_id = element(&before, "PolicyId");

        let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", arn.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: expected DeletePolicy 200, got {status}: {body}"
        );

        // The name is free: a create takes it, under a path the released policy did
        // not carry, so the new record comes from the new request.
        let recreated = create_policy(&router, &policy_name, "/recreated/").await;
        let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", recreated.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: the re-created policy must read back, got {status}: {body}"
        );
        assert_ne!(
            element(&body, "PolicyId"),
            first_id,
            "case {policy_name:?}: expected a NEW policy id after the release, got the old \
             {first_id:?}"
        );
        assert_eq!(
            element(&body, "Path"),
            "/recreated/",
            "case {policy_name:?}: expected the re-created policy to carry the new path, not \
             the released {path:?}, got {body}"
        );
    }
}
