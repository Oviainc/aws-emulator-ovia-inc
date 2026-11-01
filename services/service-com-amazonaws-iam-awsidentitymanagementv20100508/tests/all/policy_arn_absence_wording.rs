// SPDX-License-Identifier: CC0-1.0

//! The sentence EVERY operation addressed by a `PolicyArn` refuses an ARN the account
//! holds no managed policy under with, end to end over the awsQuery wire.
//!
//! The rule under test: an operation addressed by a `PolicyArn` that resolves no
//! managed policy answers `404` `NoSuchEntity` whose message is the ARN the request
//! supplied, echoed byte for byte, between its OPERATION FAMILY's fixed text — `Policy `
//! and ` was not found.` for the operations acting on the policy RESOURCE (read and
//! write alike), `Policy ` and ` does not exist or is not attachable.` for the operations
//! whose subject is the ATTACHMENT of that policy to an entity, among them the create of
//! the version that takes effect for the identities the policy is attached to. The
//! sentence belongs to the family, not to the handler, so no operation may compose one of
//! its own.
//!
//! This is the one place the whole `PolicyArn`-addressed surface is held to that rule at
//! once, which is what keeps a single handler's wording from drifting while its family's
//! stays right: `tests/all/policy_refusal_wording.rs` pins the two families on the
//! reporting operations and the two implemented attach calls, and the per-operation
//! suites (`tests/all/policy_version_creation.rs`, `tests/all/policy_default_version.rs`,
//! `tests/all/policy_tag_removal.rs`, `tests/all/policy_deletion.rs`) each pin their own;
//! here every operation taking that member is driven through the same table, including
//! `AttachGroupPolicy`, and the ARN is asserted to come back exactly as supplied —
//! including its CASE, which is part of a managed policy's identity.
//!
//! `PutRolePermissionsBoundary` and `PutUserPermissionsBoundary` are not here: they
//! address a policy by `PermissionsBoundary` rather than `PolicyArn` and word the same
//! absence as a scope (`tests/all/fixed_refusal_wording.rs`).
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, `Message`) of a refusal reached through
//! the wire, never a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.
//!
//! The rows differ along what the rule generalises over — which operation of which family
//! is called, and the form of the ARN that resolves to nothing (a name nothing was created
//! under, the held policy's name at another path, at the account root, in another casing,
//! in another account, in another partition, a deeply nested path, a name spelling the
//! punctuation the name alphabet admits) — so no single request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s own
/// modelled example carries. The held fixture policy needs one, and so does the
/// `CreatePolicyVersion` row, whose document must be a policy before the ARN is looked
/// up at all.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy with a single statement, which the fixture role needs.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"config.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The group the `AttachGroupPolicy` row names, so only the POLICY can refuse it.
const GROUP_NAME: &str = "AbsenceWordingGroup";

/// The role the `AttachRolePolicy` row names.
const ROLE_NAME: &str = "AbsenceWordingRole";

/// The user the `AttachUserPolicy` row names.
const USER_NAME: &str = "absence-wording-user";

/// The policy the account DOES hold: the control that keeps every refusal below about
/// the ARN asked for rather than about an empty store.
const HELD_NAME: &str = "AbsenceHeldPolicy";

/// The path the held policy is created at.
const HELD_PATH: &str = "/held/";

/// Which sentence an operation words an unresolvable `PolicyArn` with.
#[derive(Clone, Copy, Debug)]
enum Family {
    /// The operations acting on the managed policy RESOURCE — read it, list what it
    /// holds, tag it, delete it.
    Resource,
    /// The operations whose subject is whether the policy can be, or already is,
    /// attached to an entity — among them the create of the version that takes effect for
    /// the identities it is attached to.
    Attachment,
}

impl Family {
    /// IAM's message for `arn` resolving to no managed policy, for this family.
    fn message(self, arn: &str) -> String {
        match self {
            Family::Resource => format!("Policy {arn} was not found."),
            Family::Attachment => format!("Policy {arn} does not exist or is not attachable."),
        }
    }
}

/// One operation addressed by a `PolicyArn`, its family, and the other members the
/// request must carry for the ARN to be the only thing that can refuse it.
struct PolicyArnCall {
    action: &'static str,
    family: Family,
    extra: &'static [(&'static str, &'static str)],
}

/// Every operation of this service whose input carries a `PolicyArn`
/// (`smithy_query(query="digest")`, the members named `PolicyArn`), with the family whose
/// sentence it must word an unresolvable one with.
const POLICY_ARN_CALLS: &[PolicyArnCall] = &[
    PolicyArnCall {
        action: "GetPolicy",
        family: Family::Resource,
        extra: &[],
    },
    PolicyArnCall {
        action: "GetPolicyVersion",
        family: Family::Resource,
        extra: &[("VersionId", "v1")],
    },
    PolicyArnCall {
        action: "ListPolicyVersions",
        family: Family::Resource,
        extra: &[],
    },
    PolicyArnCall {
        action: "ListPolicyTags",
        family: Family::Resource,
        extra: &[],
    },
    PolicyArnCall {
        action: "CreatePolicyVersion",
        family: Family::Attachment,
        extra: &[("PolicyDocument", POLICY_DOCUMENT)],
    },
    PolicyArnCall {
        action: "SetDefaultPolicyVersion",
        family: Family::Resource,
        extra: &[("VersionId", "v1")],
    },
    PolicyArnCall {
        action: "DeletePolicyVersion",
        family: Family::Resource,
        extra: &[("VersionId", "v1")],
    },
    PolicyArnCall {
        action: "TagPolicy",
        family: Family::Resource,
        extra: &[("Tags.member.1.Key", "team"), ("Tags.member.1.Value", "sre")],
    },
    PolicyArnCall {
        action: "UntagPolicy",
        family: Family::Resource,
        extra: &[("TagKeys.member.1", "team")],
    },
    PolicyArnCall {
        action: "DeletePolicy",
        family: Family::Resource,
        extra: &[],
    },
    PolicyArnCall {
        action: "ListEntitiesForPolicy",
        family: Family::Attachment,
        extra: &[],
    },
    PolicyArnCall {
        action: "AttachGroupPolicy",
        family: Family::Attachment,
        extra: &[("GroupName", GROUP_NAME)],
    },
    PolicyArnCall {
        action: "AttachRolePolicy",
        family: Family::Attachment,
        extra: &[("RoleName", ROLE_NAME)],
    },
    PolicyArnCall {
        action: "AttachUserPolicy",
        family: Family::Attachment,
        extra: &[("UserName", USER_NAME)],
    },
];

/// Create the fixture group, role and user the attachment rows name, plus the one managed
/// policy the account holds. Answers the router and the held policy's ARN.
async fn fixture() -> (MultiServiceRouter, String) {
    let router = router();

    let (status, body) = call(&router, "CreateGroup", &[("GroupName", GROUP_NAME)]).await;
    assert_eq!(
        status, 200,
        "CreateGroup must succeed for the fixture, got {status}: {body}"
    );

    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("Path", "/"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateRole must succeed for the fixture, got {status}: {body}"
    );

    let (status, body) = call(
        &router,
        "CreateUser",
        &[("UserName", USER_NAME), ("Path", "/")],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateUser must succeed for the fixture, got {status}: {body}"
    );

    let (status, body) = call(
        &router,
        "CreatePolicy",
        &[
            ("PolicyName", HELD_NAME),
            ("Path", HELD_PATH),
            ("PolicyDocument", POLICY_DOCUMENT),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePolicy must succeed for the fixture, got {status}: {body}"
    );
    (router, element(&body, "Arn"))
}

/// Drive one `PolicyArnCall` against `policy_arn`.
async fn policy_arn_call(
    router: &MultiServiceRouter,
    call_under_test: &PolicyArnCall,
    policy_arn: &str,
) -> (u16, String) {
    let mut params = vec![param("PolicyArn", policy_arn)];
    for (member, value) in call_under_test.extra {
        params.push(param(member, value));
    }
    call(router, call_under_test.action, &borrowed(&params)).await
}

/// Every well-formed managed-policy ARN this account still holds no policy under:
/// `(what the row is, the ARN)`.
fn arns_the_account_holds_nothing_under(account: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "a name nothing was created under",
            format!("arn:aws:iam::{account}:policy/NeverCreated"),
        ),
        (
            "the held name at another path",
            format!("arn:aws:iam::{account}:policy/elsewhere/{HELD_NAME}"),
        ),
        (
            "the held name at the account root",
            format!("arn:aws:iam::{account}:policy/{HELD_NAME}"),
        ),
        (
            // A managed policy's identity carries the case its name was created with,
            // so the same spelling in another case names no policy — and the refusal
            // must echo the case the REQUEST used.
            "the held ARN in another casing",
            format!(
                "arn:aws:iam::{account}:policy{}{}",
                HELD_PATH.to_ascii_lowercase(),
                HELD_NAME.to_ascii_lowercase()
            ),
        ),
        (
            "the held name in another account",
            format!("arn:aws:iam::210987654321:policy{HELD_PATH}{HELD_NAME}"),
        ),
        (
            "the held name in another partition",
            format!("arn:aws-us-gov:iam::{account}:policy{HELD_PATH}{HELD_NAME}"),
        ),
        (
            "a deeply nested path",
            format!("arn:aws:iam::{account}:policy/app/audit/2026/Ghost"),
        ),
        (
            // `policyNameType`'s pattern, `^[\w+=,.@-]+$`
            // (`smithy_query(query="shape", shape="policyNameType")`).
            "a name of the punctuation the name alphabet admits",
            format!("arn:aws:iam::{account}:policy/ghost.name+2=x@y_-"),
        ),
    ]
}

/// RULE: every operation addressed by a `PolicyArn` that resolves no managed policy
/// refuses the request with `404` `NoSuchEntity` whose message is the ARN exactly as
/// supplied between its family's fixed text — ` was not found.` for the policy resource,
/// ` does not exist or is not attachable.` for an attachment — and writes nothing.
#[tokio::test]
async fn an_unresolvable_policy_arn_is_refused_with_its_family_sentence() {
    let (router, held) = fixture().await;
    let account = account_id();
    // Bound once, so the refusal pass and the no-write pass below are provably over the
    // same rows.
    let unresolvable = arns_the_account_holds_nothing_under(account);

    for (what, arn) in &unresolvable {
        for call_under_test in POLICY_ARN_CALLS {
            let action = call_under_test.action;
            let case = format!("{action} on {what}");
            let (status, body) = policy_arn_call(&router, call_under_test, arn).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {code:?}: {body}"
            );
            let message = element(&body, "Message");
            // The one variable part is the ARN the request supplied, echoed as it was
            // supplied: this equality is exact, so it also pins that neither the case
            // nor the path of the ARN is normalised on the way into the message.
            assert_eq!(
                message,
                call_under_test.family.message(arn),
                "case {case:?}: expected the {:?} family's sentence, got {message:?}",
                call_under_test.family
            );
        }
    }

    // None of the refused calls above wrote anything: the ARNs that resolved to nothing
    // still resolve to nothing, and so a later reader sees exactly the store the fixture
    // left.
    for (what, arn) in &unresolvable {
        let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", arn)]).await;
        assert_eq!(
            status, 404,
            "case {what:?}: a refused call must create no policy, got {status}: {body}"
        );
    }

    // And the policy the account DOES hold is untouched by the refusals: it still carries
    // the one version the create gave it, as its default, with no tag and no attachment.
    let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", &held)]).await;
    assert_eq!(
        status, 200,
        "the held policy must still be served, got {status}: {body}"
    );
    let default_version = element(&body, "DefaultVersionId");
    assert_eq!(
        default_version, "v1",
        "the held policy's default version must survive the refusals, got \
         {default_version:?}: {body}"
    );
    let attachment_count = element(&body, "AttachmentCount");
    assert_eq!(
        attachment_count, "0",
        "no refused attach may have attached the held policy, got {attachment_count:?}: {body}"
    );

    let (status, body) = call(&router, "ListPolicyVersions", &[("PolicyArn", &held)]).await;
    assert_eq!(
        status, 200,
        "the held policy's versions must still be listed, got {status}: {body}"
    );
    let versions = elements(&body, "VersionId");
    assert_eq!(
        versions,
        vec!["v1".to_string()],
        "no refused CreatePolicyVersion or DeletePolicyVersion may have changed the held \
         policy's versions, got {versions:?}"
    );

    let (status, body) = call(&router, "ListPolicyTags", &[("PolicyArn", &held)]).await;
    assert_eq!(
        status, 200,
        "the held policy's tags must still be listed, got {status}: {body}"
    );
    let keys = elements(&body, "Key");
    assert!(
        keys.is_empty(),
        "no refused TagPolicy may have tagged the held policy, got {keys:?}"
    );
}

/// RULE (the control): the SAME operations serve the ARN the account does hold, so the
/// refusals above are about the policy the ARN names and not about the family of the
/// call — and the attachment half, once it has attached the held policy, is what the
/// reporting half then relates.
#[tokio::test]
async fn the_held_policy_arn_is_served_by_every_family() {
    let (router, held) = fixture().await;

    for call_under_test in POLICY_ARN_CALLS {
        let action = call_under_test.action;
        // The two rows that would REMOVE the held policy or its only version are the
        // ones a single control pass cannot take: `DeletePolicy` would leave nothing for
        // the rows after it to address, and a policy's default version cannot be
        // deleted. Both report the absence of a policy they resolve nothing under
        // through the same table above, which is what this suite is about.
        if matches!(action, "DeletePolicy" | "DeletePolicyVersion") {
            continue;
        }
        let (status, body) = policy_arn_call(&router, call_under_test, &held).await;
        assert_eq!(
            status, 200,
            "case {action:?}: the held policy must be served, got {status}: {body}"
        );
    }

    let (status, body) = call(&router, "ListEntitiesForPolicy", &[("PolicyArn", &held)]).await;
    assert_eq!(
        status, 200,
        "the held policy must be served after the attachments, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "GroupName"),
        GROUP_NAME,
        "expected the attached group to be related, got {body}"
    );
    assert_eq!(
        element(&body, "RoleName"),
        ROLE_NAME,
        "expected the attached role to be related, got {body}"
    );
    assert_eq!(
        element(&body, "UserName"),
        USER_NAME,
        "expected the attached user to be related, got {body}"
    );
}
