// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE `CreatePolicyVersion` refuses a `PolicyArn` it resolves no managed
//! policy under with, end to end over the awsQuery wire.
//!
//! RULE: the `NoSuchEntityException` message IAM answers an unresolvable `PolicyArn`
//! with is the fixed text of the OPERATION FAMILY the call belongs to, with the ARN the
//! request supplied as its only variable part, echoed byte for byte. `CreatePolicyVersion`
//! writes the version that takes effect for the identities the policy is attached to, so
//! it belongs to the ATTACHMENT family and words the miss `Policy ` + ` does not exist or
//! is not attachable.` — the sentence the attach calls, `ListEntitiesForPolicy` and the
//! listing of a policy's versions answer — and NOT the `Policy ` + ` was not found.` an
//! operation acting on the policy RESOURCE (`GetPolicy`, `DeletePolicy`, `ListPolicyTags`)
//! answers; the status is 404 and the awsQuery code `NoSuchEntity`
//! (`smithy_query(query="operation", operation="CreatePolicyVersion")`: `NoSuchEntityException`,
//! awsQueryError `NoSuchEntity` / 404), and the refused call writes nothing.
//!
//! The two sentences are separate templates, so this suite asserts BOTH halves on the
//! same ARNs: the attachment family (the create under test beside the attach calls that
//! share its wording) and the resource family (the reads and the delete that must keep
//! theirs), which is what keeps one handler's wording from drifting onto the other
//! family's.
//!
//! The rows differ along what the rule generalises over — the form of the ARN that
//! resolves to nothing (one whose policy was just deleted, a name nothing was created
//! under, the held name at another path, at the account root, in another casing, in
//! another account, in another partition, a deeply nested path, a name spelling the
//! punctuation the name alphabet admits) and, for the create, the document sent and
//! whether `SetAsDefault` is absent, false or true — so no single request is the
//! specification. The whole `PolicyArn`-addressed surface is held to the same rule at
//! once by `tests/all/policy_arn_absence_wording.rs`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, `Message`) of a refusal reached through
//! the wire, never a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// The documents the create rows send: an IAM JSON policy, differing in the statement's
/// effect and action and in how many statements it carries, so what the refusal is
/// asserted against is the ARN and not one document's bytes. A document must be a policy
/// before the ARN is looked up at all, which is why every row carries one.
const DOCUMENTS: &[&str] = &[
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":["sqs:SendMessage","sqs:DeleteMessage"],"Resource":"*"},{"Effect":"Allow","Action":"logs:PutLogEvents","Resource":"*"}]}"#,
];

/// A trust policy with a single statement, which the fixture role needs.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The role the attachment-family control rows name.
const ROLE_NAME: &str = "VersionWordingRole";

/// The user the attachment-family control rows name.
const USER_NAME: &str = "version-wording-user";

/// The name of the policy the account DOES hold, the control that keeps every refusal
/// below about the ARN asked for rather than about an empty store.
const HELD_NAME: &str = "VersionWordingHeld";

/// The path the held policy is created at.
const HELD_PATH: &str = "/held/";

/// IAM's fixed sentence for an unresolvable `PolicyArn` in the ATTACHMENT family — the
/// one `CreatePolicyVersion` must answer.
fn attachment_sentence(arn: &str) -> String {
    format!("Policy {arn} does not exist or is not attachable.")
}

/// IAM's fixed sentence for an unresolvable `PolicyArn` in the policy-RESOURCE family,
/// which must stay off the create's answer.
fn resource_sentence(arn: &str) -> String {
    format!("Policy {arn} was not found.")
}

/// One `CreatePolicyVersion`: the `SetAsDefault` member is sent only when
/// `set_as_default` names a value, so an absent one exercises the modelled default.
async fn create_version(
    router: &MultiServiceRouter,
    policy_arn: &str,
    document: &str,
    set_as_default: Option<&str>,
) -> (u16, String) {
    let mut params = vec![("PolicyArn", policy_arn), ("PolicyDocument", document)];
    if let Some(flag) = set_as_default {
        params.push(("SetAsDefault", flag));
    }
    call(router, "CreatePolicyVersion", &params).await
}

/// Create one customer managed policy and answer the ARN its create reported.
async fn create_policy(router: &MultiServiceRouter, policy_name: &str, path: &str) -> String {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", policy_name),
            ("PolicyDocument", DOCUMENTS[0]),
            ("Path", path),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?} at {path:?}: the fixture create must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// The fixture: a role, a user, one held policy, and one policy created and then deleted
/// so its ARN is well formed and named a policy a moment ago. Answers the router, the
/// held ARN and the released ARN.
async fn fixture() -> (MultiServiceRouter, String, String) {
    let router = router();

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

    let held = create_policy(&router, HELD_NAME, HELD_PATH).await;

    // The leg a cleanup loop drives: a policy that was attached, detached and then
    // deleted, whose ARN now names nothing.
    let released = create_policy(&router, "VersionWordingReleased", "/eng/tools/").await;
    for (action, expected_count) in [("AttachRolePolicy", "1"), ("DetachRolePolicy", "0")] {
        let (status, body) = call(
            &router,
            action,
            &[("RoleName", ROLE_NAME), ("PolicyArn", &released)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {action:?}: the fixture call must succeed, got {status}: {body}"
        );
        let (status, read) = call(&router, "GetPolicy", &[("PolicyArn", &released)]).await;
        assert_eq!(
            status, 200,
            "case {action:?}: GetPolicy must succeed, got {status}: {read}"
        );
        let count = element(&read, "AttachmentCount");
        assert_eq!(
            count, expected_count,
            "case {action:?}: expected AttachmentCount {expected_count}, got {count:?}: {read}"
        );
    }
    let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", &released)]).await;
    assert_eq!(
        status, 200,
        "the fixture delete must succeed, got {status}: {body}"
    );

    (router, held, released)
}

/// Every well-formed managed-policy ARN this account holds no policy under:
/// `(what the row is, the ARN)`. `released` is the ARN the fixture deleted.
fn unresolvable_arns(account: &str, released: &str) -> Vec<(&'static str, String)> {
    vec![
        ("the ARN of a policy that was just deleted", released.to_string()),
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
            // A managed policy's identity carries the case its name was created with, so
            // the same spelling in another case names no policy — and the refusal must
            // echo the case the REQUEST used.
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

/// RULE: `CreatePolicyVersion` refuses a `PolicyArn` that resolves to no managed policy
/// with 404 `NoSuchEntity` whose message is the ARN exactly as supplied between the
/// ATTACHMENT family's fixed text — the same sentence the attach calls,
/// `ListEntitiesForPolicy` and `ListPolicyVersions` answer — while the operations acting
/// on the policy RESOURCE keep their own ` was not found.` sentence for the very same
/// ARNs.
#[tokio::test]
async fn an_unresolvable_policy_arn_is_refused_with_the_attachment_family_sentence() {
    let (router, held, released) = fixture().await;
    let account = account_id();
    let unresolvable = unresolvable_arns(account, &released);

    // The other operations of the create's family, which must answer the SAME sentence,
    // with the entity member each needs so only the policy can refuse it.
    let attachment_family: &[(&str, Option<(&str, &str)>)] = &[
        ("AttachRolePolicy", Some(("RoleName", ROLE_NAME))),
        ("AttachUserPolicy", Some(("UserName", USER_NAME))),
        ("ListEntitiesForPolicy", None),
        ("ListPolicyVersions", None),
    ];
    // The operations acting on the policy RESOURCE, which must keep the other sentence.
    let resource_family: &[(&str, Option<(&str, &str)>)] = &[
        ("GetPolicy", None),
        ("ListPolicyTags", None),
        ("DeletePolicy", None),
    ];

    for (what, arn) in &unresolvable {
        // The create itself, over every form of the other two members: an absent
        // `SetAsDefault`, an explicit false, an explicit true, and each document.
        for set_as_default in [None, Some("false"), Some("true")] {
            for (index, document) in DOCUMENTS.iter().enumerate() {
                let case = format!(
                    "CreatePolicyVersion on {what} with document {index} and SetAsDefault \
                     {set_as_default:?}"
                );
                let (status, body) = create_version(&router, arn, document, set_as_default).await;
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
                // Exact equality, so this also pins that neither the case nor the path
                // of the ARN is normalised on the way into the message.
                assert_eq!(
                    message,
                    attachment_sentence(arn),
                    "case {case:?}: expected {:?}, got {message:?}",
                    attachment_sentence(arn)
                );
                assert!(
                    !message.contains("was not found"),
                    "case {case:?}: the policy-resource sentence must not be answered, got \
                     {message:?}"
                );
            }
        }

        // The rest of the family words the same absence the same way, which is what makes
        // the create's sentence the FAMILY's rather than one handler's.
        for (action, entity) in attachment_family {
            let case = format!("{action} on {what}");
            let mut params = vec![param("PolicyArn", arn)];
            if let Some((member, value)) = entity {
                params.push(param(member, value));
            }
            let (status, body) = call(&router, action, &borrowed(&params)).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                attachment_sentence(arn),
                "case {case:?}: expected the create's own sentence, got {message:?}"
            );
        }

        // And the resource family keeps its sentence for the same ARN: the two templates
        // stay separate, so a caller can still tell the families apart.
        for (action, entity) in resource_family {
            let case = format!("{action} on {what}");
            let mut params = vec![param("PolicyArn", arn)];
            if let Some((member, value)) = entity {
                params.push(param(member, value));
            }
            let (status, body) = call(&router, action, &borrowed(&params)).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                resource_sentence(arn),
                "case {case:?}: expected the policy resource's sentence, got {message:?}"
            );
        }
    }

    // Nothing was written by any of those refusals: the ARNs that resolved to nothing
    // still resolve to nothing, and the policy the account DOES hold still carries the one
    // version its create gave it, as its default.
    for (what, arn) in &unresolvable {
        let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", arn)]).await;
        assert_eq!(
            status, 404,
            "case {what:?}: a refused create must create no policy, got {status}: {body}"
        );
    }
    let (status, body) = call(
        &router,
        "ListPolicyVersions",
        &[("PolicyArn", &held), ("MaxItems", "1000")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the held policy's versions must still be listed, got {status}: {body}"
    );
    let versions = elements(&body, "VersionId");
    assert_eq!(
        versions,
        vec!["v1".to_string()],
        "no refused create may have added a version to the held policy, got {versions:?}"
    );
}

/// RULE (the other side of the same rule): the sentence is about the ARN resolving to
/// nothing, so it is answered identically however often it is asked, and the policy the
/// account DOES hold — including one re-created under the very ARN a refusal named —
/// takes a version instead of the refusal.
#[tokio::test]
async fn the_sentence_is_stable_and_the_held_policy_still_takes_a_version() {
    for (policy_name, path) in [
        ("StableWording", "/"),
        ("stable-wording-nested", "/eng/tools/"),
        ("Stable_1+a=b,c.d@x-2", "/service-role/"),
    ] {
        let case = format!("{policy_name} at {path}");
        let router = router();
        let arn = create_policy(&router, policy_name, path).await;
        let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", &arn)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture delete must succeed, got {status}: {body}"
        );

        // A reconciliation loop asks again, and must get the same answer each time.
        for attempt in ["first attempt", "second attempt", "third attempt"] {
            let (status, body) = create_version(&router, &arn, DOCUMENTS[1], Some("true")).await;
            assert_eq!(
                status, 404,
                "case {case:?} {attempt}: expected HTTP 404, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "NoSuchEntity",
                "case {case:?} {attempt}: expected NoSuchEntity, got {code:?}: {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                attachment_sentence(&arn),
                "case {case:?} {attempt}: expected {:?}, got {message:?}",
                attachment_sentence(&arn)
            );
        }

        // A policy created again under that same ARN takes versions as any other does, so
        // the refusal was about the ARN resolving to nothing and nothing else.
        let recreated = create_policy(&router, policy_name, path).await;
        assert_eq!(
            recreated, arn,
            "case {case:?}: the re-created policy must carry the same ARN, got {recreated:?}"
        );
        let (status, body) = create_version(&router, &arn, DOCUMENTS[1], Some("true")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-created policy must take a version, got {status}: {body}"
        );
        let version_id = element(&body, "VersionId");
        assert_eq!(
            version_id, "v2",
            "case {case:?}: the version must be numbered after the one the create minted, got \
             {version_id:?}: {body}"
        );
    }
}
