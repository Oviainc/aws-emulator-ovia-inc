Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Application Auto Scaling (AnyScaleFrontendService) emulator

_Filed: 2026-10-21T21:18:19Z_
_Operation: service-com-amazonaws-applicationautoscaling-anyscalefrontendservice | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 14 operations in `src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` with zero persisted state, so no client workflow works at all. Implement a FIRST, SMALL slice: the scalable-target + scaling-policy lifecycle, stateful end to end over the `awsJson1_1` wire protocol (`POST /`, `X-Amz-Target: AnyScaleFrontendService.<OperationName>`).

Happy path to implement, in order:

1. **RegisterScalableTarget** — input `ServiceNamespace` (required enum), `ResourceId` (required), `ScalableDimension` (required enum), `MinCapacity`, `MaxCapacity`, `RoleARN`, `SuspendedState`, `Tags`. Create (or update in place, since Register is upsert-by-key) the target keyed by (ServiceNamespace, ResourceId, ScalableDimension); record `CreationTime` on first registration and keep it stable on re-registration; on an update, members absent from the request must keep their previously stored values. Return the generated `ScalableTargetARN` (e.g. `arn:aws:application-autoscaling:us-east-1:123456789012:scalable-target/<opaque-id>`) and store it so later calls return the same ARN.
2. **DescribeScalableTargets** — input `ServiceNamespace` (required), optional `ResourceIds` (len 0..50), `ScalableDimension`, `MaxResults`, `NextToken`. Must return `ScalableTargets` built from stored targets filtered by namespace (+ ResourceIds/ScalableDimension when given). Each returned `ScalableTarget` carries the stored `ServiceNamespace`, `ResourceId`, `ScalableDimension`, `MinCapacity`, `MaxCapacity`, `RoleARN`, `CreationTime`, `SuspendedState`, `ScalableTargetARN` — i.e. exactly what Register persisted, not echoed request fields. Honour `MaxResults` by truncating and emitting an opaque `NextToken` that resumes where the page ended; reject an unknown/corrupt token with `InvalidNextTokenException` (400).
3. **PutScalingPolicy** — input `PolicyName` (required, len 1..256), `ServiceNamespace`, `ResourceId`, `ScalableDimension` (all required), `PolicyType` (`TargetTrackingScaling` | `StepScaling` | `PredictiveScaling`), plus the matching `StepScalingPolicyConfiguration` / `TargetTrackingScalingPolicyConfiguration` / `PredictiveScalingPolicyConfiguration`. Must fail with `ObjectNotFoundException` (400) when no scalable target is registered for the key. On success persist the policy (upsert by (namespace, resourceId, dimension, policyName), defaulting `PolicyType` to `TargetTrackingScaling` when omitted, stamping `CreationTime`) and return the stored required `PolicyARN` (`.../scalingPolicy:<id>:resource/<...>:policyName/<name>`) — stable across repeated Put calls for the same policy — plus `Alarms` as persisted (empty list is fine for the first slice).
4. **DescribeScalingPolicies** — input `ServiceNamespace` (required), optional `PolicyNames` (0..50), `ResourceId`, `ScalableDimension`, `MaxResults`, `NextToken`. Return `ScalingPolicies` assembled from stored policies with the filters applied; each entry must include the stored `PolicyARN`, `PolicyName`, `ServiceNamespace`, `ResourceId`, `ScalableDimension`, `PolicyType`, the configuration structure as submitted, `Alarms` and `CreationTime`. Same pagination/`InvalidNextTokenException` rules as above.
5. **DeleteScalingPolicy** — input `PolicyName`, `ServiceNamespace`, `ResourceId`, `ScalableDimension` (all required). Remove the stored policy; a subsequent DescribeScalingPolicies must no longer list it. Deleting an unknown policy must return `ObjectNotFoundException` (400), not an empty success.
6. **DeregisterScalableTarget** — input `ServiceNamespace`, `ResourceId`, `ScalableDimension` (all required). Remove the target AND cascade-delete its scaling policies and scheduled actions; DescribeScalableTargets must then not list it, and deregistering an unregistered target must return `ObjectNotFoundException` (400).

No canned or echoed responses: every response field above must be computed from persisted state held in the shared engine store (`crate::state::service_state(&*self.engine)`), so a Register followed by a Describe in a later request returns what was registered, and a Delete/Deregister is observable by the next reader. Required-member and enum/length/pattern violations surface as `ValidationException` (400).

## State Model

In `ComAmazonawsApplicationautoscalingAnyscalefrontendserviceState`:

- `targets: BTreeMap<TargetKey, ScalableTargetRecord>` where `TargetKey { service_namespace: String, resource_id: String, scalable_dimension: String }` (the natural AWS identity of a scalable target) and `ScalableTargetRecord { arn: String, min_capacity: Option<i32>, max_capacity: Option<i32>, role_arn: String, creation_time: SystemTime, suspended_state: Option<SuspendedState>, tags: BTreeMap<String, String> }`.
- `policies: BTreeMap<PolicyKey, ScalingPolicyRecord>` where `PolicyKey { target: TargetKey, policy_name: String }` and `ScalingPolicyRecord { policy_arn: String, policy_type: String, step_config: Option<StepScalingPolicyConfiguration>, target_tracking_config: Option<TargetTrackingScalingPolicyConfiguration>, predictive_config: Option<PredictiveScalingPolicyConfiguration>, alarms: Vec<Alarm>, creation_time: SystemTime }`.
- `arn_index: BTreeMap<String, TargetKey>` — maps a `ScalableTargetARN` back to its target so the later tag/ListTagsForResource slice can resolve `ResourceARN` (tags already persisted on the record).
- `scheduled_actions: BTreeMap<ScheduledActionKey, ScheduledActionRecord>` — reserved for the next slice but declared now so Deregister's cascade has something concrete to clear.
- Ordering: both maps are ordered by key so Describe* pagination is stable and an opaque `NextToken` can simply encode the last key emitted.

Out of scope for this first slice (stay 501, filed separately later): PutScheduledAction, DescribeScheduledActions, DeleteScheduledAction, DescribeScalingActivities, GetPredictiveScalingForecast, TagResource, UntagResource, ListTagsForResource.

## Evidence

`services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/OPERATIONS.md` lists all 14 operations with their input/output members and errors (RegisterScalableTarget -> `ScalableTargetARN`; DescribeScalableTargets `@paginated(inputToken=NextToken, outputToken=NextToken, items=ScalableTargets)`; PutScalingPolicy -> required `PolicyARN` + `Alarms`, errors include `ObjectNotFoundException`; DeleteScalingPolicy/DeregisterScalableTarget declare `ObjectNotFoundException`; Describe* declare `InvalidNextTokenException`). `smithy_query shape=ScalableTarget,ScalingPolicy` shows the output records the Describe ops must reconstruct from state (ScalableTarget requires ServiceNamespace, ResourceId, ScalableDimension, MinCapacity, MaxCapacity, RoleARN, CreationTime; ScalingPolicy requires PolicyARN, PolicyName, ServiceNamespace, ResourceId, ScalableDimension, PolicyType, CreationTime). `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 14 handler files, confirming every operation returns HTTP 501 today; README.md documents the awsJson1_1 routing and the shared-engine state API.

## Steps to Reproduce

1. Start the emulator and point a client at it with signing name `application-autoscaling`.
2. POST / with X-Amz-Target: AnyScaleFrontendService.RegisterScalableTarget and body {"ServiceNamespace":"dynamodb","ResourceId":"table/my-table","ScalableDimension":"dynamodb:table:ReadCapacityUnits","MinCapacity":1,"MaxCapacity":10,"RoleARN":"arn:aws:iam::123456789012:role/aas"} — expect 200 with a ScalableTargetARN (today: 501 NotImplemented).
3. DescribeScalableTargets {"ServiceNamespace":"dynamodb"} — expect one ScalableTarget echoing the persisted MinCapacity=1, MaxCapacity=10, RoleARN, a CreationTime and the same ScalableTargetARN.
4. PutScalingPolicy {"PolicyName":"my-policy","ServiceNamespace":"dynamodb","ResourceId":"table/my-table","ScalableDimension":"dynamodb:table:ReadCapacityUnits","PolicyType":"TargetTrackingScaling","TargetTrackingScalingPolicyConfiguration":{"TargetValue":50.0,"PredefinedMetricSpecification":{"PredefinedMetricType":"DynamoDBReadCapacityUtilization"}}} — expect 200 with a PolicyARN.
5. DescribeScalingPolicies {"ServiceNamespace":"dynamodb","PolicyNames":["my-policy"]} — expect the policy with the same PolicyARN, PolicyType and the TargetTrackingScalingPolicyConfiguration as submitted.
6. PutScalingPolicy for an unregistered ResourceId (e.g. table/absent) — expect ObjectNotFoundException (HTTP 400).
7. DescribeScalableTargets {"ServiceNamespace":"dynamodb","MaxResults":1} after registering two targets — expect 1 item plus a NextToken; replay with that NextToken to get the second item; a garbage NextToken must give InvalidNextTokenException (400).
8. DeleteScalingPolicy for my-policy — expect 200; DescribeScalingPolicies no longer lists it; repeating the delete gives ObjectNotFoundException (400).
9. DeregisterScalableTarget for table/my-table — expect 200; DescribeScalableTargets returns an empty ScalableTargets list and repeating the deregister gives ObjectNotFoundException (400).
