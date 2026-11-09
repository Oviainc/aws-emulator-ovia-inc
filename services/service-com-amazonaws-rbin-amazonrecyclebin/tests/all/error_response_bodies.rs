// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the BODIES of this service's modelled error responses.
//!
//! Three rules, each asserted over a table that spans the class it generalises rather
//! than the one request that exposed it:
//!
//! 1. A request that violates a constraint the MODEL declares is refused with the
//!    modelled `ValidationException`: HTTP 400, `Reason` the fault class
//!    (`INVALID_PARAMETER_VALUE` for a parameter value), and `Message` composed from the
//!    request as `N validation error(s) detected: Value at '<member path>' failed to
//!    satisfy constraint: <the requirement the model declares>` — the member path and
//!    the requirement, with the submitted value NOT quoted back. Asserted across range,
//!    pattern, enum and required-member constraints, in the body and in a URI label, on
//!    six operations.
//! 2. A rule the request addressed and this Region does not hold is `ResourceNotFound`:
//!    HTTP 404 and `Message` composed from the rule's own IDENTIFIER
//!    (`Recycle Bin Rule '<identifier>' does not exist.`) — the `Identifier` member for
//!    the five operations that bind one, and the identifier the `ResourceArn` names for
//!    the three that address the rule by ARN — with the optional `Reason` the
//!    service does not populate on this path left out —
//!    `tests/all/not_found_error_body_members.rs` asserts that absence over the class.
//!    Asserted on every operation that addresses a rule — by `Identifier` and by
//!    `ResourceArn`.
//! 3. A lock, modify or delete the rule's own state forbids is `Conflict`: HTTP 409,
//!    `Reason` `INVALID_RULE_STATE`, and `Message` composed from the identifier and the
//!    blocking condition (`Cannot <action> Recycle Bin rule <id> because it
//!    <predicate>.`), never from the stored lock-state token. Asserted over every
//!    blocking condition this service can report, including a rule inside its unlock
//!    delay, and each refusal is read back through `GetRule` to show it changed nothing.
//!    The lock requests a locked rule COMPLETES, and the delay mismatch that is the one
//!    lock request it refuses, are owned by `tests/all/lock_rule_idempotence.rs`.
//!
//! Every case drives the service through the real router, so the wire protocol, the
//! route table, the input-validation gate and the handlers are exercised exactly as a
//! signed SDK request would exercise them.

use serde_json::{json, Value};

use crate::resource_tags::{keys, list_tags, pairs, percent_encode, tag_resource, untag_resource};
use crate::retention_rule_lifecycle::{
    call, create_rule, delete_rule, get_rule, lock_configuration, retention, router, update_rule,
};
use crate::rule_lock_transitions::{lock_rule, unlock_rule};
use smithy_server_runtime::MultiServiceRouter;

/// Published bounds of `RetentionPeriod$RetentionPeriodValue`.
/// Source: smithy_query(query="shape", shape="RetentionPeriod") — range {min: 1, max: 3650}.
const RETENTION_VALUE_MIN: i64 = 1;
const RETENTION_VALUE_MAX: i64 = 3650;

/// Published bounds of `ListRulesRequest$MaxResults`.
/// Source: smithy_query(query="shape", shape="MaxResults") — range {min: 1, max: 1000}.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// Published bounds of `UnlockDelay$UnlockDelayValue`.
/// Source: smithy_query(query="shape", shape="UnlockDelay") — range {min: 7, max: 30}.
const UNLOCK_DELAY_MIN: i64 = 7;
const UNLOCK_DELAY_MAX: i64 = 30;

/// The `@pattern` each constrained member declares, as the service QUOTES it for that
/// member: a member bound into the URI PATH (`@httpLabel`) is matched as one path segment
/// of the route, so what is quoted is its expression spliced into the route's — the
/// modelled one without the whole-value anchors (the recorded AWS refusal for
/// `GetRule$Identifier` carries `…regular expression pattern: [0-9a-zA-Z]{11}`), while a
/// member the request carries in its BODY is matched against its own expression and is
/// quoted exactly as the model declares it, anchors included (the recorded refusal for
/// `ListRules$NextToken` carries `…pattern: ^[A-Za-z0-9+/=]{1,2048}$`).
/// Source: smithy_query(query="shape", shape="RuleIdentifier,NextToken,Description,RuleArn")
/// — `Identifier` and `ResourceArn` are `@httpLabel`, `NextToken` and `Description` are not.
const RULE_IDENTIFIER_PATTERN: &str = "[0-9a-zA-Z]{11}";
const NEXT_TOKEN_PATTERN: &str = "^[A-Za-z0-9+/=]{1,2048}$";
const DESCRIPTION_PATTERN: &str = r"^[\S ]{0,255}$";
const RULE_ARN_PATTERN: &str = r"arn:\S+:rbin:[a-z\-0-9]{0,63}:[0-9]{12}:rule/[0-9a-zA-Z]{11}{0,1011}";

/// `ResourceType` accepts exactly these values, in this order.
/// Source: smithy_query(query="shape", shape="ResourceType") — enum values.
const RESOURCE_TYPES: [&str; 3] = ["EBS_SNAPSHOT", "EBS_VOLUME", "EC2_IMAGE"];

/// The wire error code of an error response (`__type`), or `"<none>"`. `pub(crate)` so
/// the sibling suites of this one test target read an error's code the same way.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

/// The modelled `Message` member, or `"<none>"`. `pub(crate)` like [`error_code`], so
/// the sibling suites of this one test target read an error's message the same way.
pub(crate) fn message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

/// One validation clause in this service's envelope: the member path, then the
/// requirement the model declares for it. `pub(crate)` for the same reason as
/// [`message`]: one copy of the envelope format cannot drift from another.
pub(crate) fn one_validation_error(member_path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member_path}' failed to satisfy constraint: \
         {requirement}"
    )
}

/// `rule_arn` with one ARN segment replaced: a well-formed rule ARN that addresses a
/// rule this Region and account do not hold. `pub(crate)` for the same reason as
/// [`created_rule`].
pub(crate) fn with_arn_segment(rule_arn: &str, index: usize, replacement: &str) -> String {
    let mut segments: Vec<&str> = rule_arn.split(':').collect();
    assert!(
        index < segments.len(),
        "an rbin rule ARN has 6 colon-separated segments, got {rule_arn:?}"
    );
    segments[index] = replacement;
    segments.join(":")
}

/// `POST /list-rules` — `ListRules`.
async fn list_rules(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/list-rules", Some(body)).await
}

/// Create one Region-level rule and hand back `(identifier, rule_arn)`. `pub(crate)` so
/// the sibling suites of this one test target set a rule up through the same request.
pub(crate) async fn created_rule(
    router: &MultiServiceRouter,
    case: &str,
    body: Value,
) -> (String, String) {
    let (status, created) = create_rule(router, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRule expected HTTP 201, got {status} / {created:?}"
    );
    let identifier = created["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier: {created:?}"))
        .to_string();
    let rule_arn = created["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no RuleArn: {created:?}"))
        .to_string();
    (identifier, rule_arn)
}

/// What the refusal must state as the requirement the member failed.
enum Requirement {
    /// The whole requirement text, as the bound the model declares renders.
    Exact(String),
    /// A requirement that lists a SET. The model fixes the members, not the order they
    /// are listed in, so the opening and every member are asserted and the order is not.
    Set {
        opening: &'static str,
        members: &'static [&'static str],
    },
    /// A member the request left out: the one class that reports no value at all, so the
    /// clause opens `Value null at` rather than `Value at`.
    Absent,
}

/// One row of the constraint table: the case, the request, and the member path plus the
/// requirement the refusal must state.
struct ConstraintRow {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    member_path: &'static str,
    requirement: Requirement,
}

#[tokio::test]
async fn a_violated_model_constraint_is_refused_with_both_modelled_members() {
    let router = router();
    // One row per constraint CLASS the model declares (range at both bounds, pattern,
    // enum value set, absent required member), reached in a body member, in a nested
    // member, and in a URI label, across six of the ten operations. A refusal is a
    // property of the INPUT SHAPE, so the class spans operations rather than naming one.
    let rows: Vec<ConstraintRow> = vec![
        ConstraintRow {
            case: "CreateRule: retention period one day past the maximum",
            method: "POST",
            path: "/rules".to_string(),
            body: Some(json!({
                "ResourceType": RESOURCE_TYPES[0],
                "RetentionPeriod": retention(RETENTION_VALUE_MAX + 1),
            })),
            member_path: "retentionPeriod.retentionPeriodValue",
            requirement: Requirement::Exact(format!(
                "Member must have value less than or equal to {RETENTION_VALUE_MAX}"
            )),
        },
        ConstraintRow {
            case: "CreateRule: retention period one day short of the minimum",
            method: "POST",
            path: "/rules".to_string(),
            body: Some(json!({
                "ResourceType": RESOURCE_TYPES[1],
                "RetentionPeriod": retention(RETENTION_VALUE_MIN - 1),
            })),
            member_path: "retentionPeriod.retentionPeriodValue",
            requirement: Requirement::Exact(format!(
                "Member must have value greater than or equal to {RETENTION_VALUE_MIN}"
            )),
        },
        ConstraintRow {
            case: "CreateRule: a resource type the enum does not name",
            method: "POST",
            path: "/rules".to_string(),
            body: Some(json!({
                "ResourceType": "EBS_SNAPSHOTS",
                "RetentionPeriod": retention(30),
            })),
            member_path: "resourceType",
            requirement: Requirement::Set {
                opening: "Member must satisfy enum value set: [",
                members: &RESOURCE_TYPES,
            },
        },
        ConstraintRow {
            case: "CreateRule: a description holding a character its pattern forbids",
            method: "POST",
            path: "/rules".to_string(),
            body: Some(json!({
                "ResourceType": RESOURCE_TYPES[2],
                "RetentionPeriod": retention(30),
                "Description": "two\tcolumns",
            })),
            member_path: "description",
            requirement: Requirement::Exact(format!(
                "Member must satisfy regular expression pattern: {DESCRIPTION_PATTERN}"
            )),
        },
        ConstraintRow {
            case: "GetRule: an identifier shorter than the pattern allows",
            method: "GET",
            path: "/rules/short".to_string(),
            body: None,
            member_path: "identifier",
            requirement: Requirement::Exact(format!(
                "Member must satisfy regular expression pattern: {RULE_IDENTIFIER_PATTERN}"
            )),
        },
        ConstraintRow {
            case: "DeleteRule: an 11-character identifier outside the pattern's alphabet",
            method: "DELETE",
            path: "/rules/bad-id-char".to_string(),
            body: None,
            member_path: "identifier",
            requirement: Requirement::Exact(format!(
                "Member must satisfy regular expression pattern: {RULE_IDENTIFIER_PATTERN}"
            )),
        },
        ConstraintRow {
            case: "UpdateRule: an identifier longer than the pattern allows",
            method: "PATCH",
            path: "/rules/0123456789012".to_string(),
            body: Some(json!({ "Description": "anything" })),
            member_path: "identifier",
            requirement: Requirement::Exact(format!(
                "Member must satisfy regular expression pattern: {RULE_IDENTIFIER_PATTERN}"
            )),
        },
        ConstraintRow {
            case: "LockRule: an unlock delay one day past the maximum",
            method: "PATCH",
            path: "/rules/aaaaaaaaaaa/lock".to_string(),
            body: Some(json!({
                "LockConfiguration": lock_configuration(UNLOCK_DELAY_MAX + 1),
            })),
            member_path: "lockConfiguration.unlockDelay.unlockDelayValue",
            requirement: Requirement::Exact(format!(
                "Member must have value less than or equal to {UNLOCK_DELAY_MAX}"
            )),
        },
        ConstraintRow {
            case: "LockRule: an unlock delay one day short of the minimum",
            method: "PATCH",
            path: "/rules/aaaaaaaaaaa/lock".to_string(),
            body: Some(json!({
                "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN - 1),
            })),
            member_path: "lockConfiguration.unlockDelay.unlockDelayValue",
            requirement: Requirement::Exact(format!(
                "Member must have value greater than or equal to {UNLOCK_DELAY_MIN}"
            )),
        },
        ConstraintRow {
            case: "ListRules: a page size one past the maximum",
            method: "POST",
            path: "/list-rules".to_string(),
            body: Some(json!({
                "ResourceType": RESOURCE_TYPES[0],
                "MaxResults": MAX_RESULTS_MAX + 1,
            })),
            member_path: "maxResults",
            requirement: Requirement::Exact(format!(
                "Member must have value less than or equal to {MAX_RESULTS_MAX}"
            )),
        },
        ConstraintRow {
            case: "ListRules: a page size one short of the minimum",
            method: "POST",
            path: "/list-rules".to_string(),
            body: Some(json!({
                "ResourceType": RESOURCE_TYPES[0],
                "MaxResults": MAX_RESULTS_MIN - 1,
            })),
            member_path: "maxResults",
            requirement: Requirement::Exact(format!(
                "Member must have value greater than or equal to {MAX_RESULTS_MIN}"
            )),
        },
        ConstraintRow {
            case: "ListRules: a page token outside its pattern's alphabet",
            method: "POST",
            path: "/list-rules".to_string(),
            body: Some(json!({
                "ResourceType": RESOURCE_TYPES[0],
                "NextToken": "not a token",
            })),
            member_path: "nextToken",
            requirement: Requirement::Exact(format!(
                "Member must satisfy regular expression pattern: {NEXT_TOKEN_PATTERN}"
            )),
        },
        ConstraintRow {
            case: "TagResource: a resource ARN that is not an ARN at all",
            method: "POST",
            path: format!("/tags/{}", percent_encode("not-an-arn")),
            body: Some(json!({ "Tags": [{ "Key": "env", "Value": "dev" }] })),
            member_path: "resourceArn",
            requirement: Requirement::Exact(format!(
                "Member must satisfy regular expression pattern: {RULE_ARN_PATTERN}"
            )),
        },
        ConstraintRow {
            case: "ListTagsForResource: a resource ARN of another service",
            method: "GET",
            path: format!(
                "/tags/{}",
                percent_encode("arn:aws:ec2:us-east-1:123456789012:volume/vol-0123456789")
            ),
            body: None,
            member_path: "resourceArn",
            requirement: Requirement::Exact(format!(
                "Member must satisfy regular expression pattern: {RULE_ARN_PATTERN}"
            )),
        },
        ConstraintRow {
            case: "CreateRule: the required retention period absent",
            method: "POST",
            path: "/rules".to_string(),
            body: Some(json!({ "ResourceType": RESOURCE_TYPES[0] })),
            member_path: "retentionPeriod",
            requirement: Requirement::Absent,
        },
        ConstraintRow {
            case: "LockRule: the required lock configuration absent",
            method: "PATCH",
            path: "/rules/aaaaaaaaaaa/lock".to_string(),
            body: Some(json!({})),
            member_path: "lockConfiguration",
            requirement: Requirement::Absent,
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, body) = call(&router, row.method, &row.path, row.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {body:?}"
        );
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some("INVALID_PARAMETER_VALUE"),
            "case {case:?}: expected Reason INVALID_PARAMETER_VALUE, got {:?}",
            body.get("Reason")
        );
        // An absent member has no value to report; a member that was PRESENT and
        // invalid states `Value at '<path>'`, with the submitted value not quoted back.
        let stated = message(&body);
        match &row.requirement {
            Requirement::Exact(requirement) => {
                let expected = one_validation_error(row.member_path, requirement);
                assert_eq!(
                    stated, expected,
                    "case {case:?}: expected Message {expected:?}, got {stated:?}"
                );
            }
            Requirement::Set { opening, members } => {
                let expected_opening = one_validation_error(row.member_path, opening);
                assert!(
                    stated.starts_with(&expected_opening) && stated.ends_with(']'),
                    "case {case:?}: expected Message to open {expected_opening:?} and close the \
                     set, got {stated:?}"
                );
                for member in *members {
                    assert!(
                        stated.contains(member),
                        "case {case:?}: expected the value set to name {member:?}, got {stated:?}"
                    );
                }
            }
            Requirement::Absent => {
                let expected = format!(
                    "1 validation error detected: Value null at '{}' failed to satisfy \
                     constraint: Member must not be null",
                    row.member_path
                );
                assert_eq!(
                    stated, expected,
                    "case {case:?}: expected Message {expected:?}, got {stated:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_refused_value_is_never_quoted_back_into_the_validation_message() {
    let router = router();
    // The rule holds whatever the value is: a long one, one holding the quote and
    // backslash characters a JSON body has to escape, and one holding the member path's
    // own spelling. None of them may appear in the refusal.
    let rejected = [
        "a".repeat(64),
        "\"quoted\"".to_string(),
        "back\\slash".to_string(),
        "retentionPeriod".to_string(),
    ];
    for value in rejected {
        let case = format!("a description holding {value:?}");
        // A tab is forbidden by `Description`'s pattern, so each of these values is
        // refused for the pattern and not for anything about its own text.
        let (status, body) = create_rule(
            &router,
            json!({
                "ResourceType": RESOURCE_TYPES[0],
                "RetentionPeriod": retention(30),
                "Description": format!("{value}\t"),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} / {body:?}"
        );
        let stated = message(&body);
        assert_eq!(
            stated,
            one_validation_error(
                "description",
                &format!("Member must satisfy regular expression pattern: {DESCRIPTION_PATTERN}")
            ),
            "case {case:?}: expected the member path and the pattern alone, got {stated:?}"
        );
        assert!(
            !stated.contains(value.as_str()),
            "case {case:?}: the refusal quoted the submitted value back: {stated:?}"
        );
    }
}

#[tokio::test]
async fn an_addressed_rule_this_region_does_not_hold_is_reported_as_not_existing() {
    let router = router();
    // A rule that exists, so the store is not simply empty, plus its ARN to build
    // ARN-addressed requests for rules that do not exist.
    let (identifier, rule_arn) = created_rule(
        &router,
        "a rule that exists",
        json!({ "ResourceType": RESOURCE_TYPES[0], "RetentionPeriod": retention(7) }),
    )
    .await;

    // Identifier-shaped values no rule was minted with: never created, and one created
    // and then deleted — the two ways a rule can be absent.
    let (deleted, deleted_arn) = created_rule(
        &router,
        "a rule that is then deleted",
        json!({ "ResourceType": RESOURCE_TYPES[1], "RetentionPeriod": retention(7) }),
    )
    .await;
    let (status, _) = delete_rule(&router, &deleted).await;
    assert_eq!(status, 204, "setup: DeleteRule expected HTTP 204, got {status}");

    for absent in ["aaaaaaaaaaa", "ZZZZZZZZZZZ", "0a1B2c3D4e5", deleted.as_str()] {
        for (operation, response) in [
            ("GetRule", get_rule(&router, absent).await),
            (
                "UpdateRule",
                update_rule(&router, absent, json!({ "Description": "x" })).await,
            ),
            ("DeleteRule", delete_rule(&router, absent).await),
            (
                "LockRule",
                lock_rule(
                    &router,
                    absent,
                    json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN) }),
                )
                .await,
            ),
            ("UnlockRule", unlock_rule(&router, absent).await),
        ] {
            let case = format!("{operation} on {absent}");
            let (status, body) = response;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} / {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected the modelled ResourceNotFoundException, got {body:?}"
            );
            let expected = format!("Recycle Bin Rule '{absent}' does not exist.");
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected Message {expected:?}, got {:?}",
                message(&body)
            );
            assert_eq!(
                body.get("Reason"),
                None,
                "case {case:?}: expected no Reason member on the not-found body, got {:?}",
                body.get("Reason")
            );
        }
    }

    // The same rule, addressed by ARN: the ARN of a deleted rule, and well-formed ARNs
    // whose Region, account or rule id names no rule this Region holds. Each row carries
    // the IDENTIFIER that ARN names, which is the only part of it the refusal states.
    let other_region = with_arn_segment(&rule_arn, 3, "us-gov-east-1");
    let other_account = with_arn_segment(&rule_arn, 4, "111122223333");
    let other_rule = rule_arn.replace(&identifier, "ZZZZZZZZZZZ");
    for (absent_arn, named) in [
        (deleted_arn.as_str(), deleted.as_str()),
        (other_region.as_str(), identifier.as_str()),
        (other_account.as_str(), identifier.as_str()),
        (other_rule.as_str(), "ZZZZZZZZZZZ"),
    ] {
        for (operation, response) in [
            ("ListTagsForResource", list_tags(&router, absent_arn).await),
            (
                "TagResource",
                tag_resource(&router, absent_arn, &pairs(&[("env", "dev")])).await,
            ),
            (
                "UntagResource",
                untag_resource(&router, absent_arn, &keys(&["env"])).await,
            ),
        ] {
            let case = format!("{operation} on {absent_arn}");
            let (status, body) = response;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} / {body:?}"
            );
            let expected = format!("Recycle Bin Rule '{named}' does not exist.");
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected Message {expected:?}, got {:?}",
                message(&body)
            );
            assert!(
                !message(&body).contains("arn:"),
                "case {case:?}: the refusal states the rule identifier alone, not the ARN the \
                 request addressed it through, got {:?}",
                message(&body)
            );
            assert_eq!(
                body.get("Reason"),
                None,
                "case {case:?}: expected no Reason member on the not-found body, got {:?}",
                body.get("Reason")
            );
        }
    }
}

/// The call a conflict row makes, so the table is exhaustively checked rather than
/// dispatched on a string a typo could break. The verb is the one the refusal states.
#[derive(Debug, Clone, Copy)]
enum Action {
    Lock,
    Unlock,
    Modify,
    Delete,
}

impl Action {
    fn verb(self) -> &'static str {
        match self {
            Self::Lock => "lock",
            Self::Unlock => "unlock",
            Self::Modify => "modify",
            Self::Delete => "delete",
        }
    }

    async fn call(self, router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
        match self {
            Self::Lock => {
                // The delay this lock request names is NOT the one the locked fixture
                // below holds: a request asking for the lock already in force is
                // satisfied by the stored rule, so only a request that would change it
                // reaches a refusal.
                lock_rule(
                    router,
                    identifier,
                    json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MAX) }),
                )
                .await
            }
            Self::Unlock => unlock_rule(router, identifier).await,
            Self::Modify => {
                update_rule(router, identifier, json!({ "Description": "changed" })).await
            }
            Self::Delete => delete_rule(router, identifier).await,
        }
    }
}

/// One row of the conflict table: the case, the request, and the sentence the refusal
/// must state once the identifier is interpolated. `predicate` is the whole of what the
/// message says about the rule, after "because it".
struct ConflictRow {
    case: &'static str,
    identifier: String,
    action: Action,
    predicate: &'static str,
    /// The `LockState` the rule must still report after the refusal.
    lock_state: Option<&'static str>,
}

#[tokio::test]
async fn a_state_that_blocks_a_lock_modify_or_delete_is_reported_as_the_condition() {
    let router = router();
    let (tag_level, _) = created_rule(
        &router,
        "tag-level rule",
        json!({
            "ResourceType": RESOURCE_TYPES[0],
            "RetentionPeriod": retention(7),
            "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "yes" }],
        }),
    )
    .await;
    let (excluded, _) = created_rule(
        &router,
        "Region-level rule with exclusion tags",
        json!({
            "ResourceType": RESOURCE_TYPES[1],
            "RetentionPeriod": retention(7),
            "ExcludeResourceTags": [{ "ResourceTagKey": "scratch" }],
        }),
    )
    .await;
    let (locked, _) = created_rule(
        &router,
        "locked rule",
        json!({
            "ResourceType": RESOURCE_TYPES[2],
            "RetentionPeriod": retention(7),
            "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN),
        }),
    )
    .await;
    let (never_locked, _) = created_rule(
        &router,
        "never locked rule",
        json!({ "ResourceType": RESOURCE_TYPES[0], "RetentionPeriod": retention(7) }),
    )
    .await;
    // A rule inside its unlock delay: still lock-protected, and the one case whose
    // stored lock-state token (`pending_unlock`) differs from the condition the caller
    // is told about.
    let (pending_unlock, _) = created_rule(
        &router,
        "rule inside its unlock delay",
        json!({
            "ResourceType": RESOURCE_TYPES[1],
            "RetentionPeriod": retention(7),
            "LockConfiguration": lock_configuration(UNLOCK_DELAY_MAX),
        }),
    )
    .await;
    let (status, _) = unlock_rule(&router, &pending_unlock).await;
    assert_eq!(
        status, 200,
        "setup: UnlockRule on the locked rule expected HTTP 200, got {status}"
    );

    let rows: Vec<ConflictRow> = vec![
        ConflictRow {
            case: "LockRule on a tag-level rule",
            identifier: tag_level.clone(),
            action: Action::Lock,
            predicate: "is a tag-level rule",
            lock_state: None,
        },
        ConflictRow {
            case: "LockRule on a Region-level rule with exclusion tags",
            identifier: excluded.clone(),
            action: Action::Lock,
            predicate: "is a Region-level rule with exclusion tags",
            lock_state: None,
        },
        ConflictRow {
            case: "LockRule on a locked rule, under another unlock delay",
            identifier: locked.clone(),
            action: Action::Lock,
            predicate: "has a different unlock delay already set",
            lock_state: Some("locked"),
        },
        ConflictRow {
            case: "UpdateRule on a locked rule",
            identifier: locked.clone(),
            action: Action::Modify,
            predicate: "is locked",
            lock_state: Some("locked"),
        },
        ConflictRow {
            case: "DeleteRule on a locked rule",
            identifier: locked.clone(),
            action: Action::Delete,
            predicate: "is locked",
            lock_state: Some("locked"),
        },
        ConflictRow {
            case: "UpdateRule on a rule inside its unlock delay",
            identifier: pending_unlock.clone(),
            action: Action::Modify,
            predicate: "is locked",
            lock_state: Some("pending_unlock"),
        },
        ConflictRow {
            case: "DeleteRule on a rule inside its unlock delay",
            identifier: pending_unlock.clone(),
            action: Action::Delete,
            predicate: "is locked",
            lock_state: Some("pending_unlock"),
        },
        ConflictRow {
            case: "UnlockRule on a rule that has never been locked",
            identifier: never_locked.clone(),
            action: Action::Unlock,
            predicate: "is not locked",
            lock_state: None,
        },
        ConflictRow {
            case: "UnlockRule on a rule already inside its unlock delay",
            identifier: pending_unlock.clone(),
            action: Action::Unlock,
            predicate: "is not locked",
            lock_state: Some("pending_unlock"),
        },
        ConflictRow {
            case: "UnlockRule on a tag-level rule, which has no lock to release",
            identifier: tag_level.clone(),
            action: Action::Unlock,
            predicate: "is not locked",
            lock_state: None,
        },
    ];

    for row in rows {
        let case = row.case;
        let identifier = row.identifier.as_str();
        let (status, body) = row.action.call(&router, identifier).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {body:?}"
        );
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some("INVALID_RULE_STATE"),
            "case {case:?}: expected Reason INVALID_RULE_STATE, got {:?}",
            body.get("Reason")
        );
        let expected = format!(
            "Cannot {} Recycle Bin rule {identifier} because it {}.",
            row.action.verb(),
            row.predicate
        );
        let stated = message(&body);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected Message {expected:?}, got {stated:?}"
        );
        // The stored lock-state token is internal: it must not reach the caller through
        // this message, not even on the `pending_unlock` path.
        assert!(
            !stated.contains("pending_unlock"),
            "case {case:?}: the refusal leaked the stored lock-state token: {stated:?}"
        );

        // A refused lock, modify or delete changes nothing: the rule is still there, in
        // the state it was in.
        let (status, fetched) = get_rule(&router, identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule after the refusal expected HTTP 200, got {status} / {fetched:?}"
        );
        assert_eq!(
            fetched["LockState"].as_str(),
            row.lock_state,
            "case {case:?}: expected LockState {:?} after the refusal, got {:?}",
            row.lock_state,
            fetched["LockState"]
        );
        assert_ne!(
            fetched["Description"].as_str(),
            Some("changed"),
            "case {case:?}: the refused modify must not have applied: {fetched:?}"
        );
    }
}

#[tokio::test]
async fn a_page_token_this_service_did_not_issue_is_the_page_token_fault_class() {
    let router = router();
    let (_identifier, _arn) = created_rule(
        &router,
        "a rule to page over",
        json!({ "ResourceType": RESOURCE_TYPES[0], "RetentionPeriod": retention(7) }),
    )
    .await;
    // Tokens whose SYNTAX satisfies `NextToken`'s pattern — so the constraint gate
    // passes them — but which this service never issued: the other fault class the
    // modelled `ValidationExceptionReason` names.
    for token in ["AAAA", "Zm9vYmFy", "abc=", "0123456789abcdef"] {
        let case = format!("a well-formed page token {token:?} this service did not issue");
        let (status, body) = list_rules(
            &router,
            json!({ "ResourceType": RESOURCE_TYPES[0], "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {body:?}"
        );
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some("INVALID_PAGE_TOKEN"),
            "case {case:?}: expected Reason INVALID_PAGE_TOKEN, got {:?}",
            body.get("Reason")
        );
    }
}
