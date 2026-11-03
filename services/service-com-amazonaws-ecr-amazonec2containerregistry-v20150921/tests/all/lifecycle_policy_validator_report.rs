// SPDX-License-Identifier: CC0-1.0

//! The SHAPE of the span ECR quotes when it refuses a lifecycle policy document.
//!
//! The rule under test: a lifecycle document is first checked against the lifecycle
//! policy SCHEMA, and a refusal that span quotes reproduces the schema validator's own
//! report verbatim — the validator writes a failure as a LINE, so the quoted span ends
//! with the newline that terminates it and the constraint's closing quote follows that
//! newline. The policy format's rules ACROSS members are checked after the schema and
//! worded by ECR itself, so those refusals quote a plain sentence with no terminator.
//!
//! The schema states a RULE as a CHOICE among the alternative forms the format admits one
//! in, so which part of the schema a value belongs to decides which report it earns: a
//! member every alternative shares earns the validator's report for the constraint that
//! failed, and a value that puts the rule outside every alternative earns the choice's own
//! outcome, which states neither the value nor the set an alternative admits.
//!
//! So the tables below span the CLASSES of schema failure — a required member absent, a
//! number under its minimum, a value outside an enum, an array under its minimum entry
//! count, and a rule in none of the alternative forms — over several members, values, JSON
//! types and entry counts, rather than the one recorded document; and the second table
//! spans the cross-member rules, which must NOT grow a terminator. A refused document also
//! changes nothing, so the last suite reads the previously stored policy back after every
//! row.
//!
//! Every case goes through the shared `wire` harness, so it exercises routing,
//! deserialization, the handler and serialization — the same path a client takes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, ok};

/// The error code every document refusal carries (`smithy_query(query="operation",
/// operation="PutLifecyclePolicy,StartLifecyclePolicyPreview")` declares
/// `InvalidParameterException`, HTTP 400, on both).
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The API-documented label of the parameter carrying the document.
const PARAMETER: &str = "LifecyclePolicyText";

/// What a refusal of the document's validation opens the quoted span with.
const VALIDATION_PREFIX: &str = "Lifecycle policy validation failure: ";

/// The report the schema produces for a rule in NONE of the alternative forms it admits
/// one in: the choice's own outcome, which the recorded refusal of an unrecognised
/// `countType` carries. The count is the number of those forms — one per count type
/// crossed with one per action type — and the matched count is zero, because the rule is
/// in none of them.
const OUTSIDE_EVERY_ALTERNATIVE: &str =
    "instance failed to match exactly one schema (matched 0 out of 4)";

/// The modelled minimum length of `lifecyclePolicyText` (`@length(min: 100, max: 30720)`
/// on `LifecyclePolicyText`, per `smithy_query(query="operation",
/// operation="PutLifecyclePolicy")`). A shorter document is refused at the boundary,
/// before the handler reads it, so every document here is padded past it with trailing
/// whitespace — which a JSON parser ignores.
const MIN_TEXT_LEN: usize = 100;

/// The two operations that read a lifecycle document out of the same parameter, so both
/// word a refusal of it the same way.
const READERS: [&str; 2] = ["PutLifecyclePolicy", "StartLifecyclePolicyPreview"];

/// `document` as the wire carries it: padded past [`MIN_TEXT_LEN`].
fn padded(document: &str) -> String {
    let mut text = document.to_string();
    while text.len() < MIN_TEXT_LEN {
        text.push(' ');
    }
    text
}

/// A lifecycle document built from its rules.
fn document(rules: Value) -> String {
    padded(&json!({ "rules": rules }).to_string())
}

/// The smallest rule the policy format accepts: one expiry selected by a count of
/// images, with the catch-all `tagStatus`.
fn count_rule(priority: i64, count: i64) -> Value {
    json!({
        "rulePriority": priority,
        "selection": {
            "tagStatus": "any",
            "countType": "imageCountMoreThan",
            "countNumber": count,
        },
        "action": { "type": "expire" },
    })
}

/// One rule from the selection and action it is given.
fn rule_with(selection: Value, action: Value) -> Value {
    json!({ "rulePriority": 1, "selection": selection, "action": action })
}

/// The message `operation` refused `text` with, after asserting the status and the error
/// code the whole class carries.
async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    text: &str,
) -> String {
    let answer = call(
        router,
        operation,
        &json!({ "repositoryName": repository_name(), "lifecyclePolicyText": text }),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        INVALID_PARAMETER,
        "case {case:?}: expected error code {INVALID_PARAMETER} from {operation}, got {:?}",
        answer.body
    );
    answer.message().to_string()
}

/// The repository every suite below puts its documents to.
fn repository_name() -> &'static str {
    "catalog/validator-report"
}

/// Create [`repository_name`] and return the emulator holding it.
async fn emulator_with_repository() -> MultiServiceRouter {
    let router = emulator();
    ok(
        &router,
        "the repository under test",
        "CreateRepository",
        &json!({ "repositoryName": repository_name() }),
    )
    .await;
    router
}

/// The span ECR quotes as the constraint, lifted out of the refusal message — so a case
/// can assert what the quoted span ENDS with rather than only the whole string.
fn quoted_constraint(case: &str, message: &str) -> String {
    let opening = format!("Invalid parameter at '{PARAMETER}' failed to satisfy constraint: '");
    let rest = message.strip_prefix(&opening).unwrap_or_else(|| {
        panic!("case {case:?}: the refusal must open with {opening:?}, got {message:?}")
    });
    rest.strip_suffix('\'')
        .unwrap_or_else(|| {
            panic!("case {case:?}: the quoted constraint must be closed, got {message:?}")
        })
        .to_string()
}

/// Every class of lifecycle SCHEMA failure, with the validator report each one produces.
/// The rows differ along the schema keyword that failed, the member at fault, its JSON
/// type, its value and the entry count of the array that holds it.
fn schema_failures() -> Vec<(&'static str, String, String)> {
    let expire = || json!({ "type": "expire" });
    let count_selection = |count_type: &str, count: i64| {
        json!({ "tagStatus": "any", "countType": count_type, "countNumber": count })
    };
    let mut cases: Vec<(&'static str, String, String)> = vec![
        (
            "a member the schema requires absent from the policy",
            padded(r#"{"description": "a policy with no rules member"}"#),
            r#"object has missing required properties (["rules"])"#.to_string(),
        ),
        (
            "a member the schema requires absent from a selection",
            document(json!([rule_with(
                json!({ "tagStatus": "any", "countType": "imageCountMoreThan" }),
                expire()
            )])),
            r#"object has missing required properties (["countNumber"])"#.to_string(),
        ),
        (
            "a member the schema requires absent from an action",
            document(json!([rule_with(
                count_selection("imageCountMoreThan", 5),
                json!({})
            )])),
            r#"object has missing required properties (["type"])"#.to_string(),
        ),
        (
            "rules holding fewer entries than the schema admits",
            document(json!([])),
            "array is too short: must have at least 1 elements but instance has 0 elements"
                .to_string(),
        ),
        (
            "a tagStatus outside the values the format defines",
            document(json!([rule_with(
                json!({
                    "tagStatus": "sometimes",
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                }),
                expire()
            )])),
            r#"instance value ("sometimes") not found in enum (possible values: ["any","tagged","untagged"])"#
                .to_string(),
        ),
        (
            // The recorded instance: a count type the format does not define, so the rule
            // is in none of the alternative forms.
            "a countType outside the values the format defines",
            document(json!([rule_with(
                count_selection("notARealCountType", 5),
                expire()
            )])),
            OUTSIDE_EVERY_ALTERNATIVE.to_string(),
        ),
        (
            "a countUnit outside the values the format defines",
            document(json!([rule_with(
                json!({
                    "tagStatus": "any",
                    "countType": "sinceImagePushed",
                    "countUnit": "hours",
                    "countNumber": 5,
                }),
                expire()
            )])),
            OUTSIDE_EVERY_ALTERNATIVE.to_string(),
        ),
        (
            "an age count carrying no countUnit at all",
            document(json!([rule_with(
                count_selection("sinceImagePushed", 5),
                expire()
            )])),
            OUTSIDE_EVERY_ALTERNATIVE.to_string(),
        ),
        (
            "an action type outside the values the format defines",
            document(json!([rule_with(
                count_selection("imageCountMoreThan", 5),
                json!({ "type": "delete" })
            )])),
            OUTSIDE_EVERY_ALTERNATIVE.to_string(),
        ),
        (
            "a transition carrying no targetStorageClass",
            document(json!([rule_with(
                count_selection("imageCountMoreThan", 5),
                json!({ "type": "transition" })
            )])),
            OUTSIDE_EVERY_ALTERNATIVE.to_string(),
        ),
        (
            "a rulePriority under the minimum the format declares",
            document(json!([count_rule(0, 5)])),
            "numeric instance is lower than the required minimum (minimum: 1, found: 0)"
                .to_string(),
        ),
        (
            "a zero day count under the minimum the format declares",
            document(json!([rule_with(
                json!({
                    "tagStatus": "any",
                    "countType": "sinceImagePushed",
                    "countUnit": "days",
                    "countNumber": 0,
                }),
                expire()
            )])),
            "numeric instance is lower than the required minimum (minimum: 1, found: 0)"
                .to_string(),
        ),
    ];

    // The number the report echoes is the one the request carried, so the minimum rule is
    // asserted over a spread of counts under it — 0 is the recorded row.
    for count in [0_i64, -1, -7, -4096] {
        cases.push((
            "an image count under the minimum the format declares",
            document(json!([count_rule(1, count)])),
            format!(
                "numeric instance is lower than the required minimum (minimum: 1, found: {count})"
            ),
        ));
    }
    cases
}

/// Every rule the policy format states ACROSS a document's members: refused with a
/// sentence of ECR's own, which is not validator output.
fn cross_member_failures() -> Vec<(&'static str, String)> {
    let expire = || json!({ "type": "expire" });
    vec![
        (
            "a tagged rule naming no tag filter",
            document(json!([rule_with(
                json!({
                    "tagStatus": "tagged",
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                }),
                expire()
            )])),
        ),
        (
            "a tagged rule naming both tag filters",
            document(json!([rule_with(
                json!({
                    "tagStatus": "tagged",
                    "tagPrefixList": ["prod"],
                    "tagPatternList": ["prod*"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                }),
                expire()
            )])),
        ),
        (
            "an untagged rule carrying a tag filter",
            document(json!([rule_with(
                json!({
                    "tagStatus": "untagged",
                    "tagPrefixList": ["prod"],
                    "countType": "imageCountMoreThan",
                    "countNumber": 5,
                }),
                expire()
            )])),
        ),
        (
            "two rules sharing one rulePriority",
            document(json!([count_rule(1, 5), count_rule(1, 10)])),
        ),
        (
            "the catch-all rule evaluated before another",
            document(json!([
                count_rule(1, 10),
                {
                    "rulePriority": 2,
                    "selection": {
                        "tagStatus": "tagged",
                        "tagPrefixList": ["prod"],
                        "countType": "imageCountMoreThan",
                        "countNumber": 5,
                    },
                    "action": { "type": "expire" },
                },
            ])),
        ),
    ]
}

/// A schema failure quotes the validator's report as the validator emits it: the line,
/// then the newline that terminates it, inside the closing quote.
#[tokio::test]
async fn a_schema_refusal_quotes_the_validator_report_with_its_newline_terminator() {
    let router = emulator_with_repository().await;

    for (case, text, report) in schema_failures() {
        assert!(
            text.len() >= MIN_TEXT_LEN,
            "case {case:?}: the document must clear the modelled length bound so the \
             handler is what refuses it, got {} characters",
            text.len()
        );
        let expected = format!("{VALIDATION_PREFIX}{report}\n");

        for operation in READERS {
            let message = refusal(&router, case, operation, &text).await;
            let constraint = quoted_constraint(case, &message);
            assert_eq!(
                constraint, expected,
                "case {case:?}: expected {operation} to quote {expected:?}, got {constraint:?}"
            );
            assert!(
                constraint.ends_with('\n'),
                "case {case:?}: the quoted validator report must keep the newline that \
                 terminates it, got {constraint:?} from {operation}"
            );
            assert_eq!(
                constraint.matches('\n').count(),
                1,
                "case {case:?}: the report is one terminated line, got {constraint:?} from \
                 {operation}"
            );
            assert!(
                message.ends_with("\n'"),
                "case {case:?}: the constraint's closing quote follows the terminator, got \
                 {message:?} from {operation}"
            );
        }
    }
}

/// A rule the format states across members is ECR's own sentence, not validator output,
/// so nothing terminates it.
#[tokio::test]
async fn a_cross_member_refusal_quotes_a_sentence_with_no_validator_terminator() {
    let router = emulator_with_repository().await;

    for (case, text) in cross_member_failures() {
        for operation in READERS {
            let message = refusal(&router, case, operation, &text).await;
            let constraint = quoted_constraint(case, &message);
            let sentence = constraint.strip_prefix(VALIDATION_PREFIX).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a document that validates against the schema and breaks a \
                     format rule states the validation-failure constraint, got {constraint:?} \
                     from {operation}"
                )
            });
            assert!(
                !sentence.contains('\n'),
                "case {case:?}: ECR's own sentence carries no validator terminator, got \
                 {constraint:?} from {operation}"
            );
            assert!(
                sentence.ends_with('.'),
                "case {case:?}: ECR's own sentence closes on its full stop, got \
                 {constraint:?} from {operation}"
            );
        }
    }
}

/// A refused document changes nothing: the policy stored before it reads back exactly as
/// the put that stored it reported, whichever row was refused.
#[tokio::test]
async fn a_refused_document_leaves_the_stored_policy_unchanged() {
    let router = emulator_with_repository().await;

    let stored = document(json!([count_rule(1, 10)]));
    let put = ok(
        &router,
        "the policy in place before the refusals",
        "PutLifecyclePolicy",
        &json!({ "repositoryName": repository_name(), "lifecyclePolicyText": stored }),
    )
    .await;
    let reported = put["lifecyclePolicyText"].clone();

    let refused: Vec<(&'static str, String)> = schema_failures()
        .into_iter()
        .map(|(case, text, _)| (case, text))
        .chain(cross_member_failures())
        .collect();

    for (case, text) in refused {
        refusal(&router, case, "PutLifecyclePolicy", &text).await;
        let got = ok(
            &router,
            case,
            "GetLifecyclePolicy",
            &json!({ "repositoryName": repository_name() }),
        )
        .await;
        assert_eq!(
            got["lifecyclePolicyText"], reported,
            "case {case:?}: a refused put must leave the stored policy as the put that \
             stored it reported it, got {:?}",
            got["lifecyclePolicyText"]
        );
    }
}
