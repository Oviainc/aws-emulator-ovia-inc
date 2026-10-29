Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ede2de3f7102f867619bb62880127888c08c3e6a

# A ResourceId that is only the resource-type prefix, with no unique identifier after it, is accepted as a scalable resource

_Filed: 2026-10-29T08:36:16Z_
_Operation: GetScalingPlanResourceForecastData | Class: validation_

> _Note: this issue was identified against the code base at commit `ede2de3f7102f867619bb62880127888c08c3e6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::validate_resource_id` is the one place this crate decides whether a `ResourceId` identifies a resource of the kind its `ScalableDimension` scales. It tests only `resource_id.starts_with(prefix)` (plus, for DynamoDB, whether the `/index/` segment is present or absent). It never checks that anything follows the prefix, so a resource id consisting of nothing but the resource type is accepted as naming a resource:

* `autoScalingGroup/` for `autoscaling:autoScalingGroup:DesiredCapacity` (the only triple the forecast read serves);
* `table/` for `dynamodb:table:ReadCapacityUnits`;
* `cluster:` for `rds:cluster:ReadReplicaCount`;
* `service/` for `ecs:service:DesiredCount`, `spot-fleet-request/` for `ec2:spot-fleet-request:TargetCapacity`;
* `table/index/` (or `table//index/my-index`) for `dynamodb:index:ReadCapacityUnits`, where the table name and/or the index name are empty.

This contradicts the model sentence for this operation's own `ResourceId` member, which states the string is a prefix FOLLOWED BY the name of a group, and it contradicts the rest of this crate, which uniformly refuses an identifier whose name part is empty: `is_cloud_formation_stack_arn` requires `stack/` to be followed by a non-empty name, and `named_stack_arn`/`named_tag_key` both treat a present-but-empty value as naming nothing. `validate_resource_id` is the only identifier predicate in the module that lets a bare prefix through.

The helper is `pub` and shared, so the gap shows up on three paths: this operation (which calls it directly on its own `ResourceId`), and `CreateScalingPlan`/`UpdateScalingPlan` through `validate_scaling_instructions`. On the write paths an instruction naming no resource is stored and then echoed by `DescribeScalingPlans` and minted into a `ScalingPlanResource` (with a `PolicyName` digest of that empty identity) by `DescribeScalingPlanResources`.

## Evidence

The model sentence for THIS operation's `ResourceId` member — `smithy_query(query="shape",shape="GetScalingPlanResourceForecastDataRequest")`, member `ResourceId`:

> "The ID of the resource. This string consists of a prefix (`autoScalingGroup`) **followed by the name of a specified Auto Scaling group** (`my-asg`). Example: `autoScalingGroup/my-asg`."

and for the instruction member the same helper guards — `smithy_query(query="shape",shape="ScalingInstruction")`, member `ResourceId`:

> "The ID of the resource. **This string consists of the resource type and unique identifier.** … Auto Scaling group - The resource type is `autoScalingGroup` and the unique identifier is the name of the Auto Scaling group. Example: `autoScalingGroup/my-asg`."

The handler hands its own member to the shared helper — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/get_scaling_plan_resource_forecast_data.rs:377-386:
```
fn validate_forecast_resource(
    input: &GetScalingPlanResourceForecastDataInput,
) -> Result<(), String> {
    if input.service_namespace != FORECAST_SERVICE_NAMESPACE
        || input.scalable_dimension != FORECAST_SCALABLE_DIMENSION
    {
        return Err(crate::state::UNSUPPORTED_RESOURCE.to_string());
    }
    crate::state::validate_resource_id(&input.resource_id, &input.scalable_dimension)
}
```

The helper, which never looks past the prefix — src/state.rs:1232-1252:
```
/// A resource id has to identify a resource of the kind its scalable dimension scales.
///
/// `pub` so the forecast read applies the same rule to its own `ResourceId` as the two
/// write paths apply to a scaling instruction's.
pub fn validate_resource_id(resource_id: &str, dimension: &str) -> Result<(), String> {
    // A dimension the model does not list is refused by its own enum check, not here.
    let Some((prefix, _)) = scalable_dimension(dimension) else {
        return Ok(());
    };
    let resource_type = dimension.split(':').nth(1).unwrap_or_default();
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
    Ok(())
}
```

The other side of the crate's own rule — the sibling identifier predicate in the SAME module DOES require a non-empty name after its type prefix, src/state.rs:693-699:
```
    !partition.is_empty()
        && !region.is_empty()
        && !account.is_empty()
        && resource
            .strip_prefix("stack/")
            .is_some_and(|name| !name.is_empty())
```
and src/state.rs:649-651, where a present-but-empty value names nothing:
```
fn named_tag_key(filter: &TagFilter) -> Option<&str> {
    filter.key.as_deref().filter(|key| !key.is_empty())
}
```

Test coverage: no test sends a bare prefix. Every `ResourceId` row in tests/all/scaling_instruction_validation.rs carries a name (`table/my-table` at :151 and :157, `cluster:my-db-cluster` at :176, `service/default/sample-webapp` at :438), and tests/all/policy_name_identity.rs:149-160 only builds `autoScalingGroup/` + padding at `MAX_RESOURCE_ID` (1600), never at the prefix length itself. Refusing a bare prefix therefore leaves every existing test passing.

## Steps to Reproduce

1. CreateScalingPlan with ScalingPlanName "bare-prefix", a valid CloudFormation stack ApplicationSource, and one scaling instruction with ServiceNamespace "autoscaling", ScalableDimension "autoscaling:autoScalingGroup:DesiredCapacity", ResourceId "autoScalingGroup/" (nothing after the slash), MinCapacity 1, MaxCapacity 10, one TargetTrackingConfiguration tracking ASGAverageCPUUtilization at 40, and PredefinedLoadMetricSpecification ASGTotalCPUUtilization.
2. Observe HTTP 200 with ScalingPlanVersion 1 — an instruction that names no Auto Scaling group is stored.
3. Let the plan settle, then DescribeScalingPlanResources(ScalingPlanName="bare-prefix", ScalingPlanVersion=1): a ScalingPlanResource comes back with ResourceId "autoScalingGroup/" and a minted TargetTrackingScaling PolicyName keyed on that empty identity.
4. GetScalingPlanResourceForecastData(ScalingPlanName="bare-prefix", ScalingPlanVersion=1, ServiceNamespace="autoscaling", ScalableDimension="autoscaling:autoScalingGroup:DesiredCapacity", ResourceId="autoScalingGroup/", ForecastDataType="CapacityForecast", StartTime/EndTime one hour apart in the recent past): HTTP 200 with computed Datapoints for a resource that does not exist.
5. Contrast with the sibling predicate in the same module: CreateScalingPlan with ApplicationSource {"CloudFormationStackARN": "arn:aws:cloudformation:us-east-1:000000000000:stack/"} IS refused with "Invalid CloudFormation stack ARN: …", because that path requires a non-empty name after its `stack/` prefix.
6. Repeat step 1 with ResourceId "table/" on dynamodb:table:ReadCapacityUnits, "cluster:" on rds:cluster:ReadReplicaCount, and "table/index/" on dynamodb:index:ReadCapacityUnits — all accepted.
