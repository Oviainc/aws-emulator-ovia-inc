// SPDX-License-Identifier: CC0-1.0

//! What a `Condition` over the TARGET of an IAM call is decided from, end to end over the
//! awsQuery wire.
//!
//! The rule under test: the request a decision is made about carries the tags of the
//! resource the call is taken on — under both the service-specific `iam:ResourceTag/<key>`
//! and the global `aws:ResourceTag/<key>` spelling — read from that resource at the
//! instant of the call, so a clause over one of them is decided (and a clause over a tag
//! key the resource does not carry leaves its statement undecided, which grants nothing).
//! The caller's own keys are in the same request, so a clause VALUE written as a policy
//! variable resolves per caller: one document decides two targets differently, and
//! retagging either side changes what the identical request decides. An ARN naming no
//! resource the account holds carries no tags that can be read, so a clause over one of
//! them cannot be decided for it either.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which of
//! the two key families the clause names, whether its value is a literal or a policy
//! variable, which operator compares them (equality, its negation, a pattern, and the
//! existence operator), whether the target's value matches the caller's, which kind of
//! resource the request addresses (a user, a role, a managed policy), and whether the
//! target is tagged, untagged or absent.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share.

use smithy_server_runtime::MultiServiceRouter;

use crate::policy_variable_scoping::tagged_principal;
use crate::user_lifecycle::{account_id, call, call_signed_by, element, error_code, router};

/// The tag key every row conditions on.
const TAG_KEY: &str = "project";

/// The caller's own value for [`TAG_KEY`], and the value the matching targets carry.
const CALLER_VALUE: &str = "alpha";

/// The value the non-matching target carries: the rows differ only in this value, so a
/// target that is simply untagged cannot pass for a matching one.
const OTHER_VALUE: &str = "beta";

/// A one-statement `Allow` for `action` over `resource`, conditioned on `key` comparing to
/// `value` under `operator`.
fn conditioned(operator: &str, key: &str, value: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"{action}","Resource":"{resource}","Condition":{{"{operator}":{{"{key}":"{value}"}}}}}}]}}"#
    )
}

/// One row: the clause the caller's document carries, the call it then makes, and whether
/// the rule requires that call to be served.
struct Case {
    case: &'static str,
    /// The condition operator, the key it compares and the value it compares against —
    /// the value being a literal on some rows and a policy variable on others.
    operator: &'static str,
    key: String,
    value: String,
    /// The IAM action the document allows, and the awsQuery `Action` the row calls with
    /// the parameter addressing its target.
    iam_action: &'static str,
    action: &'static str,
    parameter: &'static str,
    target: String,
    /// The `Resource` element the statement is scoped to: every row names the target's
    /// own ARN, so the clause is the only thing that can separate one row from another.
    resource: String,
    /// Whether the clause holds for that target, which is whether the call is served.
    served: bool,
}

/// The value of the caller's own tag, written as the policy variable that resolves to it.
fn principal_tag_variable() -> String {
    format!("${{aws:PrincipalTag/{TAG_KEY}}}")
}

/// The ARN of an IAM user of this account at the root path.
fn user_arn(user_name: &str) -> String {
    format!("arn:aws:iam::{}:user/{user_name}", account_id())
}

/// The ARN of an IAM role of this account at the root path.
fn role_arn(role_name: &str) -> String {
    format!("arn:aws:iam::{}:role/{role_name}", account_id())
}

/// The ARN of a customer managed policy of this account at the root path.
fn policy_arn(policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy/{policy_name}", account_id())
}

/// The fixture's targets: a user whose tag matches the caller's, one whose value differs,
/// one carrying no tag at all, a role and a managed policy that match, and a name the
/// account holds nothing under.
const MATCHING_USER: &str = "target-match";
const DIFFERING_USER: &str = "target-other";
const UNTAGGED_USER: &str = "target-bare";
const MATCHING_ROLE: &str = "target-role";
const MATCHING_POLICY: &str = "target-policy";
const ABSENT_USER: &str = "target-absent";

/// The rows.
fn cases() -> Vec<Case> {
    let variable = principal_tag_variable();
    let iam_key = format!("iam:ResourceTag/{TAG_KEY}");
    let aws_key = format!("aws:ResourceTag/{TAG_KEY}");
    vec![
        // The clause compares the target's tag to the CALLER's, through a policy
        // variable: it holds for the target tagged like the caller.
        Case {
            case: "the target's tag equals the caller's, through a variable",
            operator: "StringEquals",
            key: iam_key.clone(),
            value: variable.clone(),
            iam_action: "iam:ListAccessKeys",
            action: "ListAccessKeys",
            parameter: "UserName",
            target: MATCHING_USER.to_string(),
            resource: user_arn(MATCHING_USER),
            served: true,
        },
        // The same document, the same request shape, a target whose value differs: only
        // the clause separates the two rows.
        Case {
            case: "the target's tag differs from the caller's, through a variable",
            operator: "StringEquals",
            key: iam_key.clone(),
            value: variable.clone(),
            iam_action: "iam:ListAccessKeys",
            action: "ListAccessKeys",
            parameter: "UserName",
            target: DIFFERING_USER.to_string(),
            resource: user_arn(DIFFERING_USER),
            served: false,
        },
        // The GLOBAL spelling of the key names the same tag of the same resource.
        Case {
            case: "the global resource-tag spelling, through a variable",
            operator: "StringEquals",
            key: aws_key.clone(),
            value: variable.clone(),
            iam_action: "iam:ListUserTags",
            action: "ListUserTags",
            parameter: "UserName",
            target: MATCHING_USER.to_string(),
            resource: user_arn(MATCHING_USER),
            served: true,
        },
        // A LITERAL value, so the clause is about the target alone: it holds for the
        // target carrying that value and not for the other.
        Case {
            case: "a literal value the target carries",
            operator: "StringEquals",
            key: iam_key.clone(),
            value: OTHER_VALUE.to_string(),
            iam_action: "iam:ListAccessKeys",
            action: "ListAccessKeys",
            parameter: "UserName",
            target: DIFFERING_USER.to_string(),
            resource: user_arn(DIFFERING_USER),
            served: true,
        },
        Case {
            case: "a literal value the target does not carry",
            operator: "StringEquals",
            key: iam_key.clone(),
            value: OTHER_VALUE.to_string(),
            iam_action: "iam:ListAccessKeys",
            action: "ListAccessKeys",
            parameter: "UserName",
            target: MATCHING_USER.to_string(),
            resource: user_arn(MATCHING_USER),
            served: false,
        },
        // The negated comparison holds exactly where its positive sibling does not.
        Case {
            case: "the negated comparison, on the target that differs",
            operator: "StringNotEquals",
            key: iam_key.clone(),
            value: variable.clone(),
            iam_action: "iam:ListAccessKeys",
            action: "ListAccessKeys",
            parameter: "UserName",
            target: DIFFERING_USER.to_string(),
            resource: user_arn(DIFFERING_USER),
            served: true,
        },
        Case {
            case: "the negated comparison, on the target that matches",
            operator: "StringNotEquals",
            key: iam_key.clone(),
            value: variable.clone(),
            iam_action: "iam:ListAccessKeys",
            action: "ListAccessKeys",
            parameter: "UserName",
            target: MATCHING_USER.to_string(),
            resource: user_arn(MATCHING_USER),
            served: false,
        },
        // A pattern: the wildcard belongs to the document, so it matches a run of the
        // target's value.
        Case {
            case: "a pattern the target's value matches",
            operator: "StringLike",
            key: iam_key.clone(),
            value: format!("{}*", &CALLER_VALUE[..2]),
            iam_action: "iam:ListUserTags",
            action: "ListUserTags",
            parameter: "UserName",
            target: MATCHING_USER.to_string(),
            resource: user_arn(MATCHING_USER),
            served: true,
        },
        // The existence operator asks whether the target carries the key at all, which is
        // answerable because the account knows the target's tags in full.
        Case {
            case: "the key asserted absent, on a target carrying no tag",
            operator: "Null",
            key: iam_key.clone(),
            value: "true".to_string(),
            iam_action: "iam:ListUserTags",
            action: "ListUserTags",
            parameter: "UserName",
            target: UNTAGGED_USER.to_string(),
            resource: user_arn(UNTAGGED_USER),
            served: true,
        },
        Case {
            case: "the key asserted absent, on a target carrying it",
            operator: "Null",
            key: iam_key.clone(),
            value: "true".to_string(),
            iam_action: "iam:ListUserTags",
            action: "ListUserTags",
            parameter: "UserName",
            target: MATCHING_USER.to_string(),
            resource: user_arn(MATCHING_USER),
            served: false,
        },
        Case {
            case: "the key asserted present, on a target carrying it",
            operator: "Null",
            key: iam_key.clone(),
            value: "false".to_string(),
            iam_action: "iam:ListUserTags",
            action: "ListUserTags",
            parameter: "UserName",
            target: MATCHING_USER.to_string(),
            resource: user_arn(MATCHING_USER),
            served: true,
        },
        // Another kind of resource: a role's tags answer the same clause.
        Case {
            case: "a role target tagged like the caller",
            operator: "StringEquals",
            key: iam_key.clone(),
            value: variable.clone(),
            iam_action: "iam:ListRoleTags",
            action: "ListRoleTags",
            parameter: "RoleName",
            target: MATCHING_ROLE.to_string(),
            resource: role_arn(MATCHING_ROLE),
            served: true,
        },
        // And a managed policy's, addressed by ARN rather than by name.
        Case {
            case: "a managed-policy target tagged like the caller",
            operator: "StringEquals",
            key: aws_key.clone(),
            value: variable.clone(),
            iam_action: "iam:ListPolicyTags",
            action: "ListPolicyTags",
            parameter: "PolicyArn",
            target: policy_arn(MATCHING_POLICY),
            resource: policy_arn(MATCHING_POLICY),
            served: true,
        },
        // A key of the family the target does not carry: nothing compares against it, so
        // the statement is undecided and grants nothing.
        Case {
            case: "a tag key the target does not carry",
            operator: "StringEquals",
            key: "iam:ResourceTag/absent-key".to_string(),
            value: CALLER_VALUE.to_string(),
            iam_action: "iam:ListUserTags",
            action: "ListUserTags",
            parameter: "UserName",
            target: MATCHING_USER.to_string(),
            resource: user_arn(MATCHING_USER),
            served: false,
        },
        // An ARN naming no resource the account holds: its tags cannot be read, so the
        // clause cannot be decided for it and the refusal comes before the lookup.
        Case {
            case: "a target the account holds nothing under",
            operator: "StringEquals",
            key: iam_key,
            value: variable,
            iam_action: "iam:ListUserTags",
            action: "ListUserTags",
            parameter: "UserName",
            target: ABSENT_USER.to_string(),
            resource: user_arn(ABSENT_USER),
            served: false,
        },
    ]
}

/// Create the fixture's targets: the users, role and managed policy every row addresses.
async fn targets(router: &MultiServiceRouter, case: &str) {
    for (user_name, value) in [
        (MATCHING_USER, Some(CALLER_VALUE)),
        (DIFFERING_USER, Some(OTHER_VALUE)),
        (UNTAGGED_USER, None),
    ] {
        let mut params: Vec<(&str, &str)> = vec![("UserName", user_name)];
        if let Some(value) = value {
            params.push(("Tags.member.1.Key", TAG_KEY));
            params.push(("Tags.member.1.Value", value));
        }
        let (status, created) = call(router, "CreateUser", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreateUser {user_name:?} must succeed, got \
             {status}: {created}"
        );
        // A key of its own, so a row driving `ListAccessKeys` has metadata to report.
        let (status, key) = call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreateAccessKey for {user_name:?} must succeed, got \
             {status}: {key}"
        );
    }

    let trust = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;
    let (status, created) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", MATCHING_ROLE),
            ("AssumeRolePolicyDocument", trust),
            ("Tags.member.1.Key", TAG_KEY),
            ("Tags.member.1.Value", CALLER_VALUE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateRole must succeed, got {status}: {created}"
    );

    let document = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListUsers","Resource":"*"}]}"#;
    let (status, created) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", MATCHING_POLICY),
            ("PolicyDocument", document),
            ("Tags.member.1.Key", TAG_KEY),
            ("Tags.member.1.Value", CALLER_VALUE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreatePolicy must succeed, got {status}: {created}"
    );
}

/// Create a caller tagged with `value` for [`TAG_KEY`], holding `document`, and answer the
/// access key id a request signs with to speak for it.
async fn caller(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    value: &str,
    document: &str,
) -> String {
    let access_key_id = tagged_principal(
        router,
        case,
        user_name,
        &[(TAG_KEY.to_string(), value.to_string())],
    )
    .await;
    let (status, put) = call(
        router,
        "PutUserPolicy",
        &[
            ("UserName", user_name),
            ("PolicyName", "conditioned"),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture PutUserPolicy must succeed, got {status}: {put}"
    );
    access_key_id
}

/// RULE: a clause over a tag of the resource the call is taken on is decided from that
/// resource's own tags, under either spelling of the key family, with a policy variable in
/// the clause's value resolved from the caller's keys — and a clause this cannot answer
/// grants nothing.
#[tokio::test]
async fn a_clause_over_the_targets_tags_is_decided_from_the_target() {
    let router = router();
    targets(&router, "the fixture targets").await;

    for (row, case) in cases().into_iter().enumerate() {
        let document = conditioned(
            case.operator,
            &case.key,
            &case.value,
            case.iam_action,
            &case.resource,
        );
        let access_key_id = caller(
            &router,
            case.case,
            &format!("abac-caller{row}"),
            CALLER_VALUE,
            &document,
        )
        .await;

        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            case.action,
            &[(case.parameter, &case.target)],
        )
        .await;

        if case.served {
            assert_eq!(
                status, 200,
                "case {:?}: expected the clause to hold and the call to be served, got \
                 {status}: {body}",
                case.case
            );
        } else {
            assert_eq!(
                status, 403,
                "case {:?}: expected the clause not to grant the call, got {status}: {body}",
                case.case
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {:?}: expected AccessDenied, got {:?}",
                case.case,
                error_code(&body)
            );
        }
    }
}

/// RULE: the tags both sides of the clause are read from are the ones held at the instant
/// of the call, so re-tagging either the caller or the target changes what the IDENTICAL
/// document and the IDENTICAL request decide.
#[tokio::test]
async fn a_retag_changes_what_the_same_request_decides() {
    let router = router();
    targets(&router, "the fixture targets").await;

    // One document, naming BOTH targets in its `Resource` element, so what separates the
    // two calls below is the clause alone.
    let document = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:ListAccessKeys","Resource":["{}","{}"],"Condition":{{"StringEquals":{{"iam:ResourceTag/{TAG_KEY}":"{}"}}}}}}]}}"#,
        user_arn(MATCHING_USER),
        user_arn(DIFFERING_USER),
        principal_tag_variable(),
    );
    let user_name = "abac-retagged";
    let access_key_id = caller(
        &router,
        "a retag of the caller",
        user_name,
        CALLER_VALUE,
        &document,
    )
    .await;

    // Rows: what the caller is tagged with, and which of the two targets the one
    // unchanged request is then served for.
    for (case, caller_value, served, refused) in [
        (
            "the caller tagged like the first target",
            CALLER_VALUE,
            MATCHING_USER,
            DIFFERING_USER,
        ),
        (
            "the caller re-tagged like the second",
            OTHER_VALUE,
            DIFFERING_USER,
            MATCHING_USER,
        ),
        (
            "the caller tagged back again",
            CALLER_VALUE,
            MATCHING_USER,
            DIFFERING_USER,
        ),
    ] {
        let (status, tagged) = call(
            &router,
            "TagUser",
            &[
                ("UserName", user_name),
                ("Tags.member.1.Key", TAG_KEY),
                ("Tags.member.1.Value", caller_value),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-tag must succeed, got {status}: {tagged}"
        );

        let (status, body) =
            call_signed_by(&router, Some(&access_key_id), "ListAccessKeys", &[("UserName", served)])
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the target tagged like the caller to be served, got \
             {status}: {body}"
        );

        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            "ListAccessKeys",
            &[("UserName", refused)],
        )
        .await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected the target tagged otherwise to be refused, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {:?}",
            error_code(&body)
        );
    }

    // The TARGET's own tags are read the same way: re-tagging it, with the caller and the
    // document untouched, moves the grant with it.
    let (status, tagged) = call(
        &router,
        "TagUser",
        &[
            ("UserName", DIFFERING_USER),
            ("Tags.member.1.Key", TAG_KEY),
            ("Tags.member.1.Value", CALLER_VALUE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a retag of the target\": the re-tag must succeed, got {status}: {tagged}"
    );
    let (status, body) = call_signed_by(
        &router,
        Some(&access_key_id),
        "ListAccessKeys",
        &[("UserName", DIFFERING_USER)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a retag of the target\": expected the re-tagged target to be served, got \
         {status}: {body}"
    );
    assert_eq!(
        element(&body, "UserName"),
        DIFFERING_USER,
        "case \"a retag of the target\": expected the target's own key metadata, got {body}"
    );
}
