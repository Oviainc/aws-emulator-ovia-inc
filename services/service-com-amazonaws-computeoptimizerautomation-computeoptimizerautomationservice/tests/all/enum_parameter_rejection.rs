// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a value outside an enum PARAMETER TYPE's value set.
//!
//! The rule under test: a request that supplies, for a member targeting one of
//! this service's enum types, a value the model's value set for that type does
//! not hold is refused `InvalidParameterValueException` (HTTP 400) with a
//! message built from the request and the model — the TYPE whose set the value
//! missed, the value exactly as sent, and that type's modelled values:
//!
//! ```text
//! Invalid <EnumType> value: <value>. Valid values: <v1, v2, …>
//! ```
//!
//! It is asserted over a TABLE spanning the class rather than one request: every
//! enum parameter type of this service, on reporting and mutating operations
//! alike, at the top level of an input, inside a list, and nested in a
//! structure. The recorded `ListAutomationRulePreview` case is one row.
//!
//! Two things the rule does NOT reach are pinned here as well, because the rule
//! is about an enum TYPE and not about every refusal: a member constrained by
//! the model's legacy `@enum` trait on a plain string (a filter `name`) names no
//! parameter type and keeps AWS's generic validation report, and so does a
//! `@pattern` or `@length` failure.
//!
//! Every case drives the service over the wire through the shared awsJson1_0
//! harness in [`crate::automation_rule_lifecycle`].

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    COMPARISON_OPERATOR_VALUES, ENROLLMENT_STATUS_VALUES, RECOMMENDED_ACTION_TYPE_VALUES,
    RULE_APPLY_ORDER_VALUES, RULE_STATUS_VALUES, RULE_TYPE_VALUES,
};

use crate::automation_rule_lifecycle::{
    call, call_ok, emulator, error_code, minimal_rule, string_member,
};
use crate::error_payloads::error_message;

// ---------------------------------------------------------------------------
// The modelled value sets
//
// Each list below is the value set the model declares for one enum parameter
// type, IN DECLARATION ORDER — which is the order a rejection enumerates them
// in. Source:
// `smithy_query(query="shape",shape="RuleType,RuleStatus,EnrollmentStatus,RecommendedActionType,RuleApplyOrder,ComparisonOperator")`.
// `the_value_sets_under_test_are_the_model_s` below checks each one against the
// crate's generated, model-derived constant, so a model change fails here
// rather than silently.
// ---------------------------------------------------------------------------

const RULE_TYPE: &[&str] = &["OrganizationRule", "AccountRule"];
const RULE_STATUS: &[&str] = &["Active", "Inactive"];
const ENROLLMENT_STATUS: &[&str] = &["Active", "Inactive", "Pending", "Failed"];
const RECOMMENDED_ACTION_TYPE: &[&str] =
    &["SnapshotAndDeleteUnattachedEbsVolume", "UpgradeEbsVolumeType"];
const RULE_APPLY_ORDER: &[&str] = &["BeforeAccountRules", "AfterAccountRules"];
const COMPARISON_OPERATOR: &[&str] = &[
    "StringEquals",
    "StringNotEquals",
    "StringEqualsIgnoreCase",
    "StringNotEqualsIgnoreCase",
    "StringLike",
    "StringNotLike",
    "NumericEquals",
    "NumericNotEquals",
    "NumericLessThan",
    "NumericLessThanEquals",
    "NumericGreaterThan",
    "NumericGreaterThanEquals",
    "StringEqualsIfExists",
    "StringNotEqualsIfExists",
    "StringEqualsIgnoreCaseIfExists",
    "StringNotEqualsIgnoreCaseIfExists",
    "StringLikeIfExists",
    "StringNotLikeIfExists",
    "NumericEqualsIfExists",
    "NumericNotEqualsIfExists",
    "NumericLessThanIfExists",
    "NumericLessThanEqualsIfExists",
    "NumericGreaterThanIfExists",
    "NumericGreaterThanEqualsIfExists",
];

/// Every enum parameter type of this service, each with the values the model
/// declares for it and the generated constant that carries the same set. The
/// table below has to cover all of them: the rule is about an enum type, not
/// about the one type the recorded request named.
fn enum_parameter_types() -> Vec<(&'static str, &'static [&'static str], &'static [&'static str])> {
    vec![
        (
            "ComparisonOperator",
            COMPARISON_OPERATOR,
            COMPARISON_OPERATOR_VALUES,
        ),
        (
            "EnrollmentStatus",
            ENROLLMENT_STATUS,
            ENROLLMENT_STATUS_VALUES,
        ),
        (
            "RecommendedActionType",
            RECOMMENDED_ACTION_TYPE,
            RECOMMENDED_ACTION_TYPE_VALUES,
        ),
        ("RuleApplyOrder", RULE_APPLY_ORDER, RULE_APPLY_ORDER_VALUES),
        ("RuleStatus", RULE_STATUS, RULE_STATUS_VALUES),
        ("RuleType", RULE_TYPE, RULE_TYPE_VALUES),
    ]
}

/// One row of the rule's table: the case name, the operation, the request, the
/// enum parameter type the request misses, the value it sent, and that type's
/// modelled values.
type EnumCase = (
    &'static str,
    &'static str,
    Value,
    &'static str,
    &'static str,
    &'static [&'static str],
);

/// One row of the update table: the case name, the `recommendedActionTypes` it
/// sends, and the type, value and value set the rejection must state.
type UpdateCase = (
    &'static str,
    Value,
    &'static str,
    &'static str,
    &'static [&'static str],
);

/// The message the rule prescribes for `value` sent to a member of `enum_type`.
fn expected_message(enum_type: &str, value: &str, valid: &[&str]) -> String {
    format!(
        "Invalid {enum_type} value: {value}. Valid values: {}",
        valid.join(", ")
    )
}

/// Assert one refusal: HTTP 400, `InvalidParameterValueException`, and exactly
/// the message the rule prescribes — plus the three shapes of diagnostic the
/// rejection must NOT be: a constraint-clause count, a dotted member path, or a
/// bracketed value-set dump.
fn assert_enum_rejection(
    response: &(u16, Value),
    enum_type: &str,
    value: &str,
    valid: &[&str],
    case: &str,
) {
    let (status, body) = response;
    let message = error_message(body);
    assert_eq!(
        *status, 400,
        "case {case:?}: expected HTTP 400, got {status} and {body}"
    );
    assert_eq!(
        error_code(body),
        "InvalidParameterValueException",
        "case {case:?}: expected error code InvalidParameterValueException, got {body}"
    );
    assert_eq!(
        message,
        expected_message(enum_type, value, valid),
        "case {case:?}: expected the type's own rejection, got {message:?}"
    );
    assert!(
        !message.contains("validation error"),
        "case {case:?}: the rejection is not a constraint-clause count, got {message:?}"
    );
    assert!(
        !message.contains(".member") && !message.contains('\''),
        "case {case:?}: the rejection names the type, not a member path, got {message:?}"
    );
    assert!(
        !message.contains('[') && !message.contains(']'),
        "case {case:?}: the valid values are listed, not dumped as a set, got {message:?}"
    );
}

/// A rule request whose `ruleType` is `OrganizationRule`, with the
/// `organizationConfiguration` that type requires.
fn organization_rule(name: &str, rule_apply_order: &str) -> Value {
    let mut request = minimal_rule(name);
    request["ruleType"] = json!("OrganizationRule");
    request["organizationConfiguration"] = json!({
        "ruleApplyOrder": rule_apply_order,
        "accountIds": ["012345678901"]
    });
    request
}

/// A preview request with a single criteria condition on `region`, so a
/// `comparison` outside `ComparisonOperator` can be sent.
fn preview_with_comparison(comparison: &str) -> Value {
    json!({
        "ruleType": "AccountRule",
        "recommendedActionTypes": ["UpgradeEbsVolumeType"],
        "criteria": {"region": [{"comparison": comparison, "values": ["us-east-1"]}]}
    })
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// A value outside an enum parameter type's modelled set is refused with that
/// type's own rejection — on every enum parameter type of this service, and
/// wherever in an input the value sits.
#[tokio::test]
async fn a_value_outside_an_enum_parameter_type_names_the_type_the_value_and_the_valid_set() {
    // (case, operation, request, enum type, value sent, that type's values)
    let cases: Vec<EnumCase> = vec![
        // The recorded request: one unrecognized action type in the required
        // `recommendedActionTypes` list.
        (
            "preview-action-type-unrecognized",
            "ListAutomationRulePreview",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": ["NotARealActionType"],
                "maxResults": 10
            }),
            "RecommendedActionType",
            "NotARealActionType",
            RECOMMENDED_ACTION_TYPE,
        ),
        // Not the first element: the rejection names the type, so no index or
        // `.member` segment can appear in it.
        (
            "preview-action-type-bad-second-element",
            "ListAutomationRulePreview",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": ["UpgradeEbsVolumeType", "DeleteEverything"]
            }),
            "RecommendedActionType",
            "DeleteEverything",
            RECOMMENDED_ACTION_TYPE,
        ),
        // The sibling list operation takes the identical member.
        (
            "preview-summaries-action-type-wrong-case",
            "ListAutomationRulePreviewSummaries",
            json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": ["upgradeebsvolumetype"]
            }),
            "RecommendedActionType",
            "upgradeebsvolumetype",
            RECOMMENDED_ACTION_TYPE,
        ),
        // A MUTATING operation takes the same member, and is refused the same
        // way: the rule is about the parameter type, not about previewing.
        (
            "create-action-type-pluralized",
            "CreateAutomationRule",
            {
                let mut request = minimal_rule("action-type-pluralized");
                request["recommendedActionTypes"] = json!(["UpgradeEbsVolumeTypes"]);
                request
            },
            "RecommendedActionType",
            "UpgradeEbsVolumeTypes",
            RECOMMENDED_ACTION_TYPE,
        ),
        // An empty string is a value the caller sent, so it is named as sent.
        (
            "create-action-type-empty",
            "CreateAutomationRule",
            {
                let mut request = minimal_rule("action-type-empty");
                request["recommendedActionTypes"] = json!([""]);
                request
            },
            "RecommendedActionType",
            "",
            RECOMMENDED_ACTION_TYPE,
        ),
        // A top-level enum member, on a mutating and on a reporting operation.
        (
            "create-rule-type-unrecognized",
            "CreateAutomationRule",
            {
                let mut request = minimal_rule("rule-type-unrecognized");
                request["ruleType"] = json!("RegionRule");
                request
            },
            "RuleType",
            "RegionRule",
            RULE_TYPE,
        ),
        (
            "preview-rule-type-wrong-case",
            "ListAutomationRulePreview",
            json!({
                "ruleType": "accountrule",
                "recommendedActionTypes": ["UpgradeEbsVolumeType"]
            }),
            "RuleType",
            "accountrule",
            RULE_TYPE,
        ),
        (
            "create-status-outside-the-enum",
            "CreateAutomationRule",
            {
                let mut request = minimal_rule("status-outside-the-enum");
                request["status"] = json!("Paused");
                request
            },
            "RuleStatus",
            "Paused",
            RULE_STATUS,
        ),
        // `status` on this operation targets a DIFFERENT enum type, and the
        // rejection names the type the member actually targets.
        (
            "enrollment-status-outside-the-enum",
            "UpdateEnrollmentConfiguration",
            json!({"status": "Enabled"}),
            "EnrollmentStatus",
            "Enabled",
            ENROLLMENT_STATUS,
        ),
        // Nested in a structure member.
        (
            "create-rule-apply-order-unrecognized",
            "CreateAutomationRule",
            organization_rule("rule-apply-order", "Last"),
            "RuleApplyOrder",
            "Last",
            RULE_APPLY_ORDER,
        ),
        // Nested in a list of structures, two levels down.
        (
            "preview-comparison-unrecognized",
            "ListAutomationRulePreview",
            preview_with_comparison("StringIsh"),
            "ComparisonOperator",
            "StringIsh",
            COMPARISON_OPERATOR,
        ),
        // A request that fails an enum parameter AND another constraint is
        // answered for the parameter type: the service judges parameters, so
        // the enum rejection is not demoted to one clause of a count.
        (
            "create-rule-type-and-name-both-bad",
            "CreateAutomationRule",
            {
                let mut request = minimal_rule("not a legal name!");
                request["ruleType"] = json!("RegionRule");
                request
            },
            "RuleType",
            "RegionRule",
            RULE_TYPE,
        ),
    ];

    let router = emulator();
    let mut covered: Vec<&str> = Vec::new();
    for (case, operation, request, enum_type, value, valid) in &cases {
        let response = call(&router, operation, request).await;
        assert_enum_rejection(&response, enum_type, value, valid, case);
        covered.push(enum_type);
    }

    // Every enum parameter type of this service is in the table above.
    for (enum_type, _, _) in enum_parameter_types() {
        assert!(
            covered.contains(&enum_type),
            "case \"coverage\": {enum_type} has no row, covered: {covered:?}"
        );
    }

    // No refused request left a rule behind, so the refusals above are about
    // the value and nothing was half-applied.
    let listed = call_ok(&router, "ListAutomationRules", &json!({}), "nothing-stored").await;
    assert_eq!(
        listed.get("automationRules"),
        Some(&json!([])),
        "case \"nothing-stored\": no refused request may store a rule, got {listed}"
    );
}

/// The same rule on `UpdateAutomationRule`, which needs a rule to update first —
/// so the refusal is reached past this service's resource authorization, and the
/// stored rule is left as it stood.
#[tokio::test]
async fn an_update_with_a_value_outside_an_enum_parameter_type_names_the_type() {
    let router = emulator();
    let created = call_ok(
        &router,
        "CreateAutomationRule",
        &minimal_rule("update-enum-rejection"),
        "created",
    )
    .await;
    let rule_arn = string_member(&created, "ruleArn");
    let revision = created
        .get("ruleRevision")
        .and_then(Value::as_i64)
        .unwrap_or_default();

    // (case, the member and value that misses its type, enum type, value, set)
    let cases: Vec<UpdateCase> = vec![
        (
            "update-action-type-screaming",
            json!(["SNAPSHOTANDDELETEUNATTACHEDEBSVOLUME"]),
            "RecommendedActionType",
            "SNAPSHOTANDDELETEUNATTACHEDEBSVOLUME",
            RECOMMENDED_ACTION_TYPE,
        ),
        (
            "update-action-type-truncated",
            json!(["UpgradeEbsVolume"]),
            "RecommendedActionType",
            "UpgradeEbsVolume",
            RECOMMENDED_ACTION_TYPE,
        ),
    ];

    for (case, action_types, enum_type, value, valid) in &cases {
        let response = call(
            &router,
            "UpdateAutomationRule",
            &json!({
                "ruleArn": rule_arn,
                "ruleRevision": revision,
                "recommendedActionTypes": action_types
            }),
        )
        .await;
        assert_enum_rejection(&response, enum_type, value, valid, case);

        let read_back = call_ok(
            &router,
            "GetAutomationRule",
            &json!({"ruleArn": rule_arn}),
            case,
        )
        .await;
        assert_eq!(
            read_back.get("ruleRevision").and_then(Value::as_i64),
            Some(revision),
            "case {case:?}: a refused update leaves the rule's revision alone, got {read_back}"
        );
        assert_eq!(
            read_back.get("recommendedActionTypes"),
            created.get("recommendedActionTypes"),
            "case {case:?}: a refused update leaves the rule's action types alone, got {read_back}"
        );
    }
}

/// The rule holds across a parameter type's whole value space, not for one
/// token: every near miss of a modelled value is refused by it, and every
/// modelled value itself is accepted. The cases are GENERATED from the crate's
/// generated, model-derived value set, so they follow the model rather than a
/// list written here.
#[tokio::test]
async fn every_near_miss_of_a_modelled_action_type_is_refused_by_the_rule() {
    let router = emulator();

    for modelled in RECOMMENDED_ACTION_TYPE_VALUES {
        let truncated = modelled
            .char_indices()
            .next_back()
            .map_or_else(String::new, |(at, _)| modelled[..at].to_string());
        let mutations = [
            ("lowercased", modelled.to_lowercase()),
            ("uppercased", modelled.to_uppercase()),
            ("suffixed", format!("{modelled}s")),
            ("prefixed", format!("Not{modelled}")),
            ("truncated", truncated),
            ("trailing-space", format!("{modelled} ")),
        ];

        for (mutation, value) in &mutations {
            let case = format!("{modelled}-{mutation}");
            assert!(
                !RECOMMENDED_ACTION_TYPE_VALUES.contains(&value.as_str()),
                "case {case:?}: the mutation must not be a modelled value, got {value:?}"
            );
            let response = call(
                &router,
                "ListAutomationRulePreview",
                &json!({
                    "ruleType": "AccountRule",
                    "recommendedActionTypes": [value]
                }),
            )
            .await;
            assert_enum_rejection(
                &response,
                "RecommendedActionType",
                value,
                RECOMMENDED_ACTION_TYPE,
                &case,
            );
        }

        // Positive control: the modelled value the mutations came from is
        // accepted, so the refusals are about the value and not the request.
        let accepted = call_ok(
            &router,
            "ListAutomationRulePreview",
            &json!({
                "ruleType": "AccountRule",
                "recommendedActionTypes": [modelled]
            }),
            modelled,
        )
        .await;
        assert!(
            accepted.get("previewResults").is_some(),
            "case {modelled:?}: a modelled action type is previewed, got {accepted}"
        );
    }
}

/// A value far longer than the framework echoes is still named, as the bounded
/// prefix the framework carries: the message a refusal returns cannot be grown
/// without bound by the request.
#[tokio::test]
async fn a_very_long_rejected_value_is_named_within_a_bound() {
    let router = emulator();
    let long = "A".repeat(1000);
    let response = call(
        &router,
        "ListAutomationRulePreview",
        &json!({"ruleType": "AccountRule", "recommendedActionTypes": [long]}),
    )
    .await;
    let message = error_message(&response.1);

    assert_eq!(
        response.0, 400,
        "case \"a-very-long-value\": expected HTTP 400, got {} and {}",
        response.0, response.1
    );
    assert!(
        message.starts_with("Invalid RecommendedActionType value: AAA"),
        "case \"a-very-long-value\": the rejection names the value as sent, got {message:?}"
    );
    assert!(
        message.contains("... [1000 bytes]"),
        "case \"a-very-long-value\": a value past the echo cap is elided, got {message:?}"
    );
    assert!(
        message.len() < 600,
        "case \"a-very-long-value\": the message stays bounded, got {} characters",
        message.len()
    );
}

// ---------------------------------------------------------------------------
// What the rule does NOT reach
// ---------------------------------------------------------------------------

/// A refusal that names no enum parameter type keeps AWS's generic validation
/// report: a plain string constrained by the model's legacy `@enum` trait (a
/// filter `name`, which is not an enum shape), and a `@pattern` or `@length`
/// failure.
#[tokio::test]
async fn a_refusal_with_no_enum_parameter_type_keeps_the_generic_report() {
    let router = emulator();

    // (case, operation, request, expected message)
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "a-filter-name-outside-the-value-set",
            "ListAutomationRules",
            json!({"filters": [{"name": "Nickname", "values": ["x"]}]}),
            "1 validation error detected: Value at 'filters.1.member.name' failed to satisfy \
             constraint: Member must satisfy enum value set: [Name, RecommendedActionType, \
             Status, RuleType, OrganizationConfigurationRuleApplyOrder, AccountId]"
                .to_string(),
        ),
        (
            "a-name-outside-its-pattern",
            "CreateAutomationRule",
            minimal_rule("not a legal name!"),
            "1 validation error detected: Value at 'name' failed to satisfy constraint: Member \
             must satisfy regular expression pattern: [a-zA-Z0-9_-]*"
                .to_string(),
        ),
        (
            "max-results-past-its-range",
            "ListAutomationRules",
            json!({"maxResults": 101}),
            "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: \
             Member must have value less than or equal to 100"
                .to_string(),
        ),
    ];

    for (case, operation, request, expected) in &cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} and {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected error code InvalidParameterValueException, got {body}"
        );
        assert_eq!(
            &error_message(&body),
            expected,
            "case {case:?}: expected the generic validation report, got {:?}",
            error_message(&body)
        );
    }
}

/// The value sets this suite asserts over are the model's, read back from the
/// crate's generated value constants: a type whose modelled set moved fails
/// HERE rather than leaving a stale set in an assertion above.
#[test]
fn the_value_sets_under_test_are_the_model_s() {
    for (enum_type, declared, generated) in enum_parameter_types() {
        let mut declared_sorted = declared.to_vec();
        declared_sorted.sort_unstable();
        let mut generated_sorted = generated.to_vec();
        generated_sorted.sort_unstable();
        assert_eq!(
            declared_sorted, generated_sorted,
            "case {enum_type:?}: the suite's value set must be the model's, got {declared:?} \
             against {generated:?}"
        );
    }
}
