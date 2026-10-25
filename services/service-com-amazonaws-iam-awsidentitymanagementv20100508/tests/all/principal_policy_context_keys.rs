// SPDX-License-Identifier: CC0-1.0

//! The context keys an IAM principal's policies reference, end to end over the
//! awsQuery wire.
//!
//! `GetContextKeysForPrincipalPolicy` reports the condition context keys referenced
//! by the identity-based policies the named principal carries, together with those
//! of any policies the request supplies. Every policy it reads here was PERSISTED by
//! an earlier call (`PutUserPolicy`, `PutRolePolicy`, `CreatePolicy` +
//! `AttachRolePolicy`), so each assertion is about what a client on the socket sees
//! after a round trip through the store. The awsQuery harness (router, call,
//! element, elements, error_code) is the one `tests/all/user_lifecycle.rs` owns and
//! every suite in this crate shares.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! condition operator, the number of keys in one operator block, the number of
//! operators and statements, the form of `Statement`, how many policies the
//! principal carries, which kind of policy carries them (inline, attached managed,
//! supplied in the request), and the case the keys are spelled in.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A trust policy every role this suite creates needs. It carries a `Condition` of
/// its own, which is what makes it evidence for the rule: a trust policy is a
/// RESOURCE-based policy — it says who may assume the role, not what the role may
/// do — so the keys it tests are not the principal's.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dlm.amazonaws.com"},"Action":"sts:AssumeRole","Condition":{"StringEquals":{"aws:SourceAccount":"000000000000"}}}]}"#;

/// The one key [`TRUST_POLICY`] tests, which no response in this suite may report.
const TRUST_POLICY_KEY: &str = "aws:SourceAccount";

/// The sentence the refusal of an ARN naming no readable entity carries — fixed,
/// because the message describes the resource rather than echoing the request.
const ENTITY_NOT_RETRIEVABLE: &str = "Unable to retrieve specified IAM entity.";

/// Create a user, answering the ARN the account holds it under.
async fn create_user(router: &MultiServiceRouter, user_name: &str, path: &str) -> String {
    let (status, body) = call(
        router,
        "CreateUser",
        &[("UserName", user_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: CreateUser must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Create a role, answering the ARN the account holds it under.
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) -> String {
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
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Embed `document` in a user under `policy_name`.
async fn put_user_policy(
    router: &MultiServiceRouter,
    user_name: &str,
    policy_name: &str,
    document: &str,
) {
    let (status, body) = call(
        router,
        "PutUserPolicy",
        &[
            ("UserName", user_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: PutUserPolicy must succeed, got {status}: {body}"
    );
}

/// Embed `document` in a role under `policy_name`.
async fn put_role_policy(
    router: &MultiServiceRouter,
    role_name: &str,
    policy_name: &str,
    document: &str,
) {
    let (status, body) = call(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: PutRolePolicy must succeed, got {status}: {body}"
    );
}

/// Create a managed policy carrying `document` and attach it to `role_name`.
async fn attach_managed_policy(
    router: &MultiServiceRouter,
    role_name: &str,
    policy_name: &str,
    document: &str,
) {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: CreatePolicy must succeed, got {status}: {body}"
    );
    let policy_arn = element(&body, "Arn");
    let (status, body) = call(
        router,
        "AttachRolePolicy",
        &[("RoleName", role_name), ("PolicyArn", &policy_arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: AttachRolePolicy must succeed, got {status}: {body}"
    );
}

/// One `GetContextKeysForPrincipalPolicy` call: the principal's ARN plus the
/// policies supplied in `PolicyInputList`.
async fn context_keys(
    router: &MultiServiceRouter,
    policy_source_arn: &str,
    supplied: &[&str],
) -> (u16, String) {
    let names: Vec<String> = (1..=supplied.len())
        .map(|position| format!("PolicyInputList.member.{position}"))
        .collect();
    let mut params: Vec<(&str, &str)> = vec![("PolicySourceArn", policy_source_arn)];
    for (name, document) in names.iter().zip(supplied) {
        params.push((name.as_str(), document));
    }
    call(router, "GetContextKeysForPrincipalPolicy", &params).await
}

/// The reported key names, sorted, so an assertion is about the SET the operation
/// reports rather than the order it happens to list it in.
fn reported_keys(body: &str) -> Vec<String> {
    let mut keys = elements(body, "member");
    keys.sort();
    keys
}

/// The expected key names, sorted the same way.
fn expected_keys(keys: &[&str]) -> Vec<String> {
    let mut expected: Vec<String> = keys.iter().map(|key| (*key).to_string()).collect();
    expected.sort();
    expected
}

/// One principal: the inline policies it carries, the policies the request supplies
/// beside them, and the keys the operation must report.
struct Case {
    case: &'static str,
    path: &'static str,
    embedded: &'static [&'static str],
    supplied: &'static [&'static str],
    expected: &'static [&'static str],
}

/// Rows spanning the condition forms the IAM policy grammar admits — one operator,
/// several keys in one operator block, several operators in one condition, several
/// statements, a bare statement object, an `IfExists` operator and a set operator
/// (`ForAllValues:`), a statement with no condition at all, a key repeated across
/// statements — and the places a key can come from: one inline policy, several
/// inline policies, `PolicyInputList` alone, and both together.
///
/// The key names are AWS global condition context keys (IAM User Guide, "AWS global
/// condition context keys"); the rule under test is which keys a document
/// REFERENCES, not which keys exist, so nothing here depends on that catalog.
const CASES: &[Case] = &[
    Case {
        case: "one operator testing one key",
        path: "/",
        embedded: &[
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*","Condition":{"StringEquals":{"aws:PrincipalTag/team":"storage"}}}]}"#,
        ],
        supplied: &[],
        expected: &["aws:PrincipalTag/team"],
    },
    Case {
        case: "two keys tested by one operator",
        path: "/eng/",
        embedded: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"DateGreaterThan":{"aws:CurrentTime":"2026-01-01T00:00:00Z","aws:TokenIssueTime":"2026-01-01T00:00:00Z"}}}]}"#,
        ],
        supplied: &[],
        expected: &["aws:CurrentTime", "aws:TokenIssueTime"],
    },
    Case {
        case: "two operators in one condition",
        path: "/eng/tools/",
        embedded: &[
            r#"{"Statement":[{"Effect":"Deny","Action":"iam:*","Condition":{"StringEquals":{"aws:username":"alice"},"Bool":{"aws:MultiFactorAuthPresent":"true"}}}]}"#,
        ],
        supplied: &[],
        expected: &["aws:username", "aws:MultiFactorAuthPresent"],
    },
    Case {
        case: "two statements each carrying a condition",
        path: "/sales/",
        embedded: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"s3:*","Condition":{"IpAddress":{"aws:SourceIp":"10.0.0.0/8"}}},{"Effect":"Deny","Action":"s3:*","Condition":{"Bool":{"aws:SecureTransport":"false"}}}]}"#,
        ],
        supplied: &[],
        expected: &["aws:SourceIp", "aws:SecureTransport"],
    },
    Case {
        case: "a bare statement object rather than a list",
        path: "/",
        embedded: &[
            r#"{"Statement":{"Effect":"Allow","Action":"ec2:*","Condition":{"StringEquals":{"aws:RequestedRegion":"us-east-1"}}}}"#,
        ],
        supplied: &[],
        expected: &["aws:RequestedRegion"],
    },
    Case {
        case: "an IfExists operator and a set operator",
        path: "/eng/",
        embedded: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"StringEqualsIfExists":{"aws:PrincipalOrgID":"o-abc"},"ForAllValues:StringEquals":{"aws:TagKeys":["team"]}}}]}"#,
        ],
        supplied: &[],
        expected: &["aws:PrincipalOrgID", "aws:TagKeys"],
    },
    Case {
        case: "a policy that tests nothing",
        path: "/sales/west/",
        embedded: &[
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListBucket","Resource":"*"}]}"#,
        ],
        supplied: &[],
        expected: &[],
    },
    Case {
        case: "a policy carrying no statement of its own",
        path: "/",
        embedded: &[r#"{"Statement":[]}"#],
        supplied: &[],
        expected: &[],
    },
    Case {
        case: "a principal carrying no policy at all",
        path: "/eng/",
        embedded: &[],
        supplied: &[],
        expected: &[],
    },
    Case {
        case: "one key repeated across statements and operators",
        path: "/eng/",
        embedded: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"StringEquals":{"aws:PrincipalAccount":"000000000000"}}},{"Effect":"Deny","Action":"*","Condition":{"StringNotEquals":{"aws:PrincipalAccount":"000000000001"}}}]}"#,
        ],
        supplied: &[],
        expected: &["aws:PrincipalAccount"],
    },
    Case {
        case: "three inline policies, one of them testing nothing",
        path: "/eng/tools/",
        embedded: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"s3:*","Condition":{"StringLike":{"aws:ResourceTag/project":"atlas*"}}}]}"#,
            r#"{"Statement":[{"Effect":"Allow","Action":"kms:*"}]}"#,
            r#"{"Statement":[{"Effect":"Allow","Action":"sts:*","Condition":{"ArnEquals":{"aws:SourceArn":"arn:aws:sns:us-east-1:000000000000:topic"}}}]}"#,
        ],
        supplied: &[],
        expected: &["aws:ResourceTag/project", "aws:SourceArn"],
    },
    Case {
        case: "keys supplied only through PolicyInputList",
        path: "/",
        embedded: &[],
        supplied: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"Bool":{"aws:ViaAWSService":"true"}}}]}"#,
        ],
        expected: &["aws:ViaAWSService"],
    },
    Case {
        case: "two supplied policies beside two inline ones, sharing one key",
        path: "/sales/",
        embedded: &[
            r#"{"Statement":[{"Effect":"Allow","Action":"s3:*","Condition":{"Bool":{"aws:SecureTransport":"true"}}}]}"#,
            r#"{"Statement":[{"Effect":"Allow","Action":"ec2:*","Condition":{"NumericLessThan":{"aws:MultiFactorAuthAge":"3600"}}}]}"#,
        ],
        supplied: &[
            r#"{"Statement":[{"Effect":"Deny","Action":"*","Condition":{"Bool":{"aws:SecureTransport":"false"}}}]}"#,
            r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"StringEquals":{"aws:PrincipalType":"User"}}}]}"#,
        ],
        expected: &[
            "aws:SecureTransport",
            "aws:MultiFactorAuthAge",
            "aws:PrincipalType",
        ],
    },
];

/// The keys a principal's own policies and the request's policies reference are
/// reported as ONE set: every key any of them tests, each key once.
#[tokio::test]
async fn every_key_the_principals_policies_test_is_reported() {
    let router = router();
    for (index, case) in CASES.iter().enumerate() {
        let Case {
            case,
            path,
            embedded,
            supplied,
            expected,
        } = case;
        let user_name = format!("keys{index}");
        let arn = create_user(&router, &user_name, path).await;
        for (position, document) in embedded.iter().enumerate() {
            let policy_name = format!("embedded-{position}");
            put_user_policy(&router, &user_name, &policy_name, document).await;
        }

        let (status, body) = context_keys(&router, &arn, supplied).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read must succeed, got {status}: {body}"
        );
        assert_eq!(
            reported_keys(&body),
            expected_keys(expected),
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }
}

/// A role's keys come from the policies attached to it — its inline policies and
/// the default version of each managed policy attached to it — and NOT from its
/// trust policy, which is a resource-based policy.
#[tokio::test]
async fn a_roles_keys_come_from_its_identity_based_policies() {
    let router = router();
    let arn = create_role(&router, "ContextRole", "/eng/").await;

    // Nothing but the trust policy yet: its key must not be reported.
    let (status, body) = context_keys(&router, &arn, &[]).await;
    assert_eq!(
        status, 200,
        "a role carrying no identity-based policy must still be read, got {status}: {body}"
    );
    assert_eq!(
        reported_keys(&body),
        expected_keys(&[]),
        "the trust policy's keys must not be reported, got {body}"
    );

    // Rows: a key from an inline policy, a key from an attached managed policy, and
    // both together, so neither source is forgotten.
    put_role_policy(
        &router,
        "ContextRole",
        "Embedded",
        r#"{"Statement":[{"Effect":"Allow","Action":"s3:*","Condition":{"StringEquals":{"aws:ResourceTag/stage":"prod"}}}]}"#,
    )
    .await;
    let (status, body) = context_keys(&router, &arn, &[]).await;
    assert_eq!(status, 200, "the read must succeed, got {status}: {body}");
    assert_eq!(
        reported_keys(&body),
        expected_keys(&["aws:ResourceTag/stage"]),
        "expected the inline policy's key, got {body}"
    );

    attach_managed_policy(
        &router,
        "ContextRole",
        "ContextManaged",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"dynamodb:*","Resource":"*","Condition":{"DateLessThan":{"aws:EpochTime":"2000000000"}}}]}"#,
    )
    .await;
    let (status, body) = context_keys(&router, &arn, &[]).await;
    assert_eq!(status, 200, "the read must succeed, got {status}: {body}");
    assert_eq!(
        reported_keys(&body),
        expected_keys(&["aws:ResourceTag/stage", "aws:EpochTime"]),
        "expected the inline and the attached policy's keys, got {body}"
    );
    assert!(
        !reported_keys(&body).iter().any(|key| key == TRUST_POLICY_KEY),
        "the trust policy's key must never be reported, got {body}"
    );

    // A read reports, so a second identical read answers the same thing and the
    // policies it read are still readable.
    let (status, repeated) = context_keys(&router, &arn, &[]).await;
    assert_eq!(status, 200, "the repeated read must succeed, got {repeated}");
    assert_eq!(
        reported_keys(&repeated),
        reported_keys(&body),
        "a read must not change what a later read sees, got {repeated}"
    );
    let (status, listed) = call(
        &router,
        "ListAttachedRolePolicies",
        &[("RoleName", "ContextRole")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the attachment must survive the read, got {status}: {listed}"
    );
    assert_eq!(
        element(&listed, "PolicyName"),
        "ContextManaged",
        "the read must leave the attachment as it was, got {listed}"
    );
}

/// Context key names are not distinguished by case, so two spellings of one key are
/// ONE reported key.
#[tokio::test]
async fn keys_are_folded_without_regard_to_case() {
    // Rows: the spellings a document can carry for one key — as AWS documents it,
    // lower case, upper case and inverted case — each in its own policy.
    let spellings = [
        "aws:username",
        "AWS:USERNAME",
        "Aws:UserName",
        "aWS:uSERNAME",
    ];

    let router = router();
    let arn = create_user(&router, "FoldedKeys", "/").await;
    for (position, spelling) in spellings.iter().enumerate() {
        let document = format!(
            r#"{{"Statement":[{{"Effect":"Allow","Action":"s3:*","Condition":{{"StringEquals":{{"{spelling}":"alice"}}}}}}]}}"#
        );
        put_user_policy(
            &router,
            "FoldedKeys",
            &format!("spelling-{position}"),
            &document,
        )
        .await;
    }

    let (status, body) = context_keys(&router, &arn, &[]).await;
    assert_eq!(status, 200, "the read must succeed, got {status}: {body}");
    let reported = reported_keys(&body);
    assert_eq!(
        reported.len(),
        1,
        "the four spellings name one key, so one key must be reported, got {body}"
    );
    assert_eq!(
        reported[0].to_ascii_lowercase(),
        "aws:username",
        "expected the key the policies test, got {body}"
    );
}

/// An ARN that names no principal the account holds is refused as a missing entity,
/// and nothing about the account changes.
#[tokio::test]
async fn an_arn_naming_no_held_principal_is_not_found() {
    let router = router();
    let user_arn = create_user(&router, "Held", "/eng/").await;
    create_role(&router, "HeldRole", "/").await;
    let account = account_id();

    // Rows: a user never created, a role never created, a group (no operation can
    // create one yet), a held user's name under another path, a held user's name in
    // another account, and a held role's name carrying the user resource type.
    let absent = [
        (
            "a user nobody created",
            format!("arn:aws:iam::{account}:user/ghost"),
        ),
        (
            "a role nobody created",
            format!("arn:aws:iam::{account}:role/eng/ghost"),
        ),
        (
            "a group, which no operation can create yet",
            format!("arn:aws:iam::{account}:group/Developers"),
        ),
        (
            "a held user under another path",
            format!("arn:aws:iam::{account}:user/Held"),
        ),
        (
            "a held user in another account",
            "arn:aws:iam::000000000001:user/eng/Held".to_string(),
        ),
        (
            "a held role under the user resource type",
            format!("arn:aws:iam::{account}:user/HeldRole"),
        ),
    ];

    for (case, arn) in absent {
        let (status, body) = context_keys(&router, &arn, &[]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {arn}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            ENTITY_NOT_RETRIEVABLE,
            "case {case:?}: expected the one sentence this refusal carries, got {body}"
        );
    }

    // The held user is still read, so the refusals above are about the ARN rather
    // than about the store.
    let (status, body) = context_keys(&router, &user_arn, &[]).await;
    assert_eq!(
        status, 200,
        "the held user must still be read, got {status}: {body}"
    );
}

/// An ARN that is not an IAM user's, group's or role's, and a supplied policy that
/// is not an IAM policy document, are both refused as invalid input.
#[tokio::test]
async fn an_invalid_arn_or_supplied_policy_is_refused() {
    let router = router();
    let arn = create_user(&router, "InvalidInputTarget", "/").await;
    let account = account_id();

    // Rows spanning the ways an ARN can fail to name a principal: not an ARN at all,
    // another service's resource, an IAM resource of another type, an IAM ARN
    // carrying a region, one carrying no account, and one whose name is empty.
    // Every row is at least the 20 characters `arnType` requires, so the boundary
    // validator passes it to the handler.
    let invalid = [
        ("not an ARN at all", "a-string-of-twenty-c".to_string()),
        (
            "another service's resource",
            "arn:aws:s3:::my-bucket-name/key".to_string(),
        ),
        (
            "an IAM managed policy",
            format!("arn:aws:iam::{account}:policy/ReadOnly"),
        ),
        (
            "an IAM SAML provider",
            format!("arn:aws:iam::{account}:saml-provider/Corp"),
        ),
        (
            "an IAM ARN carrying a region",
            format!("arn:aws:iam:us-east-1:{account}:user/alice"),
        ),
        (
            "an IAM ARN carrying no account",
            "arn:aws:iam:::user/alice-with-a-long-name".to_string(),
        ),
        (
            "a user ARN with an empty name",
            format!("arn:aws:iam::{account}:user/"),
        ),
        (
            "a user ARN with an empty path segment",
            format!("arn:aws:iam::{account}:user//alice"),
        ),
    ];

    for (case, candidate) in invalid {
        let (status, body) = context_keys(&router, &candidate, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {candidate}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // The same table of non-documents `PutRolePolicy`'s suite refuses, because the
    // rule is the one shared definition of an IAM policy document.
    let malformed = [
        ("not JSON at all", "not a policy"),
        ("a bare string", r#""Statement""#),
        ("a JSON array", r#"[{"Effect":"Allow"}]"#),
        ("a JSON number", "17"),
        ("an object with no Statement", r#"{"Version":"2012-10-17"}"#),
        ("an empty object", "{}"),
        ("a truncated object", r#"{"Statement":[{"Effect":"Allow"}"#),
        ("a lower-case statement key", r#"{"statement":[]}"#),
    ];
    let valid = r#"{"Statement":[{"Effect":"Allow","Action":"*","Condition":{"Bool":{"aws:SecureTransport":"true"}}}]}"#;
    for (case, document) in malformed {
        // Alone, and beside a document that IS a policy, so the refusal is about the
        // member rather than about the list carrying one entry.
        for (position, supplied) in [vec![document], vec![valid, document]]
            .into_iter()
            .enumerate()
        {
            let (status, body) = context_keys(&router, &arn, &supplied).await;
            assert_eq!(
                status, 400,
                "case {case:?} at position {position}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInput",
                "case {case:?} at position {position}: expected the modelled awsQuery code, \
                 got {body}"
            );
        }
    }

    // A refused request reports nothing and changes nothing: the principal still
    // reads back.
    let (status, body) = context_keys(&router, &arn, &[valid]).await;
    assert_eq!(
        status, 200,
        "the principal must still be read after the refusals, got {status}: {body}"
    );
    assert_eq!(
        reported_keys(&body),
        expected_keys(&["aws:SecureTransport"]),
        "expected the supplied policy's key, got {body}"
    );
}
