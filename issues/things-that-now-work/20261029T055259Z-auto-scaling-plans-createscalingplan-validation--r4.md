Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0ab5854f8833828180776b987700bd810a961abd

# A ResourceLabel is accepted in any shape, although both members document their format explicitly and the crate enforces the documented format of every sibling identifier

_Filed: 2026-10-29T05:52:59Z_
_Operation: CreateScalingPlan | Class: validation_

> _Note: this issue was identified against the code base at commit `0ab5854f8833828180776b987700bd810a961abd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PredefinedScalingMetricSpecification$ResourceLabel` and `PredefinedLoadMetricSpecification$ResourceLabel` each document the FORMAT of the value they carry ("The format is app/<load-balancer-name>/<load-balancer-id>/targetgroup/<target-group-name>/<target-group-id>"), together with how it is built (the final portion of the load balancer ARN and of the target group ARN joined by a forward slash) and a worked example.

The write path reads only the FIRST sentence of that same paragraph — the condition that the metric type must be the ALB one — and never looks at the value itself. Any string the member's modelled `length {min:1,max:1023}` admits is accepted: `"ResourceLabel": "not-a-label"` on an `ALBRequestCountPerTarget` configuration is stored verbatim, echoed back by `DescribeScalingPlans`, and carried inside the `TargetTrackingConfiguration` of the `ScalingPolicy` that `DescribeScalingPlanResources` mints from that instruction — so a label that identifies no load balancer target group is reported as the identity of the resource the policy tracks.

This is also inconsistent with how this crate treats every OTHER identifier whose format is documented rather than modelled:

* `ScalingInstruction$ResourceId` carries no format in its doc at all, only worked EXAMPLES, and the crate still derives a required prefix per scalable dimension from them and refuses a mismatch (`validate_resource_id`);
* `ApplicationSource$CloudFormationStackARN` is documented only as "The Amazon Resource Name (ARN) of a AWS CloudFormation stack", and the crate still decomposes it into ARN fields and refuses one that does not name the `cloudformation` service and a `stack/…` resource (`is_cloud_formation_stack_arn`).

The member whose documentation states its format OUTRIGHT is the only one left unchecked. Both write paths are affected, since `CreateScalingPlan` and `UpdateScalingPlan` share `crate::state::validate_scaling_instructions`.

## Evidence

Model text that states the format — `smithy_query(query="doc",shape="PredefinedScalingMetricSpecification",member="ResourceLabel")`:

> "Identifies the resource associated with the metric type. You can't specify a resource label unless the metric type is `ALBRequestCountPerTarget` and there is a target group for an Application Load Balancer attached to the Auto Scaling group, Spot Fleet request, or ECS service. You create the resource label by appending the final portion of the load balancer ARN and the final portion of the target group ARN into a single value, separated by a forward slash (/). **The format is app/<load-balancer-name>/<load-balancer-id>/targetgroup/<target-group-name>/<target-group-id>** … This is an example: app/EC2Co-EcsEl-1TKLTMITMM0EO/f37c06a68c1748aa/targetgroup/EC2Co-Defau-LDNM7Q3ZH1ZN/6d4ea56ca2d6a18d."

The same paragraph is on the load-metric sibling — `smithy_query(query="doc",shape="PredefinedLoadMetricSpecification",member="ResourceLabel")`: "… unless the metric type is `ALBTargetGroupRequestCount` … **The format is app/<load-balancer-name>/<load-balancer-id>/targetgroup/<target-group-name>/<target-group-id>**".

What the crate takes from that paragraph — only its first sentence. services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:846-853:
```
/// The only `ScalingMetricType` a `PredefinedScalingMetricSpecification$ResourceLabel`
/// may accompany: "You can't specify a resource label unless the metric type is
/// ALBRequestCountPerTarget".
pub const ALB_SCALING_METRIC_TYPE: &str = "ALBRequestCountPerTarget";

/// The same rule for `PredefinedLoadMetricSpecification$ResourceLabel`: "You can't
/// specify a resource label unless the metric type is ALBTargetGroupRequestCount".
pub const ALB_LOAD_METRIC_TYPE: &str = "ALBTargetGroupRequestCount";
```
and the two checks that use them — the value itself is never inspected. state.rs:1384-1389:
```
    if specification.resource_label.is_some() && metric != ALB_SCALING_METRIC_TYPE {
        return Err(format!(
            "{member}.ResourceLabel: a resource label can only be specified when the metric \
             type is {ALB_SCALING_METRIC_TYPE}"
        ));
    }
    Ok(())
```
state.rs:1462-1469:
```
        if specification.resource_label.is_some()
            && specification.predefined_load_metric_type != ALB_LOAD_METRIC_TYPE
        {
            return Err(format!(
                "{member}.ResourceLabel: a resource label can only be specified when the \
                 metric type is {ALB_LOAD_METRIC_TYPE}"
            ));
        }
```
The other side of the inconsistency — the crate DOES enforce a format taken from mere examples for the sibling identifier. state.rs:873-876 (the table's own comment):
```
/// Columns: the scalable dimension; where the resource id of a resource it applies to
/// starts, taken from the worked examples in `ScalingInstruction$ResourceId`'s own
/// documentation (`smithy_query(query="doc",shape="ScalingInstruction",member="ResourceId")`)
```
state.rs:1242-1251:
```
    let identifies = resource_id.starts_with(prefix)
        && match resource_type {
            // A global secondary index is addressed through its table, a table is not.
            "index" => resource_id.contains(INDEX_SEGMENT),
            "table" => !resource_id.contains(INDEX_SEGMENT),
            _ => true,
        };
    if !identifies {
        return Err(UNSUPPORTED_RESOURCE.to_string());
    }
```
and the same for the stack ARN — state.rs:667-699:
```
/// `ApplicationSource$CloudFormationStackARN` is documented as "The Amazon Resource
/// Name (ARN) of a AWS CloudFormation stack", and an ARN is
/// `arn:<partition>:<service>:<region>:<account>:<resource>` — so a stack ARN names the
/// `cloudformation` service and a `stack/…` resource.
fn is_cloud_formation_stack_arn(arn: &str) -> bool {
```

Test coverage: every `ResourceLabel` any test sends is already in the documented shape — tests/all/scaling_instruction_validation.rs:236, :319, :428 and :483 all use `"app/my-lb/abc/targetgroup/my-tg/def"`, and the only label rows that assert a refusal ("a resource label on a metric type that is not the ALB one", line 230) are refused for the metric type, not the value. No test pins a malformed label as accepted, so enforcing the documented format leaves every existing test passing.

## Steps to Reproduce

1. CreateScalingPlan with ScalingPlanName "label-probe", a CloudFormation stack ApplicationSource, and one scaling instruction for ServiceNamespace `autoscaling`, ResourceId `autoScalingGroup/my-asg`, ScalableDimension `autoscaling:autoScalingGroup:DesiredCapacity`, MinCapacity 1, MaxCapacity 10 and one TargetTrackingConfiguration whose PredefinedScalingMetricSpecification is `{"PredefinedScalingMetricType": "ALBRequestCountPerTarget", "ResourceLabel": "not-a-label"}` with TargetValue 1000.
2. Observe HTTP 200 with ScalingPlanVersion 1: the label is accepted although it is neither `app/<lb-name>/<lb-id>/targetgroup/<tg-name>/<tg-id>` nor built from any load balancer / target group ARN.
3. Let the plan settle, then DescribeScalingPlans(ScalingPlanNames=["label-probe"]) — the instruction comes back carrying `"ResourceLabel": "not-a-label"`.
4. DescribeScalingPlanResources(ScalingPlanName="label-probe", ScalingPlanVersion=1) — the minted ScalingPolicy's TargetTrackingConfiguration reports the same unusable label as the identity of the tracked resource.
5. Compare with the sibling identifier in the same instruction: re-send the create with ResourceId `my-asg` instead of `autoScalingGroup/my-asg` and it IS refused ("Unsupported service namespace, resource type or scalable dimension"), even though that format comes only from an example in the doc, while ResourceLabel's format is stated outright and goes unchecked.
6. Repeat steps 1-2 with UpdateScalingPlan (same instruction, ApplicationSource omitted): the shared validator accepts the malformed label there too.
