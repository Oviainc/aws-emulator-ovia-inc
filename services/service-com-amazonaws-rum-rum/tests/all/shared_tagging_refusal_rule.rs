// SPDX-License-Identifier: CC0-1.0

//! The ONE refusal mapping RUM's ARN-addressed tagging frontend answers through, driven
//! through each of the three operations that used to carry its own copy of it:
//! `TagResource`, `UntagResource` and `ListTagsForResource`.
//!
//! The rule under test: whichever of those three operations is called, a refusal of a
//! given class is answered with the SAME wire error code, the same HTTP status and the
//! same body — an ARN naming no resource with `NotFoundException` at 404 carrying
//! `ResourceNotFoundException`'s `message` alone, a request the frontend refuses with
//! `BadRequestException` at 400 carrying `ValidationException`'s payload, and an action
//! the policies in force do not allow with `AccessDeniedException` at 403 carrying the
//! modelled payload. One frontend cannot word one condition two ways, so the three
//! operations share the mapping (`crate::state::TaggingRefusal`) rather than each
//! declaring it.
//!
//! Each table row differs along what the rule generalises over — the refusal class, the
//! operation asked, and within a class the input that reaches it (one ARN component per
//! not-found row, one way of failing the tag-key constraints per invalid-request row,
//! plus the documented tag ceiling) — and every row is asserted on all the operations of
//! the frontend that can reach that class. `ListTagsForResource` is absent from the
//! invalid-request table alone: it carries no request of its own to refuse, since the one
//! malformed `ResourceArn` is reworded at the boundary for every tagging operation at
//! once (`crate::service`).

use serde_json::{json, Value};

use crate::authorization::{identity_policy, monitor_arn, statement, Emulator, Reply, ACCOUNT_KEY};
use crate::support::percent_encoded;

/// The wire error code, and the HTTP status, each refusal class of this frontend is
/// answered with.
///
/// Sources: the recorded unit `rum.untag-resource-unknown-resource-rejected` for the
/// not-found pair and `rum.tag-resource-invalid-key-rejected` for the invalid-request one;
/// the statuses are the `@httpError` of the shape each refusal carries
/// (`ResourceNotFoundException` 404, `ValidationException` 400), and `AccessDeniedException`
/// is the AWS common error at 403.
const NOT_FOUND: (&str, u16) = ("NotFoundException", 404);
const BAD_REQUEST: (&str, u16) = ("BadRequestException", 400);
const ACCESS_DENIED: (&str, u16) = ("AccessDeniedException", 403);

/// The whole message the not-found refusal states, whatever ARN the request named.
/// Source: the same recorded unit.
const NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The most tags one RUM resource holds at once: `TagResource` documents "You can
/// associate as many as 50 tags with a resource".
/// Source: `smithy_query(query="operation", operation="TagResource")`.
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The tag map every app monitor below is created with, and the key an `UntagResource` row
/// names when it is not the row's subject.
const BASELINE_KEY: &str = "Team";

/// One of the three operations of the tagging frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    /// `TagResource`: `POST /tags/{ResourceArn}`, the keys carried by the `Tags` body.
    Tag,
    /// `UntagResource`: `DELETE /tags/{ResourceArn}`, the keys bound to `?tagKeys=`.
    Untag,
    /// `ListTagsForResource`: `GET /tags/{ResourceArn}`, which carries no key at all.
    List,
}

/// Every operation of the frontend, so a row is asserted on all three.
const EVERY_OPERATION: [Operation; 3] = [Operation::Tag, Operation::Untag, Operation::List];

/// The two that carry tag keys of their own, and so can refuse a request on them.
const KEY_CARRYING_OPERATIONS: [Operation; 2] = [Operation::Tag, Operation::Untag];

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Operation::Tag => "TagResource",
            Operation::Untag => "UntagResource",
            Operation::List => "ListTagsForResource",
        }
    }

    /// The IAM action the operation is authorized against.
    fn action(self) -> &'static str {
        match self {
            Operation::Tag => "rum:TagResource",
            Operation::Untag => "rum:UntagResource",
            Operation::List => "rum:ListTagsForResource",
        }
    }

    fn method(self) -> &'static str {
        match self {
            Operation::Tag => "POST",
            Operation::Untag => "DELETE",
            Operation::List => "GET",
        }
    }

    /// The request path for this operation against the resource `arn` names, with `keys`
    /// bound the way this operation carries them.
    fn path(self, arn: &str, keys: &[&str]) -> String {
        let path = format!("/tags/{}", percent_encoded(arn));
        match self {
            Operation::Untag if !keys.is_empty() => {
                let query = keys
                    .iter()
                    .map(|key| format!("tagKeys={}", percent_encoded(key)))
                    .collect::<Vec<_>>()
                    .join("&");
                format!("{path}?{query}")
            }
            _ => path,
        }
    }

    /// The request body: `TagResource` carries its keys as a `Tags` map, the other two
    /// carry no body at all.
    fn body(self, keys: &[&str]) -> Option<Value> {
        match self {
            Operation::Tag => {
                let tags: serde_json::Map<String, Value> = keys
                    .iter()
                    .map(|key| ((*key).to_string(), json!(format!("value-of-{key}"))))
                    .collect();
                Some(json!({"Tags": Value::Object(tags)}))
            }
            _ => None,
        }
    }
}

/// Create one app monitor carrying the baseline tag with the account's own credential, and
/// answer its ARN.
async fn create_tagged_monitor(emulator: &Emulator, name: &str) -> String {
    let reply = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            "/appmonitor",
            Some(json!({
                "Name": name,
                "Domain": "example.com",
                "Tags": {BASELINE_KEY: "web"},
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: creating the app monitor must succeed, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    monitor_arn(name)
}

/// One tagging call signed with `access_key_id`.
async fn tagging_call(
    emulator: &Emulator,
    access_key_id: &str,
    operation: Operation,
    arn: &str,
    keys: &[&str],
) -> Reply {
    emulator
        .rum(
            access_key_id,
            operation.method(),
            &operation.path(arn, keys),
            operation.body(keys),
        )
        .await
}

/// Assert an answer is the refusal class `expected` names: its status, its wire code, and a
/// body that reports nothing of the resource the request addressed.
fn assert_refusal(case: &str, reply: &Reply, expected: (&str, u16)) -> Value {
    let (code, status) = expected;
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.error_code(case);
    assert_eq!(
        answered, code,
        "case {case:?}: expected the wire error code {code:?}, got {answered:?}"
    );
    let body = reply.json(case);
    for member in ["Tags", "ResourceArn"] {
        assert_eq!(
            body[member],
            Value::Null,
            "case {case:?}: a refusal must report no {member}, got {body}"
        );
    }
    body
}

/// An ARN naming no resource this service holds is refused the same way by all three
/// operations: `NotFoundException` at 404, with the frontend's constant message and
/// `message` as the whole body.
#[tokio::test]
async fn every_tagging_operation_answers_the_same_not_found_refusal() {
    let emulator = Emulator::new();
    let stored = create_tagged_monitor(&emulator, "shared-refusal-stored").await;

    // One row per component of the stored monitor's ARN: each is a well-formed ARN
    // differing from it in exactly one place, so none of them names a resource this
    // service holds.
    let account = crate::authorization::account();
    let rows: Vec<(&str, String)> = vec![
        ("an unknown resource name", monitor_arn("no-such-monitor")),
        (
            "another region",
            format!("arn:aws:rum:eu-west-1:{account}:appmonitor/shared-refusal-stored"),
        ),
        (
            "another account",
            "arn:aws:rum:us-east-1:123456789012:appmonitor/shared-refusal-stored".to_string(),
        ),
        (
            "another service",
            format!("arn:aws:cloudwatch:us-east-1:{account}:appmonitor/shared-refusal-stored"),
        ),
        (
            "another resource type",
            format!("arn:aws:rum:us-east-1:{account}:dashboard/shared-refusal-stored"),
        ),
        (
            "another partition",
            format!("arn:aws-cn:rum:us-east-1:{account}:appmonitor/shared-refusal-stored"),
        ),
    ];

    for (label, arn) in rows {
        assert_ne!(
            arn, stored,
            "case {label:?}: the row's ARN must differ from the stored monitor's"
        );
        for operation in EVERY_OPERATION {
            let case = format!("{} with {label}", operation.name());
            let reply = tagging_call(&emulator, ACCOUNT_KEY, operation, &arn, &[BASELINE_KEY]).await;
            let body = assert_refusal(&case, &reply, NOT_FOUND);
            assert_eq!(
                crate::support::message(&body),
                Some(NOT_FOUND_MESSAGE),
                "case {case:?}: expected the message {NOT_FOUND_MESSAGE:?}, got {:?}",
                crate::support::message(&body)
            );
            // RUM's not-found body is `message` only: the modelled `resourceName` and
            // `resourceType` are never serialized, so a client gets no second copy of what
            // it sent.
            for member in ["resourceName", "resourceType"] {
                assert_eq!(
                    body[member],
                    Value::Null,
                    "case {case:?}: expected no {member} member, got {body}"
                );
            }
        }
    }
}

/// A request the frontend refuses on the tag keys it carries is refused the same way by
/// both operations that carry keys: `BadRequestException` at 400, stating why.
#[tokio::test]
async fn every_tag_write_answers_the_same_invalid_request_refusal() {
    let emulator = Emulator::new();
    let arn = create_tagged_monitor(&emulator, "shared-refusal-keys").await;

    // One row per way a key fails `TagKey`'s `@pattern` `^(?!aws:)[a-zA-Z+-=._:/]+$` —
    // the constraint the boundary walk leaves unapplied, so the handler is what refuses
    // these: the reserved prefix the leading lookahead excludes, and three characters
    // outside the class.
    // Source: `smithy_query(query="shape", shape="TagMap")`, the key's pattern.
    for (label, key) in [
        ("the reserved aws: prefix", "aws:reserved"),
        ("a reserved prefixed key", "aws:cloudformation:stack-name"),
        ("a space in the key", "has space"),
        ("a hash in the key", "hash#tag"),
        ("a parenthesis in the key", "paren(s)"),
    ] {
        for operation in KEY_CARRYING_OPERATIONS {
            let case = format!("{} with {label}", operation.name());
            let reply = tagging_call(&emulator, ACCOUNT_KEY, operation, &arn, &[key]).await;
            let body = assert_refusal(&case, &reply, BAD_REQUEST);
            let stated = crate::support::message(&body).unwrap_or_default();
            assert!(
                stated.contains(key),
                "case {case:?}: the refusal must state the key it was made about, got \
                 {stated:?}"
            );
        }
    }

    // The documented ceiling is the other rule that reaches this class, and it reaches it
    // only through the write that merges onto the map: one tag past the maximum.
    let past_ceiling: Vec<String> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| format!("Ceiling{index:03}"))
        .collect();
    let keys: Vec<&str> = past_ceiling.iter().map(String::as_str).collect();
    let case = format!("TagResource with {} tags", keys.len());
    let reply = tagging_call(&emulator, ACCOUNT_KEY, Operation::Tag, &arn, &keys).await;
    assert_refusal(&case, &reply, BAD_REQUEST);

    // The same write at the ceiling is accepted, so the row above is refused on the
    // ceiling rather than on anything else it carries.
    let within: Vec<&str> = keys[..MAX_TAGS_PER_RESOURCE - 1].to_vec();
    let case = format!("TagResource with {} tags", within.len());
    let reply = tagging_call(&emulator, ACCOUNT_KEY, Operation::Tag, &arn, &within).await;
    assert_eq!(
        reply.status, 204,
        "case {case:?}: expected the write inside the ceiling to be accepted, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// An action the policies in force do not allow is refused the same way by all three
/// operations: `AccessDeniedException` at 403, naming the action the decision was made
/// about.
#[tokio::test]
async fn every_tagging_operation_answers_the_same_access_denied_refusal() {
    for operation in EVERY_OPERATION {
        let emulator = Emulator::new();
        let monitor = format!("shared-refusal-denied-{}", operation.method().to_lowercase());
        let arn = create_tagged_monitor(&emulator, &monitor).await;
        // A principal allowed one RUM action that is NOT this operation's, so the refusal
        // rests on the action rather than on the service.
        let key = emulator
            .assumed_role_key(
                &format!("denied-{}", operation.method().to_lowercase()),
                Some(&identity_policy(&statement(
                    "Allow",
                    "rum:GetAppMonitor",
                    "*",
                ))),
            )
            .await;

        let case = format!("{} for a principal allowed only rum:GetAppMonitor", operation.name());
        let reply = tagging_call(&emulator, &key, operation, &arn, &[BASELINE_KEY]).await;
        let body = assert_refusal(&case, &reply, ACCESS_DENIED);
        let stated = crate::support::message(&body).unwrap_or_default();
        assert!(
            stated.contains(operation.action()),
            "case {case:?}: the refusal must name the action it was made about, got \
             {stated:?}"
        );
    }
}
