// SPDX-License-Identifier: CC0-1.0

//! How this service words a 404 for a rule the store does not hold.
//!
//! One rule, asserted over a TABLE rather than one recorded request: an
//! operation that MUTATES an automation rule through the ARN it was sent reports
//! the absence by naming that ARN, while an operation that only REPORTS on the
//! same rule answers the service's fixed, resource-independent sentence. Both
//! halves run over the same set of ARNs, so the two wordings are distinguished
//! by the operation's kind and by nothing else.
//!
//! The mutations are the four operations whose ARN member targets the model's
//! `RuleArn` and which are not `@readonly`
//! (`smithy_query(query="operation",operation="UpdateAutomationRule,DeleteAutomationRule,TagResource,UntagResource")`);
//! the reports are the two that are
//! (`GetAutomationRule`, `ListTagsForResource`).
//!
//! Every case drives the service through the awsJson1_0 router a real caller
//! reaches, using [`crate::automation_rule_lifecycle`]'s harness.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;

use crate::automation_rule_lifecycle::{
    call, call_ok, emulator, error_code, member, minimal_rule, string_member,
};
use crate::error_payloads::error_message;

/// The fixed sentence a REPORTING operation's not-found carries.
const FIXED_NOT_FOUND_MESSAGE: &str = "The specified resource could not be found";

/// The message a MUTATION's not-found carries, as the rule under test states it:
/// the ARN the request supplied, and nothing else from the request.
fn expected_mutation_message(rule_arn: &str) -> String {
    format!("Rule with ARN {rule_arn} does not exist")
}

/// The id segment of a rule ARN, i.e. everything after `automation-rule/`.
fn rule_id_of(rule_arn: &str) -> String {
    rule_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The four mutations of a rule, each sent exactly the members it models
/// (`revision` is the optimistic-concurrency token every one of them requires).
///
/// `TagKey`'s form is `@length(min: 1, max: 128)` and
/// `@pattern ^[\w\s\.\-\:\/\=\+\@]+$`
/// (`smithy_query(query="shape",shape="TagKey")`), which `owner` satisfies, so
/// no case is refused before the rule lookup it is about.
fn mutation_request(operation: &str, rule_arn: &str, revision: i64) -> Value {
    match operation {
        "UpdateAutomationRule" => json!({
            "ruleArn": rule_arn,
            "ruleRevision": revision,
            "description": "must not be applied",
        }),
        "DeleteAutomationRule" => json!({"ruleArn": rule_arn, "ruleRevision": revision}),
        "TagResource" => json!({
            "resourceArn": rule_arn,
            "ruleRevision": revision,
            "tags": [{"key": "owner", "value": "team"}],
        }),
        "UntagResource" => json!({
            "resourceArn": rule_arn,
            "ruleRevision": revision,
            "tagKeys": ["owner"],
        }),
        other => panic!("{other} is not one of this suite's rule mutations"),
    }
}

/// The report requests, keyed the way each operation names the ARN.
fn report_request(operation: &str, rule_arn: &str) -> Value {
    match operation {
        "GetAutomationRule" => json!({"ruleArn": rule_arn}),
        "ListTagsForResource" => json!({"resourceArn": rule_arn}),
        other => panic!("{other} is not one of this suite's rule reports"),
    }
}

/// Assert a refusal's status, code and exact message.
fn assert_not_found(response: &(u16, Value), expected_message: &str, case: &str) {
    let (status, body) = response;
    assert_eq!(
        *status, 404,
        "case {case:?}: expected HTTP 404, got {status} and {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: expected error code ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        error_message(body),
        expected_message,
        "case {case:?}: expected message {expected_message:?}, got {:?}",
        error_message(body)
    );
}

/// Rule ids spanning the forms the model's `RuleArn` admits after
/// `automation-rule/` (`[a-zA-Z0-9_-]+`, from
/// `smithy_query(query="shape",shape="RuleArn")`): the 16-hex-character shape a
/// created rule carries, a one-character id, the punctuation the class allows
/// and a long mixed-case id. None of them is ever created, so every ARN built
/// from one names no rule.
const ABSENT_RULE_IDS: [&str; 5] = [
    "0123456789abcdef",
    "a",
    "never_created-rule",
    "RULE-0000000000000000",
    "MixedCase9_id-0",
];

/// A mutation against a rule ARN the store does not hold names THAT ARN, for
/// every mutation of the resource and every form the ARN can take — including
/// the ARN of a rule that existed and was deleted.
#[tokio::test]
async fn a_mutation_of_an_absent_rule_names_the_arn_it_was_sent() {
    let operations = [
        "UpdateAutomationRule",
        "DeleteAutomationRule",
        "TagResource",
        "UntagResource",
    ];

    for operation in operations {
        let router = emulator();

        // The ARN of a rule that DID exist: created, then deleted through the
        // wire, so the store's own ARN form is one of the cases.
        let created = call_ok(&router, "CreateAutomationRule", &minimal_rule("gone"), operation).await;
        let deleted_arn = string_member(&created, "ruleArn");
        let revision = member(&created, "ruleRevision").as_i64().unwrap_or_default();
        call_ok(
            &router,
            "DeleteAutomationRule",
            &json!({"ruleArn": deleted_arn, "ruleRevision": revision}),
            operation,
        )
        .await;

        let mut arns = vec![deleted_arn];
        arns.extend(ABSENT_RULE_IDS.iter().map(|id| state::rule_arn(id)));

        for arn in &arns {
            let case = format!("{operation} on {arn}");
            let response = call(&router, operation, &mutation_request(operation, arn, revision)).await;
            assert_not_found(&response, &expected_mutation_message(arn), &case);
            let message = error_message(&response.1);
            assert!(
                message.contains(arn),
                "case {case:?}: the refusal must name the ARN the request sent, got {message:?}"
            );
        }
    }
}

/// A report on the same absent rules keeps the service's fixed sentence, and
/// discloses neither the ARN nor the rule id.
#[tokio::test]
async fn a_report_on_an_absent_rule_keeps_the_fixed_sentence() {
    let operations = ["GetAutomationRule", "ListTagsForResource"];

    for operation in operations {
        let router = emulator();
        let created = call_ok(&router, "CreateAutomationRule", &minimal_rule("gone"), operation).await;
        let deleted_arn = string_member(&created, "ruleArn");
        let revision = member(&created, "ruleRevision").as_i64().unwrap_or_default();
        call_ok(
            &router,
            "DeleteAutomationRule",
            &json!({"ruleArn": deleted_arn, "ruleRevision": revision}),
            operation,
        )
        .await;

        let mut arns = vec![deleted_arn];
        arns.extend(ABSENT_RULE_IDS.iter().map(|id| state::rule_arn(id)));

        for arn in &arns {
            let case = format!("{operation} on {arn}");
            let response = call(&router, operation, &report_request(operation, arn)).await;
            assert_not_found(&response, FIXED_NOT_FOUND_MESSAGE, &case);
            let message = error_message(&response.1);
            assert!(
                !message.contains(arn) && !message.contains(&rule_id_of(arn)),
                "case {case:?}: a report's not-found must echo no resource identifier, \
                 got {message:?}"
            );
        }
    }
}

/// The two wordings over ONE rule's whole life: while the rule exists every
/// operation succeeds and the mutations read back through
/// `GetAutomationRule`; once it is deleted the four mutations name its ARN and
/// the two reports do not.
#[tokio::test]
async fn the_wording_splits_only_once_the_rule_is_gone() {
    let router = emulator();
    let case = "lifecycle";

    let created = call_ok(&router, "CreateAutomationRule", &minimal_rule("lifecycle"), case).await;
    let rule_arn = string_member(&created, "ruleArn");
    let revision = member(&created, "ruleRevision").as_i64().unwrap_or_default();

    // While the rule is there, the same requests are served: the tag addition
    // and removal and the update all apply, and the round trip through the read
    // proves each one reached the store.
    call_ok(
        &router,
        "TagResource",
        &mutation_request("TagResource", &rule_arn, revision),
        case,
    )
    .await;
    let tags = call_ok(
        &router,
        "ListTagsForResource",
        &report_request("ListTagsForResource", &rule_arn),
        case,
    )
    .await;
    assert_eq!(
        member(&tags, "tags"),
        &json!([{"key": "owner", "value": "team"}]),
        "case {case:?}: the addition must be the tag a later read sees, got {tags}"
    );

    call_ok(
        &router,
        "UntagResource",
        &mutation_request("UntagResource", &rule_arn, revision),
        case,
    )
    .await;
    let updated = call_ok(
        &router,
        "UpdateAutomationRule",
        &mutation_request("UpdateAutomationRule", &rule_arn, revision),
        case,
    )
    .await;
    let bumped = member(&updated, "ruleRevision").as_i64().unwrap_or_default();
    let read_back = call_ok(
        &router,
        "GetAutomationRule",
        &report_request("GetAutomationRule", &rule_arn),
        case,
    )
    .await;
    assert_eq!(
        member(&read_back, "ruleRevision").as_i64(),
        Some(bumped),
        "case {case:?}: the update must be what a later read sees, got {read_back}"
    );
    assert_eq!(
        member(&read_back, "description").as_str(),
        Some("must not be applied"),
        "case {case:?}: the update must be what a later read sees, got {read_back}"
    );
    let tags_after_removal = member(&read_back, "tags");
    assert!(
        tags_after_removal
            .as_array()
            .is_none_or(|tags| tags.is_empty()),
        "case {case:?}: the removal must be what a later read sees, got {tags_after_removal}"
    );

    call_ok(
        &router,
        "DeleteAutomationRule",
        &json!({"ruleArn": rule_arn, "ruleRevision": bumped}),
        case,
    )
    .await;

    for operation in [
        "UpdateAutomationRule",
        "DeleteAutomationRule",
        "TagResource",
        "UntagResource",
    ] {
        let response = call(
            &router,
            operation,
            &mutation_request(operation, &rule_arn, bumped),
        )
        .await;
        assert_not_found(
            &response,
            &expected_mutation_message(&rule_arn),
            &format!("{case}: {operation} after the delete"),
        );
    }
    for operation in ["GetAutomationRule", "ListTagsForResource"] {
        let response = call(&router, operation, &report_request(operation, &rule_arn)).await;
        assert_not_found(
            &response,
            FIXED_NOT_FOUND_MESSAGE,
            &format!("{case}: {operation} after the delete"),
        );
    }
}
