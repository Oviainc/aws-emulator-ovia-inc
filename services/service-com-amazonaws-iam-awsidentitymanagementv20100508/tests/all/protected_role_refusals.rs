// SPDX-License-Identifier: CC0-1.0

//! How this service treats a role a service links to ITSELF, end to end over the awsQuery
//! wire.
//!
//! Two rules are under test:
//!
//! 1. The protection covers such a role's PERMISSIONS, its TRUST relationship and its
//!    EXISTENCE, not the account's own descriptive record of it: a call that only replaces
//!    the description is served (`UpdateRole`, `UpdateRoleDescription`) and a later
//!    `GetRole` reports what it wrote, while a call that changes the role's policies, its
//!    trust policy, its instance-profile binding or its existence is refused with the
//!    modelled `UnmodifiableEntityException` (HTTP 400, awsQuery code `UnmodifiableEntity`)
//!    and changes nothing.
//! 2. That refusal is stated in ONE sentence, whatever operation was refused: fixed text
//!    apart from the role name, which is quoted exactly as the REQUEST spelled it — never
//!    the service the role is linked to, which the sentence does not mention.
//!
//! Each suite is a TABLE whose rows differ along what the rules generalise over: the
//! service the role is linked to (two- and three-label principals), whether the role
//! carries a custom suffix, the operation that was refused and the arguments it carries
//! beside the role name, the case the request spells the role's name in, and the
//! description a served update writes (empty, short, and the longest the member admits).
//! No single request is the specification.
//!
//! The awsQuery harness (`router`, `call`, `element`, `error_code`) is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// A trust policy with a single statement — an IAM policy document, which is all the
/// operations here ask of one.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// A permission policy document, for the operations that embed one.
const PERMISSION_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// An AWS-managed policy's ARN — the one form `AttachRolePolicy` and
/// `PutRolePermissionsBoundary` take without this account having to hold the policy.
const MANAGED_POLICY_ARN: &str = "arn:aws:iam::aws:policy/ReadOnlyAccess";

/// The sentence this service states a refused change to a protected role with. The role
/// name is the only variable part, and it is the one the REQUEST carried.
pub(crate) fn protected(role_name: &str) -> String {
    format!(
        "Cannot perform the operation on the protected role '{role_name}' - this role is \
         only modifiable by AWS"
    )
}

/// The role a service keeps for itself, created through `CreateServiceLinkedRole`: the
/// name AWS minted for it.
pub(crate) async fn service_linked_role(
    router: &MultiServiceRouter,
    principal: &str,
    custom_suffix: Option<&str>,
) -> String {
    let mut params = vec![("AWSServiceName", principal)];
    if let Some(custom_suffix) = custom_suffix {
        params.push(("CustomSuffix", custom_suffix));
    }
    let (status, body) = call(router, "CreateServiceLinkedRole", &params).await;
    assert_eq!(
        status, 200,
        "case {principal:?}: CreateServiceLinkedRole must succeed, got {status}: {body}"
    );
    element(&body, "RoleName")
}

/// The description `GetRole` reports for `role_name`, and the empty string when the role
/// carries none.
async fn reported_description(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = call(router, "GetRole", &[("RoleName", role_name)]).await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: GetRole must report the role, got {status}: {body}"
    );
    elements(&body, "Description")
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// The service principals (and optional custom suffixes) the rules are read over: a
/// two-label principal, a three-label one, and one whose role carries a custom suffix.
pub(crate) const LINKED_SERVICES: &[(&str, Option<&str>)] = &[
    ("autoscaling.amazonaws.com", None),
    ("ops.apigateway.amazonaws.com", None),
    ("elasticbeanstalk.amazonaws.com", Some("ingest")),
];

/// The longest description `roleDescriptionType` admits: its published length is 0..1000
/// (`smithy_query(query="operation", operation="UpdateRole")`).
const MAX_DESCRIPTION_CHARACTERS: usize = 1000;

/// RULE: a description is a member of the account's own record of a role, not one of the
/// permissions, trust or lifecycle a service-linked role protects — so `UpdateRole`
/// replaces it on such a role exactly as on any other, and `GetRole` reports what it
/// wrote.
#[tokio::test]
async fn update_role_replaces_a_service_linked_roles_description() {
    let router = router();

    // Rows spanning the length of the description: none at all, the empty string (the
    // floor of the published length), a sentence, and the longest the member admits.
    let longest = "d".repeat(MAX_DESCRIPTION_CHARACTERS);
    let descriptions = [
        ("the empty description", String::new()),
        ("a single character", "x".to_string()),
        (
            "a sentence",
            "Used by Auto Scaling on the account's behalf.".to_string(),
        ),
        ("the longest the member admits", longest),
    ];

    for (principal, custom_suffix) in LINKED_SERVICES {
        let role_name = service_linked_role(&router, principal, *custom_suffix).await;
        for (length_case, description) in &descriptions {
            let case = format!("{principal} / {length_case}");
            let params = vec![
                param("RoleName", &role_name),
                param("Description", description),
            ];
            let (status, body) = call(&router, "UpdateRole", &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a description change on a service-linked role must be \
                 served, got {status}: {body}"
            );
            assert!(
                elements(&body, "Code").is_empty(),
                "case {case:?}: a served update must carry no error code: {body}"
            );
            let read = reported_description(&router, &role_name).await;
            assert_eq!(
                read, *description,
                "case {case:?}: expected GetRole to report the written description, got \
                 {read:?}"
            );
        }

        // What makes the role the service's own is its PATH, which this update does not
        // touch.
        let (status, body) = call(&router, "GetRole", &[("RoleName", &role_name)]).await;
        assert_eq!(status, 200, "case {principal:?}: GetRole failed: {body}");
        let path = element(&body, "Path");
        assert_eq!(
            path,
            format!("/aws-service-role/{principal}/"),
            "case {principal:?}: expected the service-linked path to be untouched, got \
             {path:?}"
        );
    }
}

/// RULE: `UpdateRoleDescription` "performs the same function as the Description parameter
/// in the UpdateRole operation", so it is served for a service-linked role for the same
/// reason — and it reports the modified role, whose description is the one it wrote.
#[tokio::test]
async fn update_role_description_replaces_a_service_linked_roles_description() {
    let router = router();

    // Rows spanning the spelling of the role's name: as minted, upper-cased and
    // lower-cased (IAM entity names "are not distinguished by case").
    for (principal, custom_suffix) in LINKED_SERVICES {
        let minted = service_linked_role(&router, principal, *custom_suffix).await;
        let spellings = [
            ("as minted", minted.clone()),
            ("upper-cased", minted.to_uppercase()),
            ("lower-cased", minted.to_lowercase()),
        ];
        for (spelling_case, spelling) in spellings {
            let case = format!("{principal} / {spelling_case}");
            let description = format!("documented through {spelling_case}");
            let params = vec![
                param("RoleName", &spelling),
                param("Description", &description),
            ];
            let (status, body) = call(&router, "UpdateRoleDescription", &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a description change on a service-linked role must be \
                 served, got {status}: {body}"
            );
            assert_eq!(
                element(&body, "Description"),
                description,
                "case {case:?}: the reported role must carry the written description: {body}"
            );
            let read = reported_description(&router, &minted).await;
            assert_eq!(
                read, description,
                "case {case:?}: expected GetRole to report the written description, got \
                 {read:?}"
            );
        }
    }
}

/// One refused operation: the action, the parameters it carries beside `RoleName`, and
/// what the role must still look like afterwards is asserted by the caller.
struct ProtectedCall {
    case: &'static str,
    action: &'static str,
    /// Parameters other than `RoleName`, which the suite adds in the spelling under test.
    params: &'static [(&'static str, &'static str)],
}

/// The operations the protection covers: the role's attached and inline permissions, its
/// permissions boundary, its trust policy, its instance-profile binding and its
/// existence.
const PROTECTED_CALLS: &[ProtectedCall] = &[
    ProtectedCall {
        case: "attaching a managed policy",
        action: "AttachRolePolicy",
        params: &[("PolicyArn", MANAGED_POLICY_ARN)],
    },
    ProtectedCall {
        case: "detaching a managed policy",
        action: "DetachRolePolicy",
        params: &[("PolicyArn", MANAGED_POLICY_ARN)],
    },
    ProtectedCall {
        case: "embedding an inline policy",
        action: "PutRolePolicy",
        params: &[
            ("PolicyName", "Embedded"),
            ("PolicyDocument", PERMISSION_POLICY),
        ],
    },
    ProtectedCall {
        case: "deleting an inline policy",
        action: "DeleteRolePolicy",
        params: &[("PolicyName", "Embedded")],
    },
    ProtectedCall {
        case: "setting a permissions boundary",
        action: "PutRolePermissionsBoundary",
        params: &[("PermissionsBoundary", MANAGED_POLICY_ARN)],
    },
    ProtectedCall {
        case: "replacing the trust policy",
        action: "UpdateAssumeRolePolicy",
        params: &[("PolicyDocument", TRUST_POLICY)],
    },
    ProtectedCall {
        case: "binding the role into an instance profile",
        action: "AddRoleToInstanceProfile",
        params: &[("InstanceProfileName", "HoldingProfile")],
    },
    ProtectedCall {
        case: "taking the role out of an instance profile",
        action: "RemoveRoleFromInstanceProfile",
        params: &[("InstanceProfileName", "HoldingProfile")],
    },
    ProtectedCall {
        case: "deleting the role",
        action: "DeleteRole",
        params: &[],
    },
];

/// RULE: every call that changes a service-linked role's permissions, its trust policy or
/// its existence is refused 400 `UnmodifiableEntity`, and the message is the one fixed
/// sentence this service states that protection with — the role name quoted as the
/// request spelled it, and nothing of the service the role is linked to.
#[tokio::test]
async fn a_protected_change_is_refused_in_one_fixed_sentence() {
    let router = router();

    // The instance profile the binding rows name, so those rows are refused for the
    // role's protection rather than for a profile that does not exist.
    let (status, body) = call(
        &router,
        "CreateInstanceProfile",
        &[("InstanceProfileName", "HoldingProfile")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture CreateInstanceProfile must succeed, got {status}: {body}"
    );

    for (principal, custom_suffix) in LINKED_SERVICES {
        let minted = service_linked_role(&router, principal, *custom_suffix).await;
        // What the role carries before the refused calls: the policy its own service
        // attached when the role was created, which only that service controls.
        let (status, body) = call(
            &router,
            "ListAttachedRolePolicies",
            &[("RoleName", &minted)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {principal:?}: ListAttachedRolePolicies failed: {body}"
        );
        let attached_before = elements(&body, "PolicyArn");
        // Rows spanning the spelling the request names the role by: the refusal quotes
        // what the REQUEST carried, not what the store holds.
        let spellings = [
            ("as minted", minted.clone()),
            ("upper-cased", minted.to_uppercase()),
        ];
        for (spelling_case, spelling) in &spellings {
            for ProtectedCall {
                case,
                action,
                params,
            } in PROTECTED_CALLS
            {
                let case = format!("{principal} / {spelling_case} / {case}");
                let mut all = vec![param("RoleName", spelling)];
                for (name, value) in *params {
                    all.push(param(name, value));
                }
                let (status, body) = call(&router, action, &borrowed(&all)).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: a protected change must be refused, got {status}: {body}"
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
                    "case {case:?}: the sentence must not name the service the role is \
                     linked to, got {message:?}"
                );
            }
        }

        // Nothing the refused calls asked for happened: the role is still there, still at
        // the service's own path, and carries neither policies nor a boundary.
        let (status, body) = call(&router, "GetRole", &[("RoleName", &minted)]).await;
        assert_eq!(
            status, 200,
            "case {principal:?}: the refused deletes must have left the role, got \
             {status}: {body}"
        );
        assert!(
            elements(&body, "PermissionsBoundaryArn").is_empty(),
            "case {principal:?}: a refused put must persist no boundary: {body}"
        );
        let (status, body) = call(
            &router,
            "ListAttachedRolePolicies",
            &[("RoleName", &minted)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {principal:?}: ListAttachedRolePolicies failed: {body}"
        );
        // The role carries exactly what it carried before — the policy its own service
        // attached — and nothing the refused attach named.
        assert_eq!(
            elements(&body, "PolicyArn"),
            attached_before,
            "case {principal:?}: a refused attach must persist nothing: {body}"
        );
        assert!(
            !elements(&body, "PolicyArn").contains(&MANAGED_POLICY_ARN.to_string()),
            "case {principal:?}: a refused attach must persist nothing: {body}"
        );
    }
}

/// RULE: the protection is about the ROLE, not about the operations — so the same calls
/// are served on a role the account created for itself, with the same arguments the
/// protected rows carry.
#[tokio::test]
async fn the_same_calls_are_served_on_an_ordinary_role() {
    let router = router();

    let params = vec![
        param("RoleName", "OrdinaryRole"),
        param("AssumeRolePolicyDocument", TRUST_POLICY),
    ];
    let (status, body) = call(&router, "CreateRole", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "the fixture CreateRole must succeed, got {status}: {body}"
    );

    // Rows: embedding an inline policy, replacing the trust policy, setting a boundary
    // and attaching a managed policy — each refused on a service-linked role above.
    for ProtectedCall {
        case,
        action,
        params,
    } in PROTECTED_CALLS.iter().filter(|candidate| {
        candidate.action != "DeleteRole" && !candidate.action.ends_with("InstanceProfile")
    }) {
        let mut all = vec![param("RoleName", "OrdinaryRole")];
        for (name, value) in *params {
            all.push(param(name, value));
        }
        let (status, body) = call(&router, action, &borrowed(&all)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the call to be served on an ordinary role, got \
             {status}: {body}"
        );
    }
}
