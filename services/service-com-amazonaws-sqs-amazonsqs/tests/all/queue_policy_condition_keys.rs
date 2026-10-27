// SPDX-License-Identifier: CC0-1.0

//! What a queue policy's `Condition` block decides for a caller the request context can be
//! answered for, end to end.
//!
//! The rules under test, both about the request context a queue-policy decision is made
//! under rather than about one operation:
//!
//! 1. **A condition clause written with IAM's `IfExists` suffix HOLDS where the request
//!    carries no value for its key**, so a statement conditioned that way applies to a
//!    caller that cannot answer the key at all, while the same clause without the suffix
//!    compares nothing and therefore grants nothing. A direct API caller's request carries
//!    no `aws:SourceArn` — that key names the resource another service made the call on
//!    behalf of — so it is the key the two spellings differ over here.
//! 2. **An assumed-role session answers `aws:PrincipalArn` with the ARN of its ROLE**: "for
//!    IAM roles, the request context returns the ARN of the role, not the ARN of the user
//!    that assumed the role", so a statement scoped to a role's ARN applies to every session
//!    of it, while the session's own `arn:aws:sts:…:assumed-role/…` spelling is not what the
//!    key carries. A long-term credential answers it with that identity's own ARN.
//!
//! Both are read through the same gate every queue operation uses, so the tables span the
//! operator family (ARN, string, boolean), whether it is negated, whether the suffix is
//! written, whether the key is carried, modelled-but-absent or not modelled at all, the
//! effect the conditioned statement states, the kind of credential the request presents, and
//! the operation — and therefore the action — being judged.
//!
//! The principals are made through IAM's and STS's own operations and the calls carry the
//! credential's access key id the way a client does; the harness is
//! [`super::queue_policy_authorization`]'s, shared rather than re-derived.

use serde_json::{json, Value};

use super::queue_policy_authorization::{denial, implicit_denial, policy, Emulator, SESSION_NAME};
use super::support::{account, expected_arn, REGION};

/// The ARN STS reports for a session of the role called `role_name` — what a refusal names
/// the caller by, and the spelling `aws:PrincipalArn` does NOT carry.
fn session_arn(role_name: &str) -> String {
    format!(
        "arn:aws:sts::{}:assumed-role/{role_name}/{SESSION_NAME}",
        account()
    )
}

/// The ARN of a resource of ANOTHER service, the kind of value `aws:SourceArn` carries when
/// a service makes a call on a resource's behalf — and which a direct caller's request
/// carries no value for.
fn source_arn() -> String {
    format!("arn:aws:sns:{REGION}:{}:a-topic", account())
}

/// One row of the condition table: the case, the `Condition` element the grant carries, and
/// whether the grantee's call is allowed under it.
type ConditionRow = (&'static str, fn(&str, &str) -> Value, bool);

/// The `Condition` of a statement granting the role, written over the key a direct caller
/// carries no value for (`aws:SourceArn`) and over the one it does (`aws:PrincipalArn`).
///
/// The rows differ along what rule 1 generalises over: the operator family, whether the
/// operator is negated, whether the `IfExists` suffix is written, and whether the key the
/// clause names is one the request carries, one it models and carries no value for, or one
/// it does not model at all.
const SUFFIX_ROWS: &[ConditionRow] = &[
    (
        "ArnEquals on a key a direct caller carries no value for",
        |_role, source| json!({"ArnEquals": {"aws:SourceArn": source}}),
        false,
    ),
    (
        "ArnEqualsIfExists on that same key",
        |_role, source| json!({"ArnEqualsIfExists": {"aws:SourceArn": source}}),
        true,
    ),
    (
        "ArnLikeIfExists with a wildcard over that key",
        |_role, _source| {
            json!({"ArnLikeIfExists": {"aws:SourceArn": format!("arn:aws:sns:{REGION}:*:*")}})
        },
        true,
    ),
    (
        "StringEqualsIfExists over that key",
        |_role, source| json!({"StringEqualsIfExists": {"aws:SourceArn": source}}),
        true,
    ),
    (
        "ArnNotEquals over that key, which compares nothing and holds",
        |_role, source| json!({"ArnNotEquals": {"aws:SourceArn": source}}),
        true,
    ),
    (
        "BoolIfExists over the transport key the request does answer",
        |_role, _source| json!({"BoolIfExists": {"aws:SecureTransport": "true"}}),
        true,
    ),
    (
        "the suffix over a key this gate does not model, which decides nothing",
        |_role, _source| json!({"StringEqualsIfExists": {"aws:SourceVpc": "vpc-1"}}),
        false,
    ),
    (
        "NullIfExists, which IAM defines for no operator",
        |_role, _source| json!({"NullIfExists": {"aws:SourceArn": "true"}}),
        false,
    ),
    (
        "the suffix over a key the request DOES carry, with a value that matches",
        |role, _source| json!({"ArnEqualsIfExists": {"aws:PrincipalArn": role}}),
        true,
    ),
    (
        "the suffix over a key the request DOES carry, with a value that does not",
        |_role, _source| {
            json!({"ArnEqualsIfExists": {"aws:PrincipalArn": format!("arn:aws:iam::{}:role/somebody-else", account())}})
        },
        false,
    ),
];

/// A clause written `…IfExists` holds for a caller that carries no value for its key, so the
/// statement grants; the same comparison without the suffix grants nothing, and a refused
/// call leaves the queue as it was.
#[tokio::test]
async fn an_if_exists_clause_grants_where_the_request_carries_no_value_for_the_key() {
    for (index, (case, condition, allowed)) in SUFFIX_ROWS.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("if-exists-{index}");
        let url = emulator.queue(case, &name).await;
        let arn = expected_arn(&name);
        let (role_arn, key) = emulator
            .assumed_role(case, &format!("if-exists-role-{index}"), &[])
            .await;
        emulator
            .set_policy(
                case,
                &url,
                &policy(json!([{
                    "Sid": "ConditionedGrant",
                    "Effect": "Allow",
                    "Principal": {"AWS": role_arn},
                    "Action": "sqs:SendMessage",
                    "Resource": arn,
                    "Condition": condition(&role_arn, &source_arn()),
                }])),
            )
            .await;

        let body = format!("body of {case}");
        let reply = emulator
            .sqs(
                &key,
                "SendMessage",
                &json!({"QueueUrl": url, "MessageBody": body}),
            )
            .await;
        if *allowed {
            assert_eq!(
                reply.0, 200,
                "case {case:?}: the conditioned grant must authorize the send, got {} with {}",
                reply.0, reply.1
            );
            assert!(
                reply.1["MessageId"].is_string(),
                "case {case:?}: an allowed send must answer a MessageId, got {}",
                reply.1
            );
            continue;
        }
        let session_arn = session_arn(&format!("if-exists-role-{index}"));
        let message = denial(case, reply);
        assert_eq!(
            message,
            implicit_denial(&session_arn, "sqs:sendmessage", &arn),
            "case {case:?}: the refusal must name the action no policy allows"
        );
        let attributes = emulator.attributes(case, &url).await;
        assert_eq!(
            attributes
                .get("ApproximateNumberOfMessages")
                .map(String::as_str),
            Some("0"),
            "case {case:?}: a refused send must enqueue nothing, got {attributes:?}"
        );
    }
}

/// One row of the principal table: the case, the `Condition` the grant carries over
/// `aws:PrincipalArn`, and whether the call is allowed under it.
type PrincipalRow = (&'static str, fn(&str) -> Value, bool);

/// `aws:PrincipalArn` is answered with the ARN of the identity whose policies are in force,
/// which for an assumed-role session is the ROLE — so a statement scoped to the role's ARN
/// applies to its sessions, and one scoped to the session's own `sts` spelling does not.
#[tokio::test]
async fn a_sessions_principal_arn_condition_is_answered_with_its_role() {
    let rows: &[PrincipalRow] = &[
        (
            "ArnEquals on the role's own ARN",
            |role| json!({"ArnEquals": {"aws:PrincipalArn": role}}),
            true,
        ),
        (
            "StringEquals on the role's own ARN",
            |role| json!({"StringEquals": {"aws:PrincipalArn": role}}),
            true,
        ),
        (
            "ArnLike over every role ARN of this account",
            |_role| json!({"ArnLike": {"aws:PrincipalArn": format!("arn:aws:iam::{}:role/*", account())}}),
            true,
        ),
        (
            "ArnLike over the session spelling the key does NOT carry",
            |_role| json!({"ArnLike": {"aws:PrincipalArn": "arn:aws:sts::*:assumed-role/*"}}),
            false,
        ),
        (
            "StringNotEquals on the role's own ARN",
            |role| json!({"StringNotEquals": {"aws:PrincipalArn": role}}),
            false,
        ),
    ];
    for (index, (case, condition, allowed)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("principal-arn-{index}");
        let url = emulator.queue(case, &name).await;
        let arn = expected_arn(&name);
        let (role_arn, key) = emulator
            .assumed_role(case, &format!("principal-arn-role-{index}"), &[])
            .await;
        emulator
            .set_policy(
                case,
                &url,
                &policy(json!([{
                    "Sid": "ScopedToThePrincipal",
                    "Effect": "Allow",
                    "Principal": {"AWS": role_arn},
                    // A second action, so the rule is judged under an operation that only
                    // REPORTS the queue as well as under the one that writes to it.
                    "Action": ["sqs:GetQueueAttributes", "sqs:SendMessage"],
                    "Resource": arn,
                    "Condition": condition(&role_arn),
                }])),
            )
            .await;

        let reply = emulator
            .sqs(
                &key,
                "GetQueueAttributes",
                &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
            )
            .await;
        if *allowed {
            assert_eq!(
                reply.0, 200,
                "case {case:?}: the grant must authorize the read, got {} with {}",
                reply.0, reply.1
            );
            assert_eq!(
                reply.1["Attributes"]["QueueArn"],
                json!(arn),
                "case {case:?}: the read must report the queue's ARN, got {}",
                reply.1
            );
            continue;
        }
        let session_arn = session_arn(&format!("principal-arn-role-{index}"));
        let message = denial(case, reply);
        assert_eq!(
            message,
            implicit_denial(&session_arn, "sqs:getqueueattributes", &arn),
            "case {case:?}: the refusal must name the session and the action"
        );
    }
}

/// A long-term credential answers `aws:PrincipalArn` with the identity's own ARN, which is
/// the same rule read for a principal that is not a session.
#[tokio::test]
async fn a_users_principal_arn_condition_is_answered_with_its_own_arn() {
    let emulator = Emulator::new();
    let case = "a user scoped to its own ARN";
    let name = "principal-arn-user";
    let url = emulator.queue(case, name).await;
    let arn = expected_arn(name);
    let (user_arn, key) = emulator.user(case, "principal-arn-sender", None, &[]).await;
    emulator
        .set_policy(
            case,
            &url,
            &policy(json!([{
                "Effect": "Allow",
                "Principal": {"AWS": user_arn},
                "Action": "sqs:SendMessage",
                "Resource": arn,
                "Condition": {"ArnEquals": {"aws:PrincipalArn": user_arn}},
            }])),
        )
        .await;
    let reply = emulator
        .sqs(
            &key,
            "SendMessage",
            &json!({"QueueUrl": url, "MessageBody": "from a user"}),
        )
        .await;
    assert_eq!(
        reply.0, 200,
        "case {case:?}: the user's own ARN must answer the key, got {} with {}",
        reply.0, reply.1
    );
}

/// A `Deny` whose `IfExists` clause holds because the key is absent REFUSES, so the suffix
/// is not a one-way weakening: it decides a statement of either effect.
#[tokio::test]
async fn an_if_exists_deny_refuses_a_caller_that_carries_no_value_for_the_key() {
    let emulator = Emulator::new();
    let case = "an IfExists deny beside an unconditioned grant";
    let name = "if-exists-deny";
    let url = emulator.queue(case, name).await;
    let arn = expected_arn(name);
    let (role_arn, key) = emulator
        .assumed_role(case, "if-exists-deny-role", &[])
        .await;
    emulator
        .set_policy(
            case,
            &url,
            &policy(json!([
                {
                    "Effect": "Allow",
                    "Principal": {"AWS": role_arn},
                    "Action": "sqs:SendMessage",
                    "Resource": arn,
                },
                {
                    "Effect": "Deny",
                    "Principal": {"AWS": role_arn},
                    "Action": "sqs:SendMessage",
                    "Resource": arn,
                    "Condition": {"ArnEqualsIfExists": {"aws:SourceArn": source_arn()}},
                },
            ])),
        )
        .await;
    let reply = emulator
        .sqs(
            &key,
            "SendMessage",
            &json!({"QueueUrl": url, "MessageBody": "denied by the conditioned statement"}),
        )
        .await;
    let session_arn = session_arn("if-exists-deny-role");
    let message = denial(case, reply);
    assert_eq!(
        message,
        format!(
            "User: {session_arn} is not authorized to perform: sqs:sendmessage on resource: \
             {arn} with an explicit deny in a resource-based policy"
        ),
        "case {case:?}: the refusal must name the document that denied it"
    );
    let attributes = emulator.attributes(case, &url).await;
    assert_eq!(
        attributes
            .get("ApproximateNumberOfMessages")
            .map(String::as_str),
        Some("0"),
        "case {case:?}: a refused send must enqueue nothing, got {attributes:?}"
    );
}
