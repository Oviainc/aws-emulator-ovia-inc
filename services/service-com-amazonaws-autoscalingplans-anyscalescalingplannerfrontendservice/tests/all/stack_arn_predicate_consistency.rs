// SPDX-License-Identifier: CC0-1.0

//! One predicate decides when an `ApplicationSource` names a CloudFormation stack, and
//! every path that asks gets the same answer.
//!
//! `ApplicationSource` is documented as "A CloudFormation stack or a set of tags" and
//! `ApplicationSource$CloudFormationStackARN` as "The Amazon Resource Name (ARN) of a
//! AWS CloudFormation stack"
//! (`smithy_query(query="doc",shape="ApplicationSource",member="CloudFormationStackARN")`),
//! so a source names a stack exactly when that member carries a value: a member that is
//! absent, or present but empty, names no stack at all.
//!
//! Three decisions rest on that one predicate — the stack-ARN format check, the
//! either/or check between the two members, and the identity key behind "You can create
//! one scaling plan per application source" — so this suite drives the SAME table of
//! `CloudFormationStackARN` spellings through an operation for each of them:
//! `CreateScalingPlan` (format and either/or), `DescribeScalingPlans`'
//! `ApplicationSources` filter (format, either/or and identity) and `UpdateScalingPlan`
//! (identity). The rows differ along what the predicate generalises over: the member
//! absent, present and empty, present and malformed, present and well-formed, and each
//! of those beside a tag-filter list.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state as plan_state;

use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// A well-formed CloudFormation stack ARN that no other suite's fixture uses.
const WELL_FORMED_ARN: &str = "arn:aws:cloudformation:eu-west-1:000000000000:stack/fold/0a1b";

/// What the one rule says about a row's source.
enum Expect {
    /// The source identifies an application, so a write over it is accepted and a read
    /// filtered by it finds exactly that plan.
    Accepted,
    /// The source is refused, with this message.
    Refused(String),
}

/// One spelling of `CloudFormationStackARN`, with or without a tag-filter list.
struct Row {
    label: &'static str,
    source: Value,
    expect: Expect,
}

/// The refusal for a value that names a stack but is not a stack ARN.
fn malformed(arn: &str) -> Expect {
    Expect::Refused(format!("Invalid CloudFormation stack ARN: {arn}"))
}

/// The refusal for a source that names both alternatives.
fn both() -> Expect {
    Expect::Refused(plan_state::AMBIGUOUS_APPLICATION_SOURCE.to_string())
}

/// The refusal for a source that names neither alternative.
fn neither() -> Expect {
    Expect::Refused(plan_state::EMPTY_APPLICATION_SOURCE.to_string())
}

/// One tag-filter list, used by the rows that pair a stack-ARN spelling with tags.
fn tag_filters() -> Value {
    json!([{ "Key": "env", "Values": ["prod"] }])
}

/// The table every test in this suite spans.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a well-formed stack ARN, no tag filters",
            source: json!({ "CloudFormationStackARN": WELL_FORMED_ARN }),
            expect: Expect::Accepted,
        },
        Row {
            label: "the member absent, tag filters present",
            source: json!({ "TagFilters": tag_filters() }),
            expect: Expect::Accepted,
        },
        Row {
            label: "the member present but empty, tag filters present",
            source: json!({ "CloudFormationStackARN": "", "TagFilters": tag_filters() }),
            expect: Expect::Accepted,
        },
        Row {
            label: "the member present but empty, no tag filters",
            source: json!({ "CloudFormationStackARN": "" }),
            expect: neither(),
        },
        Row {
            label: "the member present but empty, empty tag-filter list",
            source: json!({ "CloudFormationStackARN": "", "TagFilters": [] }),
            expect: neither(),
        },
        Row {
            label: "neither member",
            source: json!({}),
            expect: neither(),
        },
        Row {
            label: "a stack name rather than an ARN",
            source: json!({ "CloudFormationStackARN": "fold" }),
            expect: malformed("fold"),
        },
        Row {
            label: "an ARN of another service",
            source: json!({
                "CloudFormationStackARN": "arn:aws:ec2:eu-west-1:000000000000:instance/i-0a1b"
            }),
            expect: malformed("arn:aws:ec2:eu-west-1:000000000000:instance/i-0a1b"),
        },
        Row {
            label: "a CloudFormation ARN of another resource type",
            source: json!({
                "CloudFormationStackARN":
                    "arn:aws:cloudformation:eu-west-1:000000000000:changeSet/fold/0a1b"
            }),
            expect: malformed("arn:aws:cloudformation:eu-west-1:000000000000:changeSet/fold/0a1b"),
        },
        Row {
            label: "a malformed ARN beside tag filters: the ARN is still checked",
            source: json!({ "CloudFormationStackARN": "fold", "TagFilters": tag_filters() }),
            expect: malformed("fold"),
        },
        Row {
            label: "a well-formed ARN beside tag filters",
            source: json!({
                "CloudFormationStackARN": WELL_FORMED_ARN,
                "TagFilters": tag_filters()
            }),
            expect: both(),
        },
    ]
}

/// The names `DescribeScalingPlans` reports for one `ApplicationSources` filter.
async fn plans_covering(emulator: &Emulator, source: &Value) -> Vec<String> {
    let described = emulator
        .ok(
            "DescribeScalingPlans",
            json!({ "ApplicationSources": [source] }),
        )
        .await;
    described["ScalingPlans"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|plan| {
            plan["ScalingPlanName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_write_and_the_read_path_classify_every_stack_arn_spelling_alike() {
    for row in rows() {
        let label = row.label;
        let emulator = Emulator::new();

        let (create_status, create_body) = emulator
            .call(
                "CreateScalingPlan",
                &json!({
                    "ScalingPlanName": "plan-a",
                    "ApplicationSource": row.source,
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        // The read path asks the one predicate the same question about the same source.
        let (describe_status, describe_body) = emulator
            .call(
                "DescribeScalingPlans",
                &json!({ "ApplicationSources": [row.source] }),
            )
            .await;

        match &row.expect {
            Expect::Refused(message) => {
                for (operation, status, body) in [
                    ("CreateScalingPlan", create_status, &create_body),
                    ("DescribeScalingPlans", describe_status, &describe_body),
                ] {
                    assert_eq!(
                        (status, error_code(body), error_message(body)),
                        (400, "ValidationException", message.as_str()),
                        "case {label}/{operation}: expected a 400 ValidationException \
                         carrying the one refusal, got {status} with {body:?}"
                    );
                }
            }
            Expect::Accepted => {
                assert_eq!(
                    create_status, 200,
                    "case {label}: the source identifies an application, so the create \
                     must be accepted, got {create_status} with {create_body:?}"
                );
                assert_eq!(
                    describe_status, 200,
                    "case {label}: the same source must be an accepted filter, got \
                     {describe_status} with {describe_body:?}"
                );
                let covering = plans_covering(&emulator, &row.source).await;
                assert_eq!(
                    covering,
                    vec!["plan-a".to_string()],
                    "case {label}: the filter must find the plan created over this \
                     source, got {covering:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_empty_stack_arn_names_no_stack_for_the_identity_rule_either() {
    // The identity key must read the SAME predicate as the either/or check: a source
    // whose `CloudFormationStackARN` is empty is the tag-filter source it also names,
    // so it collides with the plan that already claims those tags — on both write paths
    // and on the read filter.
    let claimed = json!({ "TagFilters": tag_filters() });
    let respelled = json!({ "CloudFormationStackARN": "", "TagFilters": tag_filters() });
    let expected = "The application source is already covered by the scaling plan 'plan-a'";

    let emulator = Emulator::new();
    for (name, source) in [("plan-a", &claimed), ("plan-b", &stack_source(42))] {
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": name,
                    "ApplicationSource": source,
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
    }
    emulator.make_all_due();

    for (operation, request) in [
        (
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "plan-c",
                "ApplicationSource": respelled,
                "ScalingInstructions": simple_instructions(),
            }),
        ),
        (
            "UpdateScalingPlan",
            json!({
                "ScalingPlanName": "plan-b",
                "ScalingPlanVersion": 1,
                "ApplicationSource": respelled,
            }),
        ),
    ] {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (400, "ValidationException", expected),
            "case empty ARN beside the claimed tags/{operation}: the identity key must \
             ignore an empty stack ARN, got {status} with {body:?}"
        );
    }

    let covering = plans_covering(&emulator, &respelled).await;
    assert_eq!(
        covering,
        vec!["plan-a".to_string()],
        "case empty ARN beside the claimed tags/DescribeScalingPlans: the filter must \
         resolve to the same application source, got {covering:?}"
    );
}
