// SPDX-License-Identifier: CC0-1.0

//! The `PolicyName` a scaling plan resource reports is a function of the IDENTITIES
//! behind the policy, not of where anything sits in a list.
//!
//! The rule pinned here: a service-generated name identifies the thing it was minted
//! for, so the name `ScalingPlanResource$ScalingPolicies` reports is determined by the
//! plan that owns the policy, the `(ServiceNamespace, ResourceId, ScalableDimension)`
//! resource its instruction applies to ("Each scaling instruction applies to one
//! resource", `smithy_query(query="shape",shape="ScalingInstruction")`) and the metric
//! the target tracking configuration it came from tracks ("Each of these structures
//! must specify a unique scaling metric", same shape's
//! `TargetTrackingConfigurations`) — and by nothing else. Two consequences are
//! observable through the wire path and both are asserted below:
//!
//! * `UpdateScalingPlan` stores the instruction list the request carries, in the
//!   request's order, so dropping, inserting or reordering an instruction moves every
//!   position in that list — and must leave the name of a policy whose resource and
//!   metric it did not touch alone;
//! * a name that survives such a rewrite still names the policy it named before, so a
//!   client that tracks a policy by name cannot be handed another resource's policy.
//!
//! The bound comes along for free: `ScalingPolicy$PolicyName` is 1-256 printable
//! characters (`smithy_query(query="shape",shape="ScalingPolicy")`) while a `ResourceId`
//! runs to 1600 (`smithy_query(query="shape",shape="ResourceIdMaxLen1600")`) and a
//! customized metric's namespace, name and dimensions are plain strings, so the rows
//! here include identities at and beyond those lengths.
//!
//! The harness (router, dispatch, lifecycle clock) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::scaling_plan_lifecycle::{resource_identity, stack_source, Emulator, ResourceIdentity};

/// The plan every case writes to.
const PLAN: &str = "my-plan";

/// The documented bounds of `ScalingPolicy$PolicyName`
/// (`smithy_query(query="shape",shape="ScalingPolicy")`: len 1..256, pattern
/// `^\p{Print}+$`).
const MIN_POLICY_NAME: usize = 1;
const MAX_POLICY_NAME: usize = 256;

/// The documented maximum length of a `ScalingInstruction$ResourceId`
/// (`smithy_query(query="shape",shape="ResourceIdMaxLen1600")`: len 1..1600).
const MAX_RESOURCE_ID: usize = 1600;

/// The documented maximum length of a `ScalingPlanName`
/// (`smithy_query(query="shape",shape="ScalingPlanName")`: len 1..128).
const MAX_PLAN_NAME: usize = 128;

/// One target tracking configuration over a predefined metric.
///
/// The metric types are `ScalingMetricType` values
/// (`smithy_query(query="shape",shape="ScalingMetricType")`).
fn predefined(metric: &str, target: f64) -> Value {
    json!({
        "PredefinedScalingMetricSpecification": { "PredefinedScalingMetricType": metric },
        "TargetValue": target
    })
}

/// One target tracking configuration over a customized metric.
///
/// `Statistic` is a modelled `MetricStatistic` value
/// (`smithy_query(query="shape",shape="MetricStatistic")`); the namespace, the metric
/// name and the dimensions are plain strings under no modelled pattern.
fn customized(namespace: &str, metric_name: &str, dimensions: &[(&str, &str)], target: f64) -> Value {
    let dimensions: Vec<Value> = dimensions
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    json!({
        "CustomizedScalingMetricSpecification": {
            "Namespace": namespace,
            "MetricName": metric_name,
            "Statistic": "Average",
            "Dimensions": dimensions
        },
        "TargetValue": target
    })
}

/// One `ScalingInstruction` over the configurations given.
///
/// The namespace and the dimension are drawn from the model's own value lists
/// (`smithy_query(query="shape",shape="ServiceNamespace,ScalableDimension")`), and a
/// resource id identifies a resource of the kind its dimension scales, as that member's
/// own worked examples show.
fn instruction(
    namespace: &str,
    resource_id: &str,
    dimension: &str,
    min: i64,
    max: i64,
    configurations: Vec<Value>,
) -> Value {
    json!({
        "ServiceNamespace": namespace,
        "ResourceId": resource_id,
        "ScalableDimension": dimension,
        "MinCapacity": min,
        "MaxCapacity": max,
        "TargetTrackingConfigurations": configurations
    })
}

/// An Auto Scaling group instruction, over the configurations given.
fn asg(name: &str, configurations: Vec<Value>) -> Value {
    instruction(
        "autoscaling",
        &format!("autoScalingGroup/{name}"),
        "autoscaling:autoScalingGroup:DesiredCapacity",
        1,
        10,
        configurations,
    )
}

/// An ECS service instruction tracking average memory.
fn ecs_service(name: &str) -> Value {
    instruction(
        "ecs",
        &format!("service/default/{name}"),
        "ecs:service:DesiredCount",
        1,
        20,
        vec![predefined("ECSServiceAverageMemoryUtilization", 70.0)],
    )
}

/// A DynamoDB table instruction over `dimension`, tracking `metric`.
fn table(dimension: &str, metric: &str) -> Value {
    instruction(
        "dynamodb",
        "table/my-table",
        dimension,
        1,
        100,
        vec![predefined(metric, 50.0)],
    )
}

/// An Auto Scaling group whose resource id is `length` characters long: the resource
/// type the dimension scales, padded out to the length a row asks for.
fn asg_with_id_length(length: usize, configurations: Vec<Value>) -> Value {
    let prefix = "autoScalingGroup/";
    let padding = length.saturating_sub(prefix.len());
    instruction(
        "autoscaling",
        &format!("{prefix}{}", "a".repeat(padding)),
        "autoscaling:autoScalingGroup:DesiredCapacity",
        1,
        10,
        configurations,
    )
}

/// What identifies one policy of one plan: the resource its instruction applies to and
/// the metric the configuration it was minted from tracks.
type PolicyKey = (ResourceIdentity, String);

/// The identity of the metric one target tracking configuration tracks, read off the
/// configuration the policy echoes.
fn metric_label(configuration: &Value) -> String {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    if let Some(predefined) = configuration.get("PredefinedScalingMetricSpecification") {
        return format!(
            "predefined {}",
            text(&predefined["PredefinedScalingMetricType"])
        );
    }
    let customized = &configuration["CustomizedScalingMetricSpecification"];
    // The dimensions are a SET, so a configuration that lists them in another order
    // tracks the same metric.
    let mut dimensions: Vec<String> = customized["Dimensions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|dimension| {
            format!(
                "{}={}",
                text(&dimension["Name"]),
                text(&dimension["Value"])
            )
        })
        .collect();
    dimensions.sort();
    format!(
        "customized {}/{}/{}[{}]",
        text(&customized["Namespace"]),
        text(&customized["MetricName"]),
        text(&customized["Statistic"]),
        dimensions.join(",")
    )
}

/// Every [`PolicyKey`] a list of scaling instructions asks for.
fn requested_keys(instructions: &[Value]) -> BTreeSet<PolicyKey> {
    instructions
        .iter()
        .flat_map(|instruction| {
            let identity = resource_identity(instruction);
            instruction["TargetTrackingConfigurations"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |configuration| (identity.clone(), metric_label(&configuration)))
                .collect::<Vec<PolicyKey>>()
        })
        .collect()
}

/// Create `plan` from `instructions` and let the clock apply it, so the plan is `Active`
/// and its policies have reached its resources.
async fn applied_plan(emulator: &Emulator, plan: &str, instructions: Vec<Value>) {
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": plan,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": instructions,
            }),
        )
        .await;
    emulator.make_all_due();
}

/// Rewrite `plan`'s instruction list and let the clock apply the update.
async fn update_plan(emulator: &Emulator, plan: &str, instructions: Vec<Value>) {
    emulator
        .ok(
            "UpdateScalingPlan",
            json!({
                "ScalingPlanName": plan,
                "ScalingPlanVersion": 1,
                "ScalingInstructions": instructions,
            }),
        )
        .await;
    emulator.make_all_due();
}

/// The `PolicyName` of every policy `plan` reports, keyed by what identifies the policy,
/// with the modelled name bound and name uniqueness checked on the way through.
async fn policy_names(emulator: &Emulator, case: &str, plan: &str) -> BTreeMap<PolicyKey, String> {
    let body = emulator
        .ok(
            "DescribeScalingPlanResources",
            json!({ "ScalingPlanName": plan, "ScalingPlanVersion": 1 }),
        )
        .await;
    let resources = body["ScalingPlanResources"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no ScalingPlanResources in {body}"));

    let mut names: BTreeMap<PolicyKey, String> = BTreeMap::new();
    for resource in &resources {
        let identity = resource_identity(resource);
        for policy in resource["ScalingPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let name = policy["PolicyName"].as_str().unwrap_or_default().to_string();
            let length = name.chars().count();
            assert!(
                (MIN_POLICY_NAME..=MAX_POLICY_NAME).contains(&length)
                    && !name.chars().any(char::is_control),
                "case {case:?}: PolicyName must be {MIN_POLICY_NAME}-{MAX_POLICY_NAME} \
                 printable characters, got {length} in {name:?}"
            );
            let key = (
                identity.clone(),
                metric_label(&policy["TargetTrackingConfiguration"]),
            );
            let previous = names.insert(key.clone(), name.clone());
            assert!(
                previous.is_none(),
                "case {case:?}: {key:?} is reported twice, as {previous:?} and {name:?}"
            );
        }
    }

    let distinct: BTreeSet<&String> = names.values().collect();
    assert_eq!(
        distinct.len(),
        names.len(),
        "case {case:?}: every policy of a plan needs its own name, got {names:?}"
    );
    names
}

/// One row of the rewrite table: the instructions a plan is created with and the list an
/// update replaces them with.
struct RewriteCase {
    label: &'static str,
    before: Vec<Value>,
    after: Vec<Value>,
}

/// The table this suite spans. Rows differ along everything the rule generalises over:
/// the COUNT of instructions (one dropped, one added, both), their ORDER (two swapped,
/// three rotated), the order of the CONFIGURATIONS inside one instruction, which
/// non-identifying members the update changes (the capacity bounds, a target value), the
/// FORM of the metric (predefined vs customized), how close two resources are to each
/// other (two dimensions of one DynamoDB table), and the LENGTH of the identities (a
/// resource id at its modelled 1600-character maximum, a customized metric spelled out
/// over hundreds of characters).
fn rewrite_cases() -> Vec<RewriteCase> {
    let cpu = predefined("ASGAverageCPUUtilization", 40.0);
    let network_in = predefined("ASGAverageNetworkIn", 1_048_576.0);
    let requests = predefined("ALBRequestCountPerTarget", 1_000.0);
    let backlog = customized("MyApp", "backlog-per-instance", &[("Fleet", "orders")], 12.5);

    vec![
        RewriteCase {
            label: "two resources swap places",
            before: vec![asg("asg-a", vec![cpu.clone()]), asg("asg-b", vec![cpu.clone()])],
            after: vec![asg("asg-b", vec![cpu.clone()]), asg("asg-a", vec![cpu.clone()])],
        },
        RewriteCase {
            label: "the first of two resources is dropped",
            before: vec![asg("asg-a", vec![cpu.clone()]), asg("asg-b", vec![cpu.clone()])],
            after: vec![asg("asg-b", vec![cpu.clone()])],
        },
        RewriteCase {
            label: "a resource is inserted before the ones in force",
            before: vec![asg("asg-a", vec![cpu.clone()]), asg("asg-b", vec![cpu.clone()])],
            after: vec![
                ecs_service("sample-webapp"),
                asg("asg-a", vec![cpu.clone()]),
                asg("asg-b", vec![cpu.clone()]),
            ],
        },
        RewriteCase {
            label: "three resources are rotated",
            before: vec![
                asg("asg-a", vec![cpu.clone()]),
                asg("asg-b", vec![network_in.clone()]),
                ecs_service("sample-webapp"),
            ],
            after: vec![
                ecs_service("sample-webapp"),
                asg("asg-a", vec![cpu.clone()]),
                asg("asg-b", vec![network_in.clone()]),
            ],
        },
        RewriteCase {
            label: "one resource is dropped and another added in its place",
            before: vec![asg("asg-a", vec![cpu.clone()]), asg("asg-b", vec![cpu.clone()])],
            after: vec![asg("asg-b", vec![cpu.clone()]), ecs_service("sample-webapp")],
        },
        RewriteCase {
            label: "the configurations of one resource are reordered",
            before: vec![asg(
                "asg-a",
                vec![cpu.clone(), network_in.clone(), backlog.clone()],
            )],
            after: vec![asg(
                "asg-a",
                vec![backlog.clone(), network_in.clone(), cpu.clone()],
            )],
        },
        RewriteCase {
            label: "a configuration is dropped from a resource that keeps the others",
            before: vec![asg(
                "asg-a",
                vec![cpu.clone(), network_in.clone(), requests.clone()],
            )],
            after: vec![asg("asg-a", vec![requests.clone(), cpu.clone()])],
        },
        RewriteCase {
            label: "the capacity bounds of a resource change while its metrics do not",
            before: vec![asg("asg-a", vec![cpu.clone()]), asg("asg-b", vec![cpu.clone()])],
            after: vec![
                asg("asg-b", vec![cpu.clone()]),
                instruction(
                    "autoscaling",
                    "autoScalingGroup/asg-a",
                    "autoscaling:autoScalingGroup:DesiredCapacity",
                    5,
                    50,
                    vec![predefined("ASGAverageCPUUtilization", 80.0)],
                ),
            ],
        },
        RewriteCase {
            label: "two dimensions of one table swap places",
            before: vec![
                table("dynamodb:table:ReadCapacityUnits", "DynamoDBReadCapacityUtilization"),
                table(
                    "dynamodb:table:WriteCapacityUnits",
                    "DynamoDBWriteCapacityUtilization",
                ),
            ],
            after: vec![
                table(
                    "dynamodb:table:WriteCapacityUnits",
                    "DynamoDBWriteCapacityUtilization",
                ),
                table("dynamodb:table:ReadCapacityUnits", "DynamoDBReadCapacityUtilization"),
            ],
        },
        RewriteCase {
            label: "a resource id at the 1600-character bound swaps places",
            before: vec![
                asg_with_id_length(MAX_RESOURCE_ID, vec![cpu.clone()]),
                asg("asg-b", vec![cpu.clone()]),
            ],
            after: vec![
                asg("asg-b", vec![cpu.clone()]),
                asg_with_id_length(MAX_RESOURCE_ID, vec![cpu.clone()]),
            ],
        },
        RewriteCase {
            label: "a customized metric spelled out over hundreds of characters is reordered",
            before: vec![asg(
                "asg-a",
                vec![
                    customized(
                        &"n".repeat(300),
                        &"m".repeat(300),
                        &[("d", &"v".repeat(300))],
                        7.5,
                    ),
                    cpu.clone(),
                ],
            )],
            after: vec![asg(
                "asg-a",
                vec![
                    cpu.clone(),
                    customized(
                        &"n".repeat(300),
                        &"m".repeat(300),
                        &[("d", &"v".repeat(300))],
                        7.5,
                    ),
                ],
            )],
        },
    ]
}

#[tokio::test]
async fn a_policy_keeps_its_name_when_an_update_rewrites_the_instruction_list() {
    for case in rewrite_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        applied_plan(&emulator, PLAN, case.before.clone()).await;

        let before = policy_names(&emulator, label, PLAN).await;
        let reported: BTreeSet<PolicyKey> = before.keys().cloned().collect();
        assert_eq!(
            reported,
            requested_keys(&case.before),
            "case {label:?}: the plan must report one policy per configuration of each \
             resource it covers, got {before:?}"
        );

        update_plan(&emulator, PLAN, case.after.clone()).await;
        let after = policy_names(&emulator, label, PLAN).await;
        let reported: BTreeSet<PolicyKey> = after.keys().cloned().collect();
        assert_eq!(
            reported,
            requested_keys(&case.after),
            "case {label:?}: after the update the plan must report the policies the \
             update asks for, got {after:?}"
        );

        // A resource and metric the update left in place keeps the name it had.
        for (key, expected) in &before {
            if let Some(actual) = after.get(key) {
                assert_eq!(
                    actual, expected,
                    "case {label:?}: the policy of {key:?} was not touched by the \
                     update, so its name must not change: expected {expected:?}, got \
                     {actual:?}"
                );
            }
        }

        // And a name that survives the rewrite still names the policy it named before,
        // so no client following a name is handed another resource's policy.
        let named_before: BTreeMap<&String, &PolicyKey> =
            before.iter().map(|(key, name)| (name, key)).collect();
        for (key, name) in &after {
            if let Some(previously) = named_before.get(name) {
                assert_eq!(
                    *previously, key,
                    "case {label:?}: {name:?} named {previously:?} before the update \
                     and {key:?} after it"
                );
            }
        }
    }
}

#[tokio::test]
async fn one_plan_over_one_resource_set_reports_one_name_per_policy_whatever_order_it_was_written_in()
{
    let cpu = predefined("ASGAverageCPUUtilization", 40.0);
    let backlog = customized("MyApp", "backlog-per-instance", &[("Fleet", "orders")], 12.5);
    let a = asg("asg-a", vec![cpu.clone(), backlog.clone()]);
    let b = asg("asg-b", vec![cpu.clone()]);
    let c = ecs_service("sample-webapp");

    // The same three instructions, written in every order a caller could write them in.
    let orders: Vec<(&str, Vec<Value>)> = vec![
        ("a, b, c", vec![a.clone(), b.clone(), c.clone()]),
        ("a, c, b", vec![a.clone(), c.clone(), b.clone()]),
        ("b, a, c", vec![b.clone(), a.clone(), c.clone()]),
        ("b, c, a", vec![b.clone(), c.clone(), a.clone()]),
        ("c, a, b", vec![c.clone(), a.clone(), b.clone()]),
        ("c, b, a", vec![c.clone(), b.clone(), a.clone()]),
    ];

    let mut first: Option<BTreeMap<PolicyKey, String>> = None;
    for (label, instructions) in orders {
        let emulator = Emulator::new();
        applied_plan(&emulator, PLAN, instructions).await;
        let names = policy_names(&emulator, label, PLAN).await;
        match &first {
            None => first = Some(names),
            Some(expected) => assert_eq!(
                &names, expected,
                "case {label:?}: the name of a policy is fixed by the plan, the resource \
                 and the metric, so the order the instructions were written in must not \
                 change it: expected {expected:?}, got {names:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_policy_name_stays_inside_its_bound_for_the_longest_identities_the_model_admits() {
    // The worst case the model admits at once: a plan name at its 128-character bound,
    // a resource id at its 1600-character bound, and a customized metric whose plain-text
    // parts are longer still.
    let plan = "p".repeat(MAX_PLAN_NAME);
    let emulator = Emulator::new();
    applied_plan(
        &emulator,
        &plan,
        vec![asg_with_id_length(
            MAX_RESOURCE_ID,
            vec![
                predefined("ASGAverageCPUUtilization", 40.0),
                customized(
                    &"n".repeat(1_000),
                    &"m".repeat(1_000),
                    &[("d", &"v".repeat(1_000))],
                    7.5,
                ),
            ],
        )],
    )
    .await;

    // `policy_names` asserts the 1-256 printable bound and name uniqueness on every row
    // it reads; what this case adds is that both identities are at their extreme.
    let names = policy_names(&emulator, "the longest identities", &plan).await;
    assert_eq!(
        names.len(),
        2,
        "case {:?}: the resource must report one policy per configuration, got {names:?}",
        "the longest identities"
    );
}
