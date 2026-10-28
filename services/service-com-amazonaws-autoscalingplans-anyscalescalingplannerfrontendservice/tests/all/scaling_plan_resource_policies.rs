// SPDX-License-Identifier: CC0-1.0

//! The target tracking scaling policies `DescribeScalingPlanResources` reports, spanned
//! over the WHOLE catalog of scalable resources a scaling plan can cover and over every
//! page of a paged read.
//!
//! The rule pinned here: an applied scalable resource of a scaling plan reports one
//! `TargetTrackingScaling` policy per target tracking configuration of its own scaling
//! instruction, echoing that configuration — "AWS Auto Scaling creates target tracking
//! scaling policies based on the scaling instructions" (`ScalingInstruction` doc) —
//! whatever kind of resource the instruction scales and whichever page of the read the
//! resource arrives on.
//!
//! [`crate::applied_policies_and_status_narratives`] spans the same rule along the NUMBER
//! and FORM of the configurations on one resource; this suite spans it along the SERVICE
//! NAMESPACE / SCALABLE DIMENSION of the resource that carries them (every value the
//! model lists) and along the PAGING of the read that reports them.
//!
//! The harness (router, dispatch, lifecycle clock) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::scaling_plan_lifecycle::{resource_identity, stack_source, Emulator};

/// The one `PolicyType` the model lists
/// (`smithy_query(query="shape",shape="PolicyType")`), mirrored in
/// `src/ops/types/shared_types.rs` as `POLICY_TYPE_VALUES`.
const TARGET_TRACKING_SCALING: &str = "TargetTrackingScaling";

/// The documented upper bound of `ScalingPolicy$PolicyName`
/// (`smithy_query(query="shape",shape="ScalingPolicy")`: len 1..256, pattern
/// `^\p{Print}+$`).
const MAX_POLICY_NAME: usize = 256;

/// One target tracking configuration tracking a predefined metric.
fn predefined(metric: &str, target: f64) -> Value {
    json!({
        "PredefinedScalingMetricSpecification": { "PredefinedScalingMetricType": metric },
        "TargetValue": target
    })
}

/// One row of the catalog table: the scalable resource an instruction names, and the
/// target tracking configurations that instruction asks for.
///
/// `pub(crate)` with [`resource_cases`], so a suite that needs a plan covering the whole
/// catalog of scalable resources takes this one rather than writing a second copy of it.
pub(crate) struct ResourceCase {
    label: &'static str,
    pub(crate) namespace: &'static str,
    pub(crate) resource_id: &'static str,
    pub(crate) dimension: &'static str,
    configurations: Vec<Value>,
}

impl ResourceCase {
    /// This row as the `ScalingInstruction` a plan is created with.
    pub(crate) fn instruction(&self) -> Value {
        json!({
            "ServiceNamespace": self.namespace,
            "ResourceId": self.resource_id,
            "ScalableDimension": self.dimension,
            "MinCapacity": 1,
            "MaxCapacity": 100,
            "TargetTrackingConfigurations": self.configurations.clone()
        })
    }
}

/// The catalog this suite spans: one row per `ScalableDimension` the model lists
/// (`smithy_query(query="shape",shape="ScalableDimension")` — eight values), each with a
/// resource id of the kind that dimension scales and metrics that measure it. The ids
/// come from the worked examples in `ScalingInstruction$ResourceId`'s own documentation
/// (`smithy_query(query="doc",shape="ScalingInstruction",member="ResourceId")`) and the
/// metric types from `ScalingMetricType`
/// (`smithy_query(query="shape",shape="ScalingMetricType")`), with
/// `ALBRequestCountPerTarget` documented as applying "only to Auto Scaling groups, Spot
/// Fleet requests, and ECS services".
///
/// Rows also differ along the NUMBER of configurations the resource carries and the FORM
/// of the metric each tracks (predefined vs customized), and the identities are pairwise
/// distinct, so one plan can cover every row at once.
pub(crate) fn resource_cases() -> Vec<ResourceCase> {
    vec![
        ResourceCase {
            label: "an Auto Scaling group tracking average CPU",
            namespace: "autoscaling",
            resource_id: "autoScalingGroup/my-asg",
            dimension: "autoscaling:autoScalingGroup:DesiredCapacity",
            configurations: vec![predefined("ASGAverageCPUUtilization", 40.0)],
        },
        ResourceCase {
            label: "a second Auto Scaling group tracking three metrics",
            namespace: "autoscaling",
            resource_id: "autoScalingGroup/my-other-asg",
            dimension: "autoscaling:autoScalingGroup:DesiredCapacity",
            configurations: vec![
                predefined("ASGAverageNetworkIn", 1_048_576.0),
                predefined("ASGAverageNetworkOut", 524_288.0),
                predefined("ALBRequestCountPerTarget", 1_000.0),
            ],
        },
        ResourceCase {
            label: "an ECS service tracking average memory",
            namespace: "ecs",
            resource_id: "service/default/sample-webapp",
            dimension: "ecs:service:DesiredCount",
            configurations: vec![predefined("ECSServiceAverageMemoryUtilization", 70.0)],
        },
        ResourceCase {
            label: "a Spot Fleet request tracking a customized metric",
            namespace: "ec2",
            resource_id: "spot-fleet-request/sfr-73fbd2ce-aa30-494c-8788-1cee4EXAMPLE",
            dimension: "ec2:spot-fleet-request:TargetCapacity",
            configurations: vec![json!({
                "CustomizedScalingMetricSpecification": {
                    "MetricName": "backlog-per-instance",
                    "Namespace": "MyApp",
                    "Statistic": "Average",
                    "Dimensions": [{ "Name": "Fleet", "Value": "orders" }],
                    "Unit": "Count"
                },
                "TargetValue": 12.5,
                "ScaleOutCooldown": 30,
                "ScaleInCooldown": 120
            })],
        },
        ResourceCase {
            label: "an Aurora DB cluster tracking two reader metrics",
            namespace: "rds",
            resource_id: "cluster:my-db-cluster",
            dimension: "rds:cluster:ReadReplicaCount",
            configurations: vec![
                predefined("RDSReaderAverageCPUUtilization", 60.0),
                json!({
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": "RDSReaderAverageDatabaseConnections"
                    },
                    "TargetValue": 900.0,
                    "DisableScaleIn": true
                }),
            ],
        },
        ResourceCase {
            label: "a DynamoDB table's read capacity",
            namespace: "dynamodb",
            resource_id: "table/my-table",
            dimension: "dynamodb:table:ReadCapacityUnits",
            configurations: vec![predefined("DynamoDBReadCapacityUtilization", 50.0)],
        },
        ResourceCase {
            label: "a DynamoDB table's write capacity",
            namespace: "dynamodb",
            resource_id: "table/my-table",
            dimension: "dynamodb:table:WriteCapacityUnits",
            configurations: vec![predefined("DynamoDBWriteCapacityUtilization", 65.0)],
        },
        ResourceCase {
            label: "a DynamoDB index's read capacity",
            namespace: "dynamodb",
            resource_id: "table/my-table/index/my-table-index",
            dimension: "dynamodb:index:ReadCapacityUnits",
            configurations: vec![predefined("DynamoDBReadCapacityUtilization", 55.0)],
        },
        ResourceCase {
            label: "a DynamoDB index's write capacity",
            namespace: "dynamodb",
            resource_id: "table/my-table/index/my-table-index",
            dimension: "dynamodb:index:WriteCapacityUnits",
            configurations: vec![predefined("DynamoDBWriteCapacityUtilization", 80.0)],
        },
    ]
}

/// Create `name` from `instructions` and let the clock apply it, so the plan is `Active`
/// and its policies have reached its resources.
async fn applied_plan(emulator: &Emulator, name: &str, instructions: Vec<Value>) {
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": name,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": instructions,
            }),
        )
        .await;
    emulator.make_all_due();
}

/// The resources of `name`, as one unpaged `DescribeScalingPlanResources` reports them.
async fn resources(emulator: &Emulator, name: &str) -> Vec<Value> {
    let body = emulator
        .ok(
            "DescribeScalingPlanResources",
            json!({ "ScalingPlanName": name, "ScalingPlanVersion": 1 }),
        )
        .await;
    body["ScalingPlanResources"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("no ScalingPlanResources in {body:?}"))
}

/// Assert that `resource` reports exactly the policies `configurations` calls for, and
/// collect their names.
fn assert_policies(label: &str, resource: &Value, configurations: &[Value]) -> Vec<String> {
    let policies = resource["ScalingPolicies"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: no ScalingPolicies in {resource:?}"));
    assert_eq!(
        policies.len(),
        configurations.len(),
        "case {label:?}: one policy per target tracking configuration, got {policies:?}"
    );
    let mut names = Vec::with_capacity(policies.len());
    for (policy, configuration) in policies.iter().zip(configurations.iter()) {
        assert_eq!(
            policy.get("PolicyType"),
            Some(&json!(TARGET_TRACKING_SCALING)),
            "case {label:?}: a scaling plan mints target tracking policies, got {policy:?}"
        );
        assert_eq!(
            policy.get("TargetTrackingConfiguration"),
            Some(configuration),
            "case {label:?}: the policy must echo its configuration, got {policy:?}"
        );
        let name = policy["PolicyName"].as_str().unwrap_or_default();
        assert!(
            !name.is_empty() && name.len() <= MAX_POLICY_NAME && !name.chars().any(char::is_control),
            "case {label:?}: PolicyName must be 1-{MAX_POLICY_NAME} printable characters, \
             got {name:?}"
        );
        names.push(name.to_string());
    }
    names
}

#[tokio::test]
async fn every_scalable_resource_a_plan_covers_reports_its_own_target_tracking_policies() {
    for case in resource_cases() {
        let emulator = Emulator::new();
        let label = case.label;
        applied_plan(&emulator, "my-plan", vec![case.instruction()]).await;

        let reported = resources(&emulator, "my-plan").await;
        assert_eq!(
            reported.len(),
            1,
            "case {label:?}: one resource per instruction, got {reported:?}"
        );
        let resource = &reported[0];
        assert_eq!(
            (
                resource.get("ServiceNamespace"),
                resource.get("ResourceId"),
                resource.get("ScalableDimension")
            ),
            (
                Some(&json!(case.namespace)),
                Some(&json!(case.resource_id)),
                Some(&json!(case.dimension))
            ),
            "case {label:?}: the reported resource is the one the instruction names, got \
             {resource:?}"
        );

        let names = assert_policies(label, resource, &case.configurations);
        let unique: BTreeSet<&String> = names.iter().collect();
        assert_eq!(
            unique.len(),
            names.len(),
            "case {label:?}: every policy of a plan needs its own name, got {names:?}"
        );

        // Reading does not re-mint: the same state reports the same policies.
        let again = resources(&emulator, "my-plan").await;
        assert_eq!(
            again, reported,
            "case {label:?}: two reads of the same state must report the same policies"
        );
    }
}

#[tokio::test]
async fn policies_travel_with_their_resource_on_every_page_of_a_paged_read() {
    let cases = resource_cases();
    let emulator = Emulator::new();
    applied_plan(
        &emulator,
        "my-plan",
        cases.iter().map(ResourceCase::instruction).collect(),
    )
    .await;

    let unpaged = resources(&emulator, "my-plan").await;
    assert_eq!(
        unpaged.len(),
        cases.len(),
        "the plan covers one resource per instruction, got {unpaged:?}"
    );

    // One resource per page, walked by the cursor the previous page handed back.
    let mut paged: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({
            "ScalingPlanName": "my-plan",
            "ScalingPlanVersion": 1,
            "MaxResults": 1
        });
        if let Some(token) = next_token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let body = emulator.ok("DescribeScalingPlanResources", request).await;
        let page = body["ScalingPlanResources"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("no ScalingPlanResources in {body:?}"));
        assert_eq!(
            page.len(),
            1,
            "case page {}: MaxResults 1 reports one resource, got {page:?}",
            paged.len()
        );
        // The row this page reports is found by the identity that addresses the
        // resource, not by the page's position in the walk: the resources are reported
        // in the order of that identity, which the order the rows are written in need
        // not match.
        let reported = &page[0];
        let identity = resource_identity(reported);
        let case = cases
            .iter()
            .find(|case| {
                identity
                    == (
                        case.namespace.to_string(),
                        case.resource_id.to_string(),
                        case.dimension.to_string(),
                    )
            })
            .unwrap_or_else(|| panic!("case paging: no row names the resource {reported:?}"));
        assert_policies(case.label, reported, &case.configurations);
        paged.extend(page);
        next_token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            break;
        }
        assert!(
            paged.len() < cases.len(),
            "case paging: a cursor past the last resource must not be handed back, got \
             {paged:?}"
        );
    }

    assert_eq!(
        paged, unpaged,
        "case paging: walking the pages must report the same resources, with the same \
         policies, as one unpaged read"
    );
}
