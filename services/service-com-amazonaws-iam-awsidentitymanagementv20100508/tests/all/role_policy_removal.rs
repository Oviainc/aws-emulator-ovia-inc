// SPDX-License-Identifier: CC0-1.0

//! `DetachRolePolicy` and `DeleteRolePolicy`, end to end over the awsQuery wire.
//!
//! RULE: removing a subordinate entity from an IAM role CLEARS the state that records
//! the role's dependency on it — a detached managed policy stops being reported by
//! `ListAttachedRolePolicies`, a deleted inline policy stops resolving through
//! `GetRolePolicy` — so once the last dependent is removed the role holds nothing and
//! `DeleteRole` releases it with HTTP 200, where it answers the modelled
//! `DeleteConflict` (409) for as long as one dependent is still present.
//!
//! That is what makes the ordinary create-use-detach-delete teardown terminate: a
//! client that removes a role's policies and then deletes the role must not be told the
//! role still has subordinate entities.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: how
//! many dependents the role carries, which KIND each one is (an attached managed policy
//! or an embedded inline policy), the order the kinds are interleaved in, the path of
//! the policy's ARN, the case the removal spells the names in, and the form of a
//! refused ARN. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, so
//! every suite in this crate builds and reads a request the same way.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// A trust policy every fixture create sends: an IAM policy document, which is all
/// `CreateRole` requires of one.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"rds.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// An inline policy document: a JSON object carrying a `Statement`, which is what
/// `PutRolePolicy` requires of one.
const INLINE_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"secretsmanager:GetSecretValue","Resource":"*"}]}"#;

/// ARNs of AWS-managed policies, spanning the paths the managed-policy ARN grammar
/// admits: the account root, the `/service-role/` path AWS files the policies of a
/// service role under, and the `/job-function/` path. The names and paths are entries
/// of the AWS Managed Policy Reference; what the rule generalises over is the ARN's
/// SHAPE (path depth, name alphabet), which is why the rows differ in path.
const AWS_MANAGED_POLICIES: &[&str] = &[
    "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess",
    "arn:aws:iam::aws:policy/SecretsManagerReadWrite",
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole",
    "arn:aws:iam::aws:policy/service-role/AmazonRDSBetaServiceRolePolicy",
    "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
];

/// One subordinate entity a role can hold, and the operation that removes it:
/// `DetachRolePolicy` for a managed policy attached to the role, `DeleteRolePolicy` for
/// an inline policy embedded in it (the two kinds `DeleteRole`'s documentation tells a
/// caller to remove "before attempting to delete a role").
#[derive(Debug, Clone, Copy)]
enum Dependent {
    /// A managed policy, named by the ARN it is attached under.
    Managed(&'static str),
    /// An inline policy, named by the `PolicyName` it is embedded under.
    Inline(&'static str),
}

/// Create a role at `path`, which every case needs first.
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("Path", path),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?} at {path:?}: the fixture CreateRole must succeed, got {status}: {body}"
    );
}

/// Give `role_name` the dependent `dependent`, through the operation that creates it.
async fn add(router: &MultiServiceRouter, role_name: &str, dependent: Dependent) {
    let (status, body) = match dependent {
        Dependent::Managed(policy_arn) => {
            call(
                router,
                "AttachRolePolicy",
                &[("RoleName", role_name), ("PolicyArn", policy_arn)],
            )
            .await
        }
        Dependent::Inline(policy_name) => {
            call(
                router,
                "PutRolePolicy",
                &[
                    ("RoleName", role_name),
                    ("PolicyName", policy_name),
                    ("PolicyDocument", INLINE_POLICY),
                ],
            )
            .await
        }
    };
    assert_eq!(
        status, 200,
        "case {dependent:?} on {role_name:?}: the fixture must be able to add it, got \
         {status}: {body}"
    );
}

/// Remove `dependent` from the role the request spells `role_name`, answering the
/// status and body of the removal itself.
async fn remove(
    router: &MultiServiceRouter,
    role_name: &str,
    dependent: Dependent,
) -> (u16, String) {
    match dependent {
        Dependent::Managed(policy_arn) => {
            call(
                router,
                "DetachRolePolicy",
                &[("RoleName", role_name), ("PolicyArn", policy_arn)],
            )
            .await
        }
        Dependent::Inline(policy_name) => {
            call(
                router,
                "DeleteRolePolicy",
                &[("RoleName", role_name), ("PolicyName", policy_name)],
            )
            .await
        }
    }
}

/// Whether the role still depends on `dependent`, read through the operation that
/// REPORTS that kind: `ListAttachedRolePolicies` for a managed policy,
/// `GetRolePolicy` for an inline one.
async fn still_held(router: &MultiServiceRouter, role_name: &str, dependent: Dependent) -> bool {
    match dependent {
        Dependent::Managed(policy_arn) => {
            let (status, body) =
                call(router, "ListAttachedRolePolicies", &[("RoleName", role_name)]).await;
            assert_eq!(
                status, 200,
                "case {policy_arn:?}: ListAttachedRolePolicies must answer for a held role, \
                 got {status}: {body}"
            );
            elements(&body, "PolicyArn").iter().any(|arn| arn == policy_arn)
        }
        Dependent::Inline(policy_name) => {
            let (status, body) = call(
                router,
                "GetRolePolicy",
                &[("RoleName", role_name), ("PolicyName", policy_name)],
            )
            .await;
            assert!(
                status == 200 || status == 404,
                "case {policy_name:?}: GetRolePolicy must either report the policy or answer \
                 not-found, got {status}: {body}"
            );
            status == 200
        }
    }
}

/// RULE: a role is released exactly when the last of its dependents has been removed —
/// each removal clears that dependent's bookkeeping, `DeleteRole` answers
/// `DeleteConflict` (409) while any remains, and 200 once none does.
///
/// The rows differ in how many dependents the role carries (one to four), which kind
/// each is, the order the kinds are interleaved, and the path the role itself sits at.
/// Every other removal spells the role name in UPPER case, because IAM entity names
/// "are not distinguished by case".
#[tokio::test]
async fn removing_every_dependent_frees_the_role_for_deletion() {
    let cases: Vec<(&str, &str, Vec<Dependent>)> = vec![
        (
            "one attached managed policy",
            "/",
            vec![Dependent::Managed(AWS_MANAGED_POLICIES[0])],
        ),
        (
            "one embedded inline policy",
            "/",
            vec![Dependent::Inline("rds-proxy-secrets")],
        ),
        (
            "three attached managed policies, three ARN paths",
            "/service-role/not-reserved/",
            vec![
                Dependent::Managed(AWS_MANAGED_POLICIES[0]),
                Dependent::Managed(AWS_MANAGED_POLICIES[2]),
                Dependent::Managed(AWS_MANAGED_POLICIES[4]),
            ],
        ),
        (
            "two embedded inline policies",
            "/eng/tools/",
            vec![
                Dependent::Inline("read_secrets"),
                Dependent::Inline("write.logs@1"),
            ],
        ),
        (
            "both kinds, managed first",
            "/eng/",
            vec![
                Dependent::Managed(AWS_MANAGED_POLICIES[1]),
                Dependent::Inline("mixed-1"),
                Dependent::Managed(AWS_MANAGED_POLICIES[3]),
                Dependent::Inline("mixed+2"),
            ],
        ),
        (
            "both kinds, inline first",
            "/",
            vec![
                Dependent::Inline("inline=first"),
                Dependent::Managed(AWS_MANAGED_POLICIES[0]),
            ],
        ),
    ];

    for (case, path, dependents) in cases {
        let router = router();
        let role_name = format!("proxy-role-{}", case.len());
        create_role(&router, &role_name, path).await;
        for dependent in &dependents {
            add(&router, &role_name, *dependent).await;
        }

        let mut remaining: Vec<Dependent> = dependents.clone();
        for (index, dependent) in dependents.iter().enumerate() {
            // While a dependent is still present the role is NOT deleted: that is the
            // one thing `DeleteConflict` is for.
            let (status, body) = call(&router, "DeleteRole", &[("RoleName", role_name.as_str())]).await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected DeleteRole 409 while {} dependent(s) remain, got \
                 {status}: {body}",
                remaining.len()
            );
            assert_eq!(
                error_code(&body),
                "DeleteConflict",
                "case {case:?}: expected the modelled awsQuery code while dependents remain, \
                 got {body}"
            );

            // Every other removal spells the role name in another case, which names
            // the same role.
            let spelling = if index % 2 == 0 {
                role_name.clone()
            } else {
                role_name.to_ascii_uppercase()
            };
            let (status, body) = remove(&router, &spelling, *dependent).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected removing {dependent:?} (spelled {spelling:?}) to \
                 succeed, got {status}: {body}"
            );

            remaining.remove(0);
            assert!(
                !still_held(&router, &role_name, *dependent).await,
                "case {case:?}: {dependent:?} is still reported against the role after its \
                 removal"
            );
            for held in &remaining {
                assert!(
                    still_held(&router, &role_name, *held).await,
                    "case {case:?}: removing {dependent:?} also removed {held:?}"
                );
            }
        }

        // Nothing depends on the role any more, so the delete is the plain success
        // AWS answers: HTTP 200 and an empty response.
        let (status, body) = call(&router, "DeleteRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteRole 200 once every dependent was removed, got \
             {status}: {body}"
        );
        let (status, body) = call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the released role must resolve nowhere, got {status}: {body}"
        );
    }
}

/// RULE: a removal that names a role the account does not hold, or a dependent the role
/// does not carry, is refused with `NoSuchEntity` (HTTP 404) and changes nothing — so a
/// repeated removal is refused rather than reported as a second success.
#[tokio::test]
async fn removing_a_dependent_the_role_does_not_carry_is_not_found() {
    let router = router();
    create_role(&router, "carrier", "/").await;
    let carried = [
        Dependent::Managed(AWS_MANAGED_POLICIES[0]),
        Dependent::Inline("held-inline"),
    ];
    for dependent in carried {
        add(&router, "carrier", dependent).await;
    }

    // Rows: a role the account never held (three spellings), a managed policy the role
    // never carried (three ARN shapes), an inline name it never carried (three name
    // shapes), and each carried dependent removed a SECOND time.
    let mut rows: Vec<(String, Dependent)> = Vec::new();
    for absent_role in ["nobody", "carrie", "carriers"] {
        rows.push((
            absent_role.to_string(),
            Dependent::Managed(AWS_MANAGED_POLICIES[0]),
        ));
        rows.push((absent_role.to_string(), Dependent::Inline("held-inline")));
    }
    for policy_arn in &AWS_MANAGED_POLICIES[1..4] {
        rows.push(("carrier".to_string(), Dependent::Managed(policy_arn)));
    }
    for policy_name in ["absent", "held_inline", "held-inline-2"] {
        rows.push(("carrier".to_string(), Dependent::Inline(policy_name)));
    }

    for (role_name, dependent) in &rows {
        let (status, body) = remove(&router, role_name, *dependent).await;
        assert_eq!(
            status, 404,
            "case {role_name:?}/{dependent:?}: expected 404 for a dependent the role does not \
             carry, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {role_name:?}/{dependent:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // The refusals changed nothing: both carried dependents are still reported, and
    // the role is still held back from deletion by them.
    for dependent in carried {
        assert!(
            still_held(&router, "carrier", dependent).await,
            "case {dependent:?}: a refused removal dropped a dependent the role carries"
        );
    }
    let (status, body) = call(&router, "DeleteRole", &[("RoleName", "carrier")]).await;
    assert_eq!(
        status, 409,
        "the refused removals must leave the role's dependents in place, got {status}: {body}"
    );

    // A removal that DID remove something cannot remove it twice.
    for dependent in carried {
        let (status, body) = remove(&router, "carrier", dependent).await;
        assert_eq!(
            status, 200,
            "case {dependent:?}: the first removal must succeed, got {status}: {body}"
        );
        let (status, body) = remove(&router, "carrier", dependent).await;
        assert_eq!(
            status, 404,
            "case {dependent:?}: expected 404 for a repeated removal, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {dependent:?}: expected the modelled awsQuery code on the repeat, got {body}"
        );
    }
    let (status, body) = call(&router, "DeleteRole", &[("RoleName", "carrier")]).await;
    assert_eq!(
        status, 200,
        "expected the role to be released once both dependents were removed, got {status}: \
         {body}"
    );
}

/// RULE: a `PolicyArn` that is not an IAM managed policy's ARN is refused HTTP 400 — the
/// modelled `arnType` asks only for a 20..2048 character string, so the ARN's SHAPE is
/// what the operation checks — under the code of the layer that took the form:
/// `ValidationError` for a value that is not an ARN at all, the modelled `InvalidInput`
/// for an ARN naming another service or resource type.
///
/// The rows are strings long enough to pass `arnType`'s length constraint that name
/// something other than a managed policy: another IAM entity type, another service's
/// resource, a regional (so non-IAM-shaped) policy ARN, one with an empty path segment,
/// and a bare non-ARN.
#[tokio::test]
async fn a_policy_arn_that_names_no_managed_policy_is_invalid_input() {
    let router = router();
    create_role(&router, "arn-checks", "/").await;
    add(
        &router,
        "arn-checks",
        Dependent::Managed(AWS_MANAGED_POLICIES[0]),
    )
    .await;

    let malformed = [
        ("arn:aws:iam::123456789012:role/rds-proxy-role", "InvalidInput"),
        ("arn:aws:iam::123456789012:user/engineer-one", "InvalidInput"),
        ("arn:aws:s3:::rds-proxy-bucket-0001", "InvalidInput"),
        (
            "arn:aws:iam:us-east-1:123456789012:policy/Regional",
            "InvalidInput",
        ),
        (
            "arn:aws:iam::123456789012:policy//EmptySegment",
            "InvalidInput",
        ),
        ("AmazonS3ReadOnlyAccess-but-not-an-arn", "ValidationError"),
    ];
    for (policy_arn, expected_code) in malformed {
        let (status, body) = call(
            &router,
            "DetachRolePolicy",
            &[("RoleName", "arn-checks"), ("PolicyArn", policy_arn)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {policy_arn:?}: expected 400 for an ARN that names no managed policy, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {policy_arn:?}: expected the code {expected_code}, got {body}"
        );
    }

    // The refusals detached nothing: the real attachment is still reported, and the
    // ARN that DOES name it is still detachable.
    assert!(
        still_held(
            &router,
            "arn-checks",
            Dependent::Managed(AWS_MANAGED_POLICIES[0])
        )
        .await,
        "a refused detach dropped the attachment the role carries"
    );
    let (status, body) = remove(
        &router,
        "arn-checks",
        Dependent::Managed(AWS_MANAGED_POLICIES[0]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the well-formed ARN must still detach the policy, got {status}: {body}"
    );
}

/// RULE: a service-linked role is a protected resource — "only the service that depends
/// on the service-linked role can modify or delete the role on your behalf" — so a
/// caller's removal of one of its policies is refused with `UnmodifiableEntity` (HTTP
/// 400), while the same removal on an ordinary role of the account succeeds.
///
/// The rows are service principals whose service-linked role paths differ in label
/// count and case, plus one created with a `CustomSuffix`, so the refusal is asserted on
/// the role's PATH rather than on one derived name.
#[tokio::test]
async fn a_service_linked_roles_policies_are_not_removed_by_its_account() {
    // (the service principal, the optional CustomSuffix) — the derived role name is
    // read out of the create's own response, as `tests/all/service_linked_roles.rs`
    // pins the derivation.
    let cases: [(&str, Option<&str>); 3] = [
        ("networkmonitor.amazonaws.com", None),
        ("ops.apigateway.amazonaws.com", None),
        ("autoscaling.amazonaws.com", Some("debug")),
    ];

    for (principal, suffix) in cases {
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
        let role_name = element(&created, "RoleName");

        for dependent in [
            Dependent::Managed(AWS_MANAGED_POLICIES[2]),
            Dependent::Inline("whatever"),
        ] {
            let (status, body) = remove(&router, &role_name, dependent).await;
            assert_eq!(
                status, 400,
                "case {principal:?}/{dependent:?}: expected 400 for a service-linked role, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnmodifiableEntity",
                "case {principal:?}/{dependent:?}: expected the modelled awsQuery code, got \
                 {body}"
            );
        }

        // The refusals left the protected role exactly as it was.
        let (status, body) = call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {principal:?}: the protected role must survive the refusals, got {status}: \
             {body}"
        );

        // An ordinary role of the same account HAS its policies removed: it is the
        // role's service-linked path that protects it, not the account's state.
        create_role(&router, "ordinary", "/").await;
        for dependent in [
            Dependent::Managed(AWS_MANAGED_POLICIES[2]),
            Dependent::Inline("whatever"),
        ] {
            add(&router, "ordinary", dependent).await;
            let (status, body) = remove(&router, "ordinary", dependent).await;
            assert_eq!(
                status, 200,
                "case {principal:?}/{dependent:?}: expected an ordinary role's dependent to be \
                 removed, got {status}: {body}"
            );
        }
        let (status, body) = call(&router, "DeleteRole", &[("RoleName", "ordinary")]).await;
        assert_eq!(
            status, 200,
            "case {principal:?}: expected the ordinary role to be released, got {status}: {body}"
        );
    }
}
