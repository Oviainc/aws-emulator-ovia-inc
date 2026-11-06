// SPDX-License-Identifier: CC0-1.0

//! How an ARN in AWS's own managed-policy namespace (`arn:aws:iam::aws:policy/...`) is
//! RESOLVED by the operations that take one, over the awsQuery wire.
//!
//! The rule being asserted: a managed policy's identity is its PATH and its friendly
//! name together — the two its ARN spells — and a friendly name identifies one policy in
//! the namespace that owns it, so an ARN spelling a policy the catalog carries under any
//! OTHER path names no policy at all and can neither be attached, bound, listed nor read.
//! The catalog is a documented SUBSET of AWS's (which runs to well over a thousand
//! policies), so a name it carries no entry for cannot be judged absent and is still
//! accepted — what the catalog knows, it is authoritative about, including where the
//! policy it knows lives.
//!
//! The second rule here is the FORM of such a member: a value that is not an ARN at all
//! is refused with the code IAM answers a wrong form with (`ValidationError`), while an
//! ARN that is one but names another service or resource type is the operation's own
//! modelled `InvalidInput`; both carry IAM's one sentence for an unusable ARN.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, message, XML members) a client on the
//! socket sees. The tables span what the rules generalise over: the catalog entry (every
//! one of them, not the entries a fixture cited), the path the ARN misspells, the kind of
//! identity the call addresses (role, user, group), the operation family (attach, detach,
//! permissions boundary, read) and the way a value can fail to be a usable ARN.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// A trust policy with a single statement — every fixture role needs one to exist.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The paths AWS publishes managed policies under, as the catalog carries them: the
/// account root, the path reserved for service roles, the one reserved for
/// service-linked roles, and the job-function family. Written out here so a row's
/// misspelled path comes from the published set rather than from the entry under test.
const PUBLISHED_PATHS: &[&str] = &["/", "/service-role/", "/aws-service-role/", "/job-function/"];

/// A friendly name no entry of the catalog carries, under each published path: the
/// subset caveat's rows, which must still be accepted.
const UNCARRIED_NAME: &str = "AmazonSimparityNeverPublishedPolicy";

/// Create a role at the account root.
async fn create_role(router: &MultiServiceRouter, role_name: &str) {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
}

/// Create a user at the account root.
async fn create_user(router: &MultiServiceRouter, user_name: &str) {
    let (status, body) = call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: CreateUser must succeed, got {status}: {body}"
    );
}

/// Create a group at the account root.
async fn create_group(router: &MultiServiceRouter, group_name: &str) {
    let (status, body) = call(router, "CreateGroup", &[("GroupName", group_name)]).await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: CreateGroup must succeed, got {status}: {body}"
    );
}

/// One `AttachRolePolicy`.
async fn attach_role_policy(
    router: &MultiServiceRouter,
    role_name: &str,
    policy_arn: &str,
) -> (u16, String) {
    call(
        router,
        "AttachRolePolicy",
        &[("RoleName", role_name), ("PolicyArn", policy_arn)],
    )
    .await
}

/// The `PolicyArn`s `ListAttachedRolePolicies` reports for `role_name`, in the order it
/// reports them.
async fn attached_role_policies(router: &MultiServiceRouter, role_name: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "ListAttachedRolePolicies",
        &[("RoleName", role_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: ListAttachedRolePolicies must succeed, got {status}: {body}"
    );
    elements(&body, "PolicyArn")
}

/// IAM's `NoSuchEntityException` message for a policy ARN an attachment call can resolve
/// nothing under. SOURCE: the recorded refusal of `AttachRolePolicy` — fixed text
/// `Policy ` and ` does not exist or is not attachable.` around the ARN supplied.
fn not_attachable(policy_arn: &str) -> String {
    format!("Policy {policy_arn} does not exist or is not attachable.")
}

/// IAM's `NoSuchEntityException` message for a permissions-boundary put whose ARN names
/// no policy: a boundary call names a SCOPE, which IAM words `Scope ARN: <arn> …`.
fn scope_not_attachable(policy_arn: &str) -> String {
    format!("Scope ARN: {policy_arn} does not exist or is not attachable.")
}

/// IAM's one sentence for a value a member that must BE an ARN cannot use.
fn arn_not_valid(value: &str) -> String {
    format!("ARN {value} is not valid.")
}

/// Every ARN that spells a carried policy's friendly name under a path AWS does NOT
/// publish it at, paired with the entry it misspells: the table the path rule is
/// asserted over, one row per entry per other published path.
fn misspelled_paths() -> Vec<(&'static state::AwsManagedPolicy, String)> {
    let mut rows = Vec::new();
    for entry in state::AWS_MANAGED_POLICIES {
        for path in PUBLISHED_PATHS {
            if *path == entry.path {
                continue;
            }
            rows.push((
                entry,
                format!("arn:aws:iam::aws:policy{path}{}", entry.policy_name),
            ));
        }
    }
    rows
}

/// RULE: an AWS-managed policy ARN names a policy only under the ONE path AWS publishes
/// that policy at, so the same friendly name spelled under any other published path
/// names no policy and `AttachRolePolicy` refuses it as the missing entity — leaving the
/// role's attachments exactly as they were.
#[tokio::test]
async fn attaching_a_carried_policy_under_another_path_is_refused() {
    let router = router();
    let role = "PathResolutionRole";
    create_role(&router, role).await;

    let rows = misspelled_paths();
    assert!(
        rows.len() >= state::AWS_MANAGED_POLICIES.len(),
        "the table must carry at least one misspelled path per catalog entry, got {} rows \
         for {} entries",
        rows.len(),
        state::AWS_MANAGED_POLICIES.len()
    );

    for (entry, misspelled) in &rows {
        let case = misspelled.clone();
        let (status, body) = attach_role_policy(&router, role, misspelled).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an ARN spelling {:?} under a path AWS does not publish it at \
             names no policy, so the attach must answer 404, got {status}: {body}",
            entry.policy_name
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: the refusal must be NoSuchEntity, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            not_attachable(misspelled),
            "case {case:?}: expected IAM's attachment-family wording, got {:?}",
            element(&body, "Message")
        );
    }

    let attached = attached_role_policies(&router, role).await;
    assert!(
        attached.is_empty(),
        "every attach above was refused, so the role must carry nothing, got {attached:?}"
    );
}

/// RULE (the other half): the ARN AWS publishes the policy at DOES resolve, so the same
/// name at its own path attaches and is reported back — once, under the ARN it was
/// attached with. Asserted over every attachable catalog entry, so the refusal above is
/// the path's doing and not a blanket refusal of the namespace.
#[tokio::test]
async fn attaching_a_carried_policy_at_its_published_path_is_served() {
    let router = router();

    for (index, entry) in state::AWS_MANAGED_POLICIES.iter().enumerate() {
        if !entry.is_attachable() {
            continue;
        }
        // A role of its own per row, so one row's attachment is never read as another's.
        let role = format!("published-{index}");
        create_role(&router, &role).await;
        let arn = entry.arn();
        let case = arn.clone();

        let (status, body) = attach_role_policy(&router, &role, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the ARN AWS publishes the policy at must attach, got {status}: \
             {body}"
        );

        let attached = attached_role_policies(&router, &role).await;
        assert_eq!(
            attached,
            vec![arn.clone()],
            "case {case:?}: the role must carry exactly the ARN it was attached with, got \
             {attached:?}"
        );
    }
}

/// RULE: the catalog is a documented SUBSET of AWS's, so an ARN whose friendly name it
/// carries NO entry for cannot be judged absent: it is accepted under every published
/// path, because refusing it would refuse an attachment AWS serves.
#[tokio::test]
async fn attaching_a_name_the_catalog_does_not_carry_is_accepted() {
    let router = router();
    assert!(
        state::aws_managed_policy_named(UNCARRIED_NAME).is_none(),
        "the row's name must be one the catalog carries no entry for, so the subset \
         caveat is what this asserts"
    );

    for (index, path) in PUBLISHED_PATHS.iter().enumerate() {
        let arn = format!("arn:aws:iam::aws:policy{path}{UNCARRIED_NAME}");
        let case = arn.clone();
        let role = format!("uncarried-{index}");
        create_role(&router, &role).await;

        let (status, body) = attach_role_policy(&router, &role, &arn).await;
        // The path AWS reserves for service-linked roles is the one exception, and not a
        // resolution one: a policy filed there belongs to that role alone, so it is
        // refused as a policy that may not be attached rather than as one that is absent.
        if state::is_service_linked_role_path(path) {
            assert_eq!(
                status, 400,
                "case {case:?}: a policy under the path reserved for service-linked roles \
                 may not be attached, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "PolicyNotAttachable",
                "case {case:?}: expected the reserved-policy refusal, got {:?}: {body}",
                error_code(&body)
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: a name the catalog carries no entry for cannot be judged \
             absent, so the attach must be served, got {status}: {body}"
        );
        let attached = attached_role_policies(&router, &role).await;
        assert_eq!(
            attached,
            vec![arn.clone()],
            "case {case:?}: the role must carry the ARN it was attached with, got \
             {attached:?}"
        );
    }
}

/// RULE: it is ONE resolution, so every member taking a managed policy's ARN answers the
/// same way — the three attach calls, the two permissions-boundary puts and a read of the
/// policy itself all refuse the misspelled path, each with the sentence its own family
/// words that absence in.
#[tokio::test]
async fn every_member_taking_a_policy_arn_resolves_the_path_alike() {
    let router = router();
    create_role(&router, "SharedRule").await;
    create_user(&router, "SharedRuleUser").await;
    create_group(&router, "SharedRuleGroup").await;

    // One entry per published path the catalog uses, so the misspelled row for each is a
    // different path — and the entry under test is not one the recorded suite cited.
    let carried = ["AmazonS3ReadOnlyAccess", "AWSLambdaBasicExecutionRole"];
    for name in carried {
        let entry = state::aws_managed_policy_named(name)
            .unwrap_or_else(|| panic!("the catalog must carry {name:?} for this table"));
        for path in PUBLISHED_PATHS {
            if *path == entry.path || state::is_service_linked_role_path(path) {
                continue;
            }
            let arn = format!("arn:aws:iam::aws:policy{path}{name}");
            let case = arn.clone();

            for (action, params, expected) in [
                (
                    "AttachRolePolicy",
                    vec![("RoleName", "SharedRule"), ("PolicyArn", arn.as_str())],
                    not_attachable(&arn),
                ),
                (
                    "AttachUserPolicy",
                    vec![("UserName", "SharedRuleUser"), ("PolicyArn", arn.as_str())],
                    not_attachable(&arn),
                ),
                (
                    "AttachGroupPolicy",
                    vec![
                        ("GroupName", "SharedRuleGroup"),
                        ("PolicyArn", arn.as_str()),
                    ],
                    not_attachable(&arn),
                ),
                (
                    "PutRolePermissionsBoundary",
                    vec![
                        ("RoleName", "SharedRule"),
                        ("PermissionsBoundary", arn.as_str()),
                    ],
                    scope_not_attachable(&arn),
                ),
                (
                    "PutUserPermissionsBoundary",
                    vec![
                        ("UserName", "SharedRuleUser"),
                        ("PermissionsBoundary", arn.as_str()),
                    ],
                    scope_not_attachable(&arn),
                ),
                (
                    "ListEntitiesForPolicy",
                    vec![("PolicyArn", arn.as_str())],
                    not_attachable(&arn),
                ),
            ] {
                let (status, body) = call(&router, action, &params).await;
                assert_eq!(
                    status, 404,
                    "case {action} {case:?}: a misspelled path names no policy, so the \
                     answer must be 404, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "NoSuchEntity",
                    "case {action} {case:?}: expected NoSuchEntity, got {:?}: {body}",
                    error_code(&body)
                );
                assert_eq!(
                    element(&body, "Message"),
                    expected,
                    "case {action} {case:?}: expected {expected:?}, got {:?}",
                    element(&body, "Message")
                );
            }

            // A READ of the policy resource resolves the same ARN and reports the same
            // absence, in the wording its own family uses.
            let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", arn.as_str())]).await;
            assert_eq!(
                status, 404,
                "case GetPolicy {case:?}: a misspelled path names no policy to report, got \
                 {status}: {body}"
            );
            assert_eq!(
                element(&body, "Message"),
                format!("Policy {arn} was not found."),
                "case GetPolicy {case:?}: expected IAM's missing-policy wording, got {:?}",
                element(&body, "Message")
            );
        }
    }
}

/// RULE: a member that must BE an ARN is taken in two layers, and both answer HTTP 400
/// with IAM's one sentence for an unusable ARN: a value that is not an ARN at all carries
/// the code IAM answers a wrong form with (`ValidationError`), while an ARN that names
/// another service or another resource type carries the operation's own modelled
/// `InvalidInput`. Nothing is attached either way.
#[tokio::test]
async fn a_value_that_cannot_be_a_policy_arn_is_refused_by_its_layer() {
    let router = router();
    let role = "ArnFormRole";
    create_role(&router, role).await;

    // Rows span both layers and, inside each, the way the value fails: a bare friendly
    // name (the form a caller reaches for when it forgets the ARN), a name with the
    // namespace but no `arn:` frame, another service's ARN, another IAM resource type,
    // a regional IAM ARN (IAM is global) and an empty path segment. Every row is at
    // least 20 characters, `arnType`'s floor, so the handler is what refuses it.
    let rows = [
        ("AmazonEKSClusterPolicy", "ValidationError"),
        ("iam::aws:policy/AmazonS3ReadOnlyAccess", "ValidationError"),
        ("arn:aws:s3:::a-bucket-named-here", "InvalidInput"),
        ("arn:aws:iam::aws:role/NotAPolicyAtAll", "InvalidInput"),
        (
            "arn:aws:iam:us-east-1:aws:policy/AdministratorAccess",
            "InvalidInput",
        ),
        ("arn:aws:iam::aws:policy//EmptySegment", "InvalidInput"),
    ];
    for (value, expected_code) in rows {
        assert!(
            value.len() >= 20,
            "case {value:?}: a row must satisfy arnType's 20-character minimum"
        );
        let (status, body) = attach_role_policy(&router, role, value).await;
        assert_eq!(
            status, 400,
            "case {value:?}: a value that cannot be a managed-policy ARN must answer 400, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {value:?}: expected the code {expected_code}, got {:?}: {body}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            arn_not_valid(value),
            "case {value:?}: expected IAM's unusable-ARN sentence, got {:?}",
            element(&body, "Message")
        );
    }

    let attached = attached_role_policies(&router, role).await;
    assert!(
        attached.is_empty(),
        "a refused attach writes nothing, so the role must carry nothing, got {attached:?}"
    );
}
