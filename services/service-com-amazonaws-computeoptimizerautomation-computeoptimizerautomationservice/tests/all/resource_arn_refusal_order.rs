// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a request that names a resource by ARN receives, and in what
//! order the gates it passes choose it.
//!
//! The rule under test: for an operation whose input names a resource by ARN,
//! the refusal is decided by where the request FIRST fails, in this order —
//! authorization against the resource string exactly as supplied
//! (`AccessDeniedException`, 403), then the modelled-constraint walk over the
//! request's remaining members (`InvalidParameterValueException`, 400), then the
//! lookup of the rule that ARN names (`ResourceNotFoundException`, 404). A
//! string that names no automation rule of this service is therefore never
//! answered as a bad parameter — not even when another member of the same
//! request really does break a modelled constraint — while that same broken
//! member IS the 400 once the resource string names a rule.
//!
//! The rows vary what the rule generalises over: the OPERATION (every one whose
//! input names a resource by ARN, which is `ruleArn` on the three rule
//! operations and `resourceArn` on the three tagging ones), the FORM the
//! resource string takes (several ways it can fail to name a rule of this
//! service, and several well-formed ARNs no rule has) and WHICH other member of
//! the request is outside its modelled constraint.
//!
//! The 403 itself, and the sentence it carries, are
//! [`crate::automation_event_and_tagging`] and
//! [`crate::unauthorized_resource_message`]; this suite is about the ORDER the
//! three refusals are chosen in, which is what each of these operations'
//! contract docs states.
//!
//! Every case drives the service through the awsJson1_0 router a real caller
//! reaches, using [`crate::automation_rule_lifecycle`]'s harness.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;

use crate::automation_rule_lifecycle::{
    assert_refused, call, create_rule, emulator, member, read_rule,
};

/// Every operation of this service whose input names a resource by ARN, and the
/// member each one names it in: `ruleArn` on the three rule operations and
/// `resourceArn` on the three tagging ones
/// (`smithy_query(query="operation",operation="GetAutomationRule,UpdateAutomationRule,DeleteAutomationRule,TagResource,UntagResource,ListTagsForResource")`).
const RESOURCE_OPERATIONS: [&str; 6] = [
    "GetAutomationRule",
    "UpdateAutomationRule",
    "DeleteAutomationRule",
    "TagResource",
    "UntagResource",
    "ListTagsForResource",
];

/// The tag every row's rule starts with, so a refused untagging is observable in
/// the stored list.
fn one_tag() -> Value {
    json!([{"key": "team", "value": "platform"}])
}

/// Resource strings that name no automation rule of this service: one row per way
/// the modelled `RuleArn` form
/// (`^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$`, from
/// `smithy_query(query="shape",shape="RuleArn")`) can fail.
fn strings_naming_no_rule() -> Vec<(&'static str, String)> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    vec![
        ("no-arn-at-all", "a-rule-of-mine".to_string()),
        ("empty", String::new()),
        (
            "another-resource-type",
            format!("arn:aws:compute-optimizer::{account}:rule/0123456789abcdef"),
        ),
        (
            "a-region-segment",
            format!(
                "arn:aws:compute-optimizer:us-east-1:{account}:automation-rule/0123456789abcdef"
            ),
        ),
        (
            "an-account-that-is-not-twelve-digits",
            "arn:aws:compute-optimizer::7:automation-rule/0123456789abcdef".to_string(),
        ),
    ]
}

/// Well-formed `RuleArn`s of this service for rule ids no create ever mints, so
/// each one passes authorization and fails the lookup. The ids span the forms the
/// pattern admits after `automation-rule/` (`[a-zA-Z0-9_-]+`): the 16-character
/// shape a created rule carries, a one-character id, and one using the
/// punctuation the class allows.
fn absent_rule_arns() -> Vec<(&'static str, String)> {
    ["0123456789abcdef", "a", "never_created-rule"]
        .into_iter()
        .map(|rule_id| (rule_id, state::rule_arn(rule_id)))
        .collect()
}

/// The request `operation` sends for `resource`, with every other member it
/// models filled in with a value inside that member's constraints — so a row is
/// refused for the resource string or the lookup, never for anything else.
fn valid_request(operation: &str, resource: &str, revision: i64) -> Value {
    match operation {
        "GetAutomationRule" => json!({"ruleArn": resource}),
        "UpdateAutomationRule" => json!({
            "ruleArn": resource,
            "ruleRevision": revision,
            "description": "left alone",
        }),
        "DeleteAutomationRule" => json!({"ruleArn": resource, "ruleRevision": revision}),
        "ListTagsForResource" => json!({"resourceArn": resource}),
        "TagResource" => json!({
            "resourceArn": resource,
            "ruleRevision": revision,
            "tags": [{"key": "owner", "value": "platform"}],
        }),
        "UntagResource" => json!({
            "resourceArn": resource,
            "ruleRevision": revision,
            "tagKeys": ["team"],
        }),
        other => panic!("no request is defined for {other}"),
    }
}

/// The same request with ONE member other than the resource ARN put outside its
/// modelled constraint, as `(the member, the request)`.
///
/// The constraints are the model's own: `RuleDescription` @pattern
/// `^[a-zA-Z0-9_\-\s@\.]*$`, `Tag$key` (target `TagKey`) @pattern
/// `^[\w\s\.\-\:\/\=\+\@]+$` and `ClientToken` @pattern `^[a-zA-Z0-9_-]{1,64}$`
/// (`smithy_query(query="shape",shape="RuleDescription,TagKey,ClientToken")`).
///
/// `None` for the two operations whose input models the resource ARN and nothing
/// else (`GetAutomationRule` and `ListTagsForResource`): there is no other member
/// to break, so for those two the resource ARN's own `@pattern` is the only
/// modelled constraint a request can fail, and the 403 it is answered with
/// instead of that pattern's 400 is itself the ordering.
fn request_with_another_member_invalid(
    operation: &str,
    resource: &str,
    revision: i64,
) -> Option<(&'static str, Value)> {
    let mut request = valid_request(operation, resource, revision);
    match operation {
        "GetAutomationRule" | "ListTagsForResource" => None,
        "UpdateAutomationRule" => {
            request["description"] = json!("a description with a % in it");
            Some(("description", request))
        }
        "TagResource" => {
            request["tags"] = json!([{"key": "owner!", "value": "platform"}]);
            Some(("tags", request))
        }
        // `TagKeyList`'s member is an unconstrained `String`, so the member these
        // two break is the one every mutation of this service models: the
        // idempotency token.
        "DeleteAutomationRule" | "UntagResource" => {
            request["clientToken"] = json!("not a legal token!");
            Some(("clientToken", request))
        }
        other => panic!("no request is defined for {other}"),
    }
}

/// The tag keys the store holds for `rule_arn`, read back through the implemented
/// reader, in the order it reports them.
async fn stored_tag_keys(
    router: &smithy_server_runtime::MultiServiceRouter,
    rule_arn: &str,
    case: &str,
) -> Vec<String> {
    let rule = read_rule(router, rule_arn, case).await;
    member(&rule, "tags")
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    tag.get("key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Rank 1: the resource string is judged first. A string that names no automation
/// rule of this service is 403 `AccessDeniedException` whether the rest of the
/// request is sound or carries a member outside its own modelled constraint.
#[tokio::test]
async fn a_string_naming_no_rule_is_refused_before_any_other_member_is_judged() {
    for operation in RESOURCE_OPERATIONS {
        let router = emulator();
        for (form, resource) in strings_naming_no_rule() {
            let sound = format!("{operation}/{form}: every other member sound");
            assert_refused(
                call(&router, operation, &valid_request(operation, &resource, 1)).await,
                403,
                "AccessDeniedException",
                &sound,
            );

            if let Some((broken, request)) =
                request_with_another_member_invalid(operation, &resource, 1)
            {
                let case = format!("{operation}/{form}: {broken} also outside its constraint");
                assert_refused(
                    call(&router, operation, &request).await,
                    403,
                    "AccessDeniedException",
                    &case,
                );
            }
        }
    }
}

/// Rank 2: once the resource string names a rule of this service, a member
/// outside its modelled constraint is the 400 `InvalidParameterValueException`
/// the walk answers — and the refused call leaves the rule exactly as it was.
#[tokio::test]
async fn an_authorized_arn_with_another_member_invalid_is_a_parameter_refusal() {
    for operation in RESOURCE_OPERATIONS {
        let router = emulator();
        let (rule_arn, revision) = create_rule(
            &router,
            "refusal-order",
            "Active",
            &one_tag(),
            &format!("{operation}: setup"),
        )
        .await;

        let Some((broken, request)) =
            request_with_another_member_invalid(operation, &rule_arn, revision)
        else {
            // The two readers model no member but the resource ARN, so this rank
            // has no row for them; their ordering is the rank-1 case above.
            continue;
        };
        let case = format!("{operation}: {broken} outside its constraint");
        assert_refused(
            call(&router, operation, &request).await,
            400,
            "InvalidParameterValueException",
            &case,
        );

        let after = read_rule(&router, &rule_arn, &case).await;
        assert_eq!(
            member(&after, "ruleRevision").as_i64(),
            Some(revision),
            "case {case:?}: a refused request must leave the rule at revision {revision}, \
             got {after}"
        );
        let keys = stored_tag_keys(&router, &rule_arn, &case).await;
        assert_eq!(
            keys,
            vec!["team".to_string()],
            "case {case:?}: a refused request must leave the stored tags alone, got {keys:?}"
        );
    }
}

/// Rank 3: a well-formed `RuleArn` that no rule has passes authorization and the
/// walk, and is answered by the lookup — 404 `ResourceNotFoundException`, for
/// every operation and every form of rule id the pattern admits.
#[tokio::test]
async fn a_well_formed_arn_no_rule_has_reaches_the_lookup() {
    for operation in RESOURCE_OPERATIONS {
        let router = emulator();
        for (rule_id, rule_arn) in absent_rule_arns() {
            let case = format!("{operation}/{rule_id}");
            assert_refused(
                call(&router, operation, &valid_request(operation, &rule_arn, 1)).await,
                404,
                "ResourceNotFoundException",
                &case,
            );
        }
    }
}
