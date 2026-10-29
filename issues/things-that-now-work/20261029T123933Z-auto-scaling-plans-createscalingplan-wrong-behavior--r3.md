Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c73cb477601ff7c7855bce46e4f89627c43b834e

# A predefined metric's identity drops ResourceLabel, so two configurations for different target groups render one identity (false duplicate refusal, and an update stores a configuration never sent)

_Filed: 2026-10-29T12:39:33Z_
_Operation: CreateScalingPlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c73cb477601ff7c7855bce46e4f89627c43b834e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::target_metric_identity` is the single definition of "the metric a target tracking configuration tracks". It is used for three rules of this crate: the uniqueness check across one instruction (`validate_target_tracking_configurations`), the policy name a resource reports (`policy_name`), and the pairing an update merges by (`keep_tracked_target_values`).

For a CUSTOMIZED metric the identity carries every part that identifies the metric — namespace, name, statistic and the dimension SET — and the function's own doc states why: "two different metrics cannot render one identity". For a PREDEFINED metric it returns the metric TYPE alone and drops `ResourceLabel`, the member the model documents as "Identifies the resource associated with the metric type" (the final portions of the load balancer ARN and the target group ARN). Two `ALBRequestCountPerTarget` specifications naming two different target groups are therefore ONE identity, which is exactly what the doc says cannot happen.

Two observable consequences:

1. CreateScalingPlan (and UpdateScalingPlan, through the same shared validator) refuses an instruction carrying two `ALBRequestCountPerTarget` configurations whose `ResourceLabel`s name different target groups with "each target tracking configuration must specify a unique scaling metric, and 'ALBRequestCountPerTarget' is already specified" — although they track two different resources. The crate's customized-metric analogue behaves the opposite way and is pinned by a test: two configurations differing only in their dimension SET are accepted and both stored.

2. On UpdateScalingPlan the merge pairs the requested configuration with the in-force one by that same identity, so an update that re-points an ALB metric to another target group (new `ResourceLabel`, new `TargetValue`) stores the NEW label with the OLD target value: a configuration the caller never sent and that was never in force. The merge rule's stated justification — the kept value "is what the applied policy tracks to" — does not hold here, because the policy in force tracks the OTHER target group. `policy_name` digests the same (unchanged) identity, so the resource keeps reporting the same `PolicyName` while the target group it tracks has changed.

## Evidence

Model text that makes the label part of the metric's identity — `smithy_query(query="doc",shape="PredefinedScalingMetricSpecification",member="ResourceLabel")`:

> "Identifies the resource associated with the metric type. You can't specify a resource label unless the metric type is `ALBRequestCountPerTarget` and there is a target group for an Application Load Balancer attached to the Auto Scaling group, Spot Fleet request, or ECS service. You create the resource label by appending the final portion of the load balancer ARN and the final portion of the target group ARN into a single value, separated by a forward slash (/)."

The crate's own rule for this identity — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:1086-1093:
```
/// A customized metric's parts are caller-supplied text under no modelled pattern at
/// all (`MetricNamespace`, `MetricName`, `MetricDimensionName` and
/// `MetricDimensionValue` are plain strings), so each is escaped on the way into the
/// key and two different metrics cannot render one identity. The dimensions are the set
/// of dimensions the metric was published with, so they reach the key through
/// [`set_key_parts`]: a reordered or a repeated dimension is the same set, hence the
/// same metric. A predefined metric type is one part, and one the model constrains to
/// its own value list.
```
The code under it, which returns the type alone and never reads `resource_label` — state.rs:1094-1097:
```
pub fn target_metric_identity(configuration: &TargetTrackingConfiguration) -> Option<String> {
    if let Some(predefined) = &configuration.predefined_scaling_metric_specification {
        return Some(predefined.predefined_scaling_metric_type.clone());
    }
```
(the customized branch, state.rs:1115-1121, renders `namespace/metric_name/statistic[dimensions]` — every identifying part).

Symptom 1 — the uniqueness check that consumes that identity, state.rs:1277-1289:
```
    for (position, configuration) in configurations.iter().enumerate() {
        let at = format!("{member}[{position}]");
        let metric = validate_target_tracking_configuration(...)?;
        if !metrics.insert(metric.clone()) {
            return Err(format!(
                "{at}: each target tracking configuration must specify a unique scaling \
                 metric, and '{metric}' is already specified"
            ));
        }
```
reached from CreateScalingPlan — src/ops/create_scaling_plan.rs:169-170:
```
        crate::state::validate_scaling_instructions(&input.scaling_instructions)
            .map_err(validation)?;
```
The opposite treatment of the customized sibling, pinned by a test — tests/all/set_identity_multiplicity.rs:240-251 (rows whose `same_metric` flag is `false`) and :286-301:
```
        (
            "a dimension added, not repeated",
            vec![("a", "1")],
            vec![("a", "1"), ("b", "2")],
            false,
        ),
...
        assert_eq!(
            status, 200,
            "case {case:?}: two configurations tracking different metrics must be \
             accepted, got {status} with {body:?}"
        );
```
No test sends two `ALBRequestCountPerTarget` configurations with different labels (`grep ResourceLabel tests/` returns only scaling_instruction_validation.rs:236, :319, :428, :483, each a single configuration with the same label value), so this branch is unpinned.

Symptom 2 — the merge pairing, state.rs:1020-1031:
```
    requested.target_tracking_configurations = requested
        .target_tracking_configurations
        .into_iter()
        .map(|mut configuration| {
            let metric = target_metric_identity(&configuration);
            let in_force = applied
                .target_tracking_configurations
                .iter()
                .find(|in_force| metric.is_some() && target_metric_identity(in_force) == metric);
            if let Some(in_force) = in_force {
                configuration.target_value = in_force.target_value;
            }
```
against its own stated precondition, state.rs:970-974:
```
/// An update does not re-point a target tracking policy a resource already has: for a
/// resource the plan already covers, the TARGET VALUE in force for a scaling metric the
/// resource already tracks is kept, because that value is what the applied policy tracks
/// to.
```
and the policy name built from the same identity, state.rs:362-369 / :385-391:
```
/// policies of one plan share a name, every name stays inside the modelled 256
...
fn policy_name(plan_name: &str, resource: &str, configuration: &str) -> String {
    format!(
        "{plan_name}/TargetTracking/{}/{}",
        identity_digest(resource),
        identity_digest(configuration)
    )
```
reached from src/ops/describe_scaling_plan_resources.rs:245 (`let policies = plan.applied_policies(instruction);`).

## Steps to Reproduce

1. CreateScalingPlan {"ScalingPlanName":"label-identity","ApplicationSource":{"CloudFormationStackARN":"arn:aws:cloudformation:us-east-1:000000000000:stack/s/abc"},"ScalingInstructions":[{"ServiceNamespace":"autoscaling","ResourceId":"autoScalingGroup/my-asg","ScalableDimension":"autoscaling:autoScalingGroup:DesiredCapacity","MinCapacity":1,"MaxCapacity":10,"TargetTrackingConfigurations":[{"PredefinedScalingMetricSpecification":{"PredefinedScalingMetricType":"ALBRequestCountPerTarget","ResourceLabel":"app/my-lb/abc/targetgroup/tg-a/111"},"TargetValue":1000.0},{"PredefinedScalingMetricSpecification":{"PredefinedScalingMetricType":"ALBRequestCountPerTarget","ResourceLabel":"app/my-lb/abc/targetgroup/tg-b/222"},"TargetValue":2000.0}]}]}
2. Observe HTTP 400 ValidationException "...: each target tracking configuration must specify a unique scaling metric, and 'ALBRequestCountPerTarget' is already specified", although the two labels identify two different target groups (two different resources, per the member's own documentation). Compare with the customized analogue, which IS accepted: the same request with two CustomizedScalingMetricSpecifications differing only in their Dimensions is answered 200 and both configurations are stored.
3. Now the stored-state symptom: CreateScalingPlan {"ScalingPlanName":"p","ApplicationSource":{"CloudFormationStackARN":"arn:aws:cloudformation:us-east-1:000000000000:stack/s/abc"},"ScalingInstructions":[ the same ASG instruction with ONE configuration: ALBRequestCountPerTarget, ResourceLabel "app/my-lb/abc/targetgroup/tg-a/111", TargetValue 1000.0 ]} -> 200, then let the clock settle the plan to Active.
4. UpdateScalingPlan {"ScalingPlanName":"p","ScalingPlanVersion":1,"ScalingInstructions":[ the same instruction but ResourceLabel "app/my-lb/abc/targetgroup/tg-b/222" and TargetValue 500.0 ]} -> 200, empty body.
5. Let the clock settle, then DescribeScalingPlans {"ScalingPlanNames":["p"]}: the stored configuration is {ResourceLabel: "app/my-lb/abc/targetgroup/tg-b/222", TargetValue: 1000.0} — neither what was in force (tg-a/1000) nor what was requested (tg-b/500), because the merge treated the two labels as one metric and kept the old target value.
6. DescribeScalingPlanResources {"ScalingPlanName":"p","ScalingPlanVersion":1}: the reported ScalingPolicies[0].PolicyName is unchanged from step 3, so the policy claimed to track tg-a is now reported as tracking tg-b under the same name and the old target value.
