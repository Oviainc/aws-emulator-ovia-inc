// SPDX-License-Identifier: CC0-1.0

//! How this service judges the VALUES a rule is made of, on every path that
//! writes or evaluates one.
//!
//! The rule under test: a value stored into a rule must satisfy the documented
//! constraints the service holds that member to, whichever operation carries it,
//! and a request that breaks one is refused `InvalidParameterValueException`
//! (HTTP 400) with no side effect — in particular without advancing the rule's
//! `ruleRevision`. The three members whose constraints the model cannot express
//! are:
//!
//! * `criteria` — a condition needs a `comparison` from its criterion's own
//!   operator family, at least one value, and (for `resourceTag`) a tag key, and
//!   "You can specify up to 20 conditions per filter criteria and 20 values per
//!   condition" (`smithy_query(query="doc",shape="Criteria")`);
//! * `recommendedActionTypes` — the action types the rule executes, so an empty
//!   list automates nothing;
//! * `schedule` — "The expression that defines when the schedule runs", a cron
//!   of six fields of which "Your cron expression must use specific values (not
//!   wildcards) for the minutes and hours fields"
//!   (`smithy_query(query="doc",shape="Schedule",member="scheduleExpression")`).
//!
//! Each is asserted over a TABLE spanning the class rather than over the one
//! request a report named: the operator cases are EXHAUSTIVE over every member
//! of `Criteria` crossed with every value of the model's `ComparisonOperator`,
//! the size cases walk each published bound and one past it, and the schedule
//! cases walk every form a cron field can take.
//!
//! Every case drives the service over the wire through the shared awsJson1_0
//! harness in [`crate::automation_rule_lifecycle`].

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::{
    COMPARISON_OPERATOR_VALUES, RECOMMENDED_ACTION_TYPE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator, member, minimal_rule, string_member,
};

// ---------------------------------------------------------------------------
// The shape of a criteria condition
// ---------------------------------------------------------------------------

/// What kind of value a criterion's conditions compare, which is what decides
/// the JSON type its `values` list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Text,
    Integer,
    Double,
}

/// Every member of `Criteria`: its wire name, the comparison-operator family it
/// accepts, and the kind of value its conditions compare.
///
/// Source: `smithy_query(query="shape",shape="Criteria")` — `region`,
/// `resourceArn`, `ebsVolumeType` and `restartNeeded` are
/// `StringCriteriaConditionList`s, `resourceTag` is a
/// `ResourceTagsCriteriaConditionList`, `ebsVolumeSizeInGib` and
/// `lookBackPeriodInDays` are `IntegerCriteriaConditionList`s and
/// `estimatedMonthlySavings` is a `DoubleCriteriaConditionList`.
const CRITERION_MEMBERS: &[(&str, &str, ValueKind)] = &[
    ("region", "String", ValueKind::Text),
    ("resourceArn", "String", ValueKind::Text),
    ("ebsVolumeType", "String", ValueKind::Text),
    ("restartNeeded", "String", ValueKind::Text),
    ("resourceTag", "String", ValueKind::Text),
    ("ebsVolumeSizeInGib", "Numeric", ValueKind::Integer),
    ("lookBackPeriodInDays", "Numeric", ValueKind::Integer),
    ("estimatedMonthlySavings", "Numeric", ValueKind::Double),
];

/// `count` values of `kind`, distinct and seeded by the condition's index so two
/// conditions of one criterion are not copies of each other.
fn condition_values(kind: ValueKind, index: usize, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|nth| match kind {
                ValueKind::Text => json!(format!("value-{index}-{nth}")),
                ValueKind::Integer => json!((index * 100 + nth + 1) as i64),
                ValueKind::Double => json!((index * 100 + nth) as f64 + 0.5),
            })
            .collect(),
    )
}

/// One condition of `member`, comparing `values` values with `comparison`.
fn condition(
    member_name: &str,
    comparison: &str,
    kind: ValueKind,
    index: usize,
    values: usize,
) -> Value {
    let mut condition = json!({
        "comparison": comparison,
        "values": condition_values(kind, index, values),
    });
    // A `resourceTag` condition names the tag KEY it is about.
    if member_name == "resourceTag" {
        condition["key"] = json!(format!("tag-key-{index}"));
    }
    condition
}

/// A `criteria` object whose `member_name` criterion holds `conditions`
/// conditions, each comparing `values` values with `comparison`.
fn criteria(
    member_name: &str,
    comparison: &str,
    kind: ValueKind,
    conditions: usize,
    values: usize,
) -> Value {
    let list: Vec<Value> = (0..conditions)
        .map(|index| condition(member_name, comparison, kind, index, values))
        .collect();
    let mut criteria = json!({});
    criteria[member_name] = Value::Array(list);
    criteria
}

/// The preview request that EVALUATES `criteria` — the path a report found
/// refusing values the write paths accepted, so each case is asserted against it
/// as well.
fn preview_request(criteria: &Value) -> Value {
    json!({
        "ruleType": "AccountRule",
        "recommendedActionTypes": RECOMMENDED_ACTION_TYPE_VALUES,
        "criteria": criteria,
    })
}

// ---------------------------------------------------------------------------
// The write paths
// ---------------------------------------------------------------------------

/// The rule every update case is applied to: its ARN, the revision it stands at
/// and the whole `GetAutomationRule` projection a refused update must leave
/// untouched (values, revision and `lastUpdatedTimestamp` alike).
struct Subject {
    arn: String,
    revision: i64,
    snapshot: Value,
}

impl Subject {
    async fn create(router: &MultiServiceRouter, name: &str) -> Self {
        let created = call_ok(router, "CreateAutomationRule", &minimal_rule(name), name).await;
        let arn = string_member(&created, "ruleArn");
        let revision = member(&created, "ruleRevision")
            .as_i64()
            .expect("the create reported a revision");
        let snapshot = call_ok(router, "GetAutomationRule", &json!({"ruleArn": &arn}), name).await;
        Self {
            arn,
            revision,
            snapshot,
        }
    }

    /// An update carrying `member_name: value` is refused, and the rule is left
    /// exactly as it was.
    async fn expect_refused(
        &self,
        router: &MultiServiceRouter,
        member_name: &str,
        value: &Value,
        case: &str,
    ) {
        let mut request = json!({"ruleArn": &self.arn, "ruleRevision": self.revision});
        request[member_name] = value.clone();
        assert_refused(
            call(router, "UpdateAutomationRule", &request).await,
            400,
            "InvalidParameterValueException",
            case,
        );
        let after = call_ok(
            router,
            "GetAutomationRule",
            &json!({"ruleArn": &self.arn}),
            case,
        )
        .await;
        assert_eq!(
            after, self.snapshot,
            "case {case:?}: a refused update must leave the rule, its revision and its update \
             time alone, got {after}"
        );
    }

    /// An update carrying `member_name: value` is applied: the revision moves by
    /// one and a later read reports the value.
    async fn expect_applied(
        &mut self,
        router: &MultiServiceRouter,
        member_name: &str,
        value: &Value,
        case: &str,
    ) {
        let mut request = json!({"ruleArn": &self.arn, "ruleRevision": self.revision});
        request[member_name] = value.clone();
        let updated = call_ok(router, "UpdateAutomationRule", &request, case).await;
        self.revision += 1;
        assert_eq!(
            member(&updated, "ruleRevision").as_i64(),
            Some(self.revision),
            "case {case:?}: an applied update bumps the revision by one, got {updated}"
        );
        let fetched = call_ok(
            router,
            "GetAutomationRule",
            &json!({"ruleArn": &self.arn}),
            case,
        )
        .await;
        assert_eq!(
            member(&fetched, member_name),
            value,
            "case {case:?}: the stored {member_name} must be what the update carried, got {fetched}"
        );
        self.snapshot = fetched;
    }
}

/// A create carrying `member_name: value` is refused and stores no rule.
async fn expect_create_refused(
    router: &MultiServiceRouter,
    member_name: &str,
    value: &Value,
    case: &str,
) {
    let name = format!("refused-{case}");
    let mut request = minimal_rule(&name);
    request[member_name] = value.clone();
    assert_refused(
        call(router, "CreateAutomationRule", &request).await,
        400,
        "InvalidParameterValueException",
        case,
    );
    let listed = call_ok(router, "ListAutomationRules", &json!({}), case).await;
    let stored: Vec<String> = member(&listed, "automationRules")
        .as_array()
        .into_iter()
        .flatten()
        .map(|rule| string_member(rule, "name"))
        .collect();
    assert!(
        !stored.contains(&name),
        "case {case:?}: a refused create must store nothing, got {listed}"
    );
}

/// A create carrying `member_name: value` is accepted and the value round-trips
/// through `GetAutomationRule`.
async fn expect_create_applied(
    router: &MultiServiceRouter,
    member_name: &str,
    value: &Value,
    case: &str,
) {
    let name = format!("created-{case}");
    let mut request = minimal_rule(&name);
    request[member_name] = value.clone();
    let created = call_ok(router, "CreateAutomationRule", &request, case).await;
    assert_eq!(
        member(&created, member_name),
        value,
        "case {case:?}: the create must echo the stored {member_name}, got {created}"
    );
    let fetched = call_ok(
        router,
        "GetAutomationRule",
        &json!({"ruleArn": string_member(&created, "ruleArn")}),
        case,
    )
    .await;
    assert_eq!(
        member(&fetched, member_name),
        value,
        "case {case:?}: the stored {member_name} must read back as sent, got {fetched}"
    );
}

// ---------------------------------------------------------------------------
// criteria: the operator family
// ---------------------------------------------------------------------------

/// A criteria condition is accepted only with a comparison operator from its own
/// criterion's family, and every write path and the preview agree about it.
///
/// EXHAUSTIVE: every member of `Criteria` crossed with every value of the
/// model's `ComparisonOperator` — `IfExists` variants included, since the suffix
/// decides what happens for a missing attribute and not which family the
/// operator belongs to.
#[tokio::test]
async fn a_criteria_condition_is_accepted_only_with_its_own_operator_family() {
    let router = emulator();
    let mut subject = Subject::create(&router, "operator-family-subject").await;

    for (member_name, family, kind) in CRITERION_MEMBERS {
        for operator in COMPARISON_OPERATOR_VALUES {
            let base = operator.strip_suffix("IfExists").unwrap_or(operator);
            let belongs = base.starts_with(family);
            let value = criteria(member_name, operator, *kind, 1, 1);
            let case = format!("{member_name}-{operator}");

            let preview = call(&router, "ListAutomationRulePreview", &preview_request(&value)).await;
            if belongs {
                subject
                    .expect_applied(&router, "criteria", &value, &case)
                    .await;
                assert_eq!(
                    preview.0, 200,
                    "case {case:?}: the preview evaluates an operator of this criterion's own \
                     family, got {} and {}",
                    preview.0, preview.1
                );
            } else {
                expect_create_refused(&router, "criteria", &value, &case).await;
                subject
                    .expect_refused(&router, "criteria", &value, &case)
                    .await;
                assert_refused(preview, 400, "InvalidParameterValueException", &case);
            }
        }
    }
}

/// A criteria condition that names no operator, no value, or (for `resourceTag`)
/// no tag key selects nothing, so every path refuses it.
#[tokio::test]
async fn a_criteria_condition_missing_what_it_compares_is_refused() {
    let router = emulator();
    let mut subject = Subject::create(&router, "incomplete-condition-subject").await;

    for (member_name, _, kind) in CRITERION_MEMBERS {
        // (case suffix, the one condition, whether the service accepts it)
        let mut cases: Vec<(String, Value, bool)> = vec![
            (
                format!("{member_name}-no-comparison"),
                json!({"values": condition_values(*kind, 0, 1)}),
                false,
            ),
            (
                format!("{member_name}-no-values"),
                json!({"comparison": "StringEqualsIfExists"}),
                false,
            ),
            (
                format!("{member_name}-empty-values"),
                condition(member_name, "StringEquals", *kind, 0, 0),
                false,
            ),
            (
                format!("{member_name}-complete"),
                condition(member_name, "StringEquals", *kind, 0, 1),
                true,
            ),
        ];
        // The two members whose conditions compare numbers take the numeric
        // family, so their complete case names a numeric operator.
        if *kind != ValueKind::Text {
            for (_, condition, accepted) in &mut cases {
                if *accepted {
                    condition["comparison"] = json!("NumericEquals");
                }
            }
            cases[1].1["comparison"] = json!("NumericEqualsIfExists");
        }
        // A `resourceTag` condition is also about one tag key, so the key the
        // helper puts there is what makes the complete case complete — and
        // dropping it is a case of its own.
        if *member_name == "resourceTag" {
            let mut keyless = condition(member_name, "StringEquals", *kind, 0, 1);
            keyless
                .as_object_mut()
                .expect("a condition is an object")
                .remove("key");
            cases.push((format!("{member_name}-no-key"), keyless, false));
        }

        for (case, one_condition, accepted) in &cases {
            let mut value = json!({});
            value[*member_name] = json!([one_condition]);
            let preview =
                call(&router, "ListAutomationRulePreview", &preview_request(&value)).await;
            if *accepted {
                expect_create_applied(&router, "criteria", &value, case).await;
                subject.expect_applied(&router, "criteria", &value, case).await;
                assert_eq!(
                    preview.0, 200,
                    "case {case:?}: a complete condition is evaluated, got {} and {}",
                    preview.0, preview.1
                );
            } else {
                expect_create_refused(&router, "criteria", &value, case).await;
                subject.expect_refused(&router, "criteria", &value, case).await;
                assert_refused(preview, 400, "InvalidParameterValueException", case);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// criteria: the documented size caps
// ---------------------------------------------------------------------------

/// "You can specify up to 20 conditions per filter criteria and 20 values per
/// condition": each published bound is accepted and one past it is refused, for
/// every member of `Criteria`.
#[tokio::test]
async fn criteria_hold_at_most_twenty_conditions_each_over_at_most_twenty_values() {
    let router = emulator();
    let mut subject = Subject::create(&router, "size-cap-subject").await;

    // (conditions in the criterion, values in each condition, accepted)
    // 20 is the documented cap on both counts, so each is walked at 1, just
    // inside the bound, AT the bound and past it. A criterion with no
    // conditions constrains nothing, which is what leaving it out does.
    let sizes: &[(usize, usize, bool)] = &[
        (0, 1, true),
        (1, 1, true),
        (2, 1, true),
        (19, 1, true),
        (20, 1, true),
        (21, 1, false),
        (40, 1, false),
        (1, 2, true),
        (1, 19, true),
        (1, 20, true),
        (1, 21, false),
        (1, 40, false),
        (20, 20, true),
        (21, 21, false),
    ];

    for (member_name, family, kind) in CRITERION_MEMBERS {
        let operator = if *family == "Numeric" {
            "NumericEquals"
        } else {
            "StringEquals"
        };
        for (conditions, values, accepted) in sizes {
            let value = criteria(member_name, operator, *kind, *conditions, *values);
            let case = format!("{member_name}-{conditions}x{values}");
            let preview =
                call(&router, "ListAutomationRulePreview", &preview_request(&value)).await;
            if *accepted {
                expect_create_applied(&router, "criteria", &value, &case).await;
                subject
                    .expect_applied(&router, "criteria", &value, &case)
                    .await;
                assert_eq!(
                    preview.0, 200,
                    "case {case:?}: a criteria inside the documented caps is evaluated, got {} \
                     and {}",
                    preview.0, preview.1
                );
            } else {
                expect_create_refused(&router, "criteria", &value, &case).await;
                subject
                    .expect_refused(&router, "criteria", &value, &case)
                    .await;
                assert_refused(preview, 400, "InvalidParameterValueException", &case);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// schedule
// ---------------------------------------------------------------------------

/// A schedule is accepted only when it carries a six-field cron expression whose
/// minutes and hours fields each name ONE value in range.
///
/// The table walks every form a cron field can take — a specific value at each
/// end of its range, both wildcards, a list, a range, a step, a value past the
/// field's bound — and every field count around the documented six. The four
/// fields the doc does not constrain (`day_of_month`, `month`, `day_of_week`,
/// `year`) are left to the expression's own vocabulary, which the accepted rows
/// below exercise.
#[tokio::test]
async fn a_schedule_is_accepted_only_with_specific_minutes_and_hours() {
    let router = emulator();
    let mut subject = Subject::create(&router, "schedule-subject").await;

    // (case, the schedule, accepted)
    let cases: Vec<(&str, Value, bool)> = vec![
        ("no-expression", json!({"executionWindowInMinutes": 60}), false),
        ("empty-expression", json!({"scheduleExpression": ""}), false),
        ("blank-expression", json!({"scheduleExpression": "   "}), false),
        (
            "specific-at-the-low-end-of-both-fields",
            json!({"scheduleExpression": "0 0 * * ? *"}),
            true,
        ),
        (
            "specific-at-the-high-end-of-both-fields",
            json!({"scheduleExpression": "59 23 * * ? *"}),
            true,
        ),
        (
            "the-documented-half-past-noon",
            json!({"scheduleExpression": "30 12 * * ? *", "executionWindowInMinutes": 1440}),
            true,
        ),
        (
            "zero-padded-specific-values",
            json!({"scheduleExpression": "07 09 * * ? *"}),
            true,
        ),
        (
            "specific-day-month-and-year",
            json!({"scheduleExpression": "15 6 1 1 ? 2027"}),
            true,
        ),
        (
            "a-named-day-of-week",
            json!({"scheduleExpression": "45 18 ? * MON *"}),
            true,
        ),
        (
            "minutes-past-their-bound",
            json!({"scheduleExpression": "60 12 * * ? *"}),
            false,
        ),
        (
            "hours-past-their-bound",
            json!({"scheduleExpression": "30 24 * * ? *"}),
            false,
        ),
        (
            "both-fields-wildcarded",
            json!({"scheduleExpression": "* * * * ? *"}),
            false,
        ),
        (
            "wildcard-minutes",
            json!({"scheduleExpression": "* 12 * * ? *"}),
            false,
        ),
        (
            "wildcard-hours",
            json!({"scheduleExpression": "30 * * * ? *"}),
            false,
        ),
        (
            "any-minutes",
            json!({"scheduleExpression": "? 12 * * ? *"}),
            false,
        ),
        (
            "any-hours",
            json!({"scheduleExpression": "30 ? * * ? *"}),
            false,
        ),
        (
            "a-list-of-minutes",
            json!({"scheduleExpression": "0,30 12 * * ? *"}),
            false,
        ),
        (
            "a-range-of-hours",
            json!({"scheduleExpression": "30 0-6 * * ? *"}),
            false,
        ),
        (
            "a-step-over-minutes",
            json!({"scheduleExpression": "*/15 12 * * ? *"}),
            false,
        ),
        (
            "a-step-over-hours",
            json!({"scheduleExpression": "30 12/2 * * ? *"}),
            false,
        ),
        (
            "a-signed-minute",
            json!({"scheduleExpression": "+5 12 * * ? *"}),
            false,
        ),
        (
            "five-fields",
            json!({"scheduleExpression": "30 12 * * ?"}),
            false,
        ),
        (
            "seven-fields",
            json!({"scheduleExpression": "30 12 * * ? * *"}),
            false,
        ),
        (
            "one-field",
            json!({"scheduleExpression": "30"}),
            false,
        ),
    ];

    for (case, schedule, accepted) in &cases {
        if *accepted {
            expect_create_applied(&router, "schedule", schedule, case).await;
            subject
                .expect_applied(&router, "schedule", schedule, case)
                .await;
        } else {
            expect_create_refused(&router, "schedule", schedule, case).await;
            subject
                .expect_refused(&router, "schedule", schedule, case)
                .await;
        }
    }
}

// ---------------------------------------------------------------------------
// recommendedActionTypes
// ---------------------------------------------------------------------------

/// A rule must name at least one recommended action type: an empty list is
/// refused, and every non-empty selection of the modelled value set is stored.
#[tokio::test]
async fn a_rule_must_name_at_least_one_recommended_action_type() {
    let router = emulator();
    let mut subject = Subject::create(&router, "action-type-subject").await;

    expect_create_refused(&router, "recommendedActionTypes", &json!([]), "empty").await;
    subject
        .expect_refused(&router, "recommendedActionTypes", &json!([]), "empty")
        .await;

    // Every singleton of the model's `RecommendedActionType`, plus the whole
    // set, so the accepted side spans the member's value space and not only the
    // one type a report named.
    let mut selections: Vec<(String, Value)> = RECOMMENDED_ACTION_TYPE_VALUES
        .iter()
        .map(|action_type| (format!("only-{action_type}"), json!([action_type])))
        .collect();
    selections.push(("every-action-type".to_string(), json!(RECOMMENDED_ACTION_TYPE_VALUES)));

    for (case, value) in &selections {
        expect_create_applied(&router, "recommendedActionTypes", value, case).await;
        subject
            .expect_applied(&router, "recommendedActionTypes", value, case)
            .await;
    }
}
