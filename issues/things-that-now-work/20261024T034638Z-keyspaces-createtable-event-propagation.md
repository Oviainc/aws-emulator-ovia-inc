Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# Keyspaces auto scaling never reaches Application Auto Scaling: no scalable target or scaling policy is registered for a table created with autoScalingSpecification

_Filed: 2026-10-24T03:46:38Z_
_Operation: CreateTable | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Amazon Keyspaces auto scaling is implemented ON TOP of Application Auto Scaling. When CreateTable is called in PROVISIONED mode with `autoScalingSpecification`, the real service registers, in Application Auto Scaling, a scalable target per capacity dimension — ServiceNamespace `cassandra`, ResourceId `keyspace/<keyspaceName>/table/<tableName>`, ScalableDimension `cassandra:table:ReadCapacityUnits` and/or `cassandra:table:WriteCapacityUnits`, MinCapacity/MaxCapacity taken from `minimumUnits`/`maximumUnits` — plus a TargetTrackingScaling policy built from `scalingPolicy.targetTrackingScalingPolicyConfiguration` (targetValue, disableScaleIn, scaleInCooldown, scaleOutCooldown). A caller can then see them with `application-autoscaling:DescribeScalableTargets --service-namespace cassandra` and `DescribeScalingPolicies`, and the targets/policies disappear again when the table is deleted (DeleteTable deregisters them; a later UpdateTable that changes the settings re-registers/updates them).

In the emulator the auto scaling settings are stored ONLY inside the Keyspaces crate's own `Table` record and are never propagated: `DescribeScalableTargets --service-namespace cassandra` returns an empty list after a Keyspaces CreateTable that configured auto scaling, and a scalable target that a caller registers directly through Application Auto Scaling is invisible to GetTableAutoScalingSettings. The two services therefore hold two disconnected views of the same resource.

## Evidence

Spec: the Keyspaces model's own documentation for GetTableAutoScalingSettings states that to use it you must allow `application-autoscaling:DescribeScalableTargets` and `application-autoscaling:DescribeScalingPolicies` — i.e. the settings ARE Application Auto Scaling resources. The Application Auto Scaling model declares ServiceNamespace `cassandra` and ScalableDimension `cassandra:table:ReadCapacityUnits`/`cassandra:table:WriteCapacityUnits`, and that emulator already carries the resource-id template for it (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/state.rs: `("cassandra", "table", "keyspace/{keyspace}/table/{table}")` and the per-dimension minimum table).

Handler: services/.../keyspaces-keyspacesservice/src/ops/create_table.rs stores `auto_scaling_specification` into `crate::state::Table` and touches no other service's store; get_table_auto_scaling_settings.rs reads it straight back out of that same record; delete_table.rs removes the table without deregistering anything.

Feasibility/precedent: cross-service writes are an established pattern in this repo — the Application Auto Scaling crate writes CloudWatch alarms into the CloudWatch crate's state (its src/state.rs ~line 2148 uses `service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state`).

Tests: tests/all/column_types_and_auto_scaling.rs only round-trips the specification through Keyspaces' own GetTableAutoScalingSettings; no test asserts any Application Auto Scaling visibility, so nothing covers this.

## Steps to Reproduce

1. CreateKeyspace {keyspaceName: ks}
2. CreateTable {keyspaceName: ks, tableName: t, schemaDefinition: {...}, capacitySpecification: {throughputMode: PROVISIONED, readCapacityUnits: 5, writeCapacityUnits: 5}, autoScalingSpecification: {readCapacityAutoScaling: {autoScalingDisabled: false, minimumUnits: 5, maximumUnits: 50, scalingPolicy: {targetTrackingScalingPolicyConfiguration: {targetValue: 60}}}}}
3. Call Application Auto Scaling DescribeScalableTargets {ServiceNamespace: "cassandra"} — expected: one target for ResourceId keyspace/ks/table/t, ScalableDimension cassandra:table:ReadCapacityUnits with MinCapacity 5 / MaxCapacity 50; actual: ScalableTargets is empty
4. Call Application Auto Scaling DescribeScalingPolicies {ServiceNamespace: "cassandra"} — expected: a TargetTrackingScaling policy for that target; actual: empty
5. DeleteTable {keyspaceName: ks, tableName: t}, then DescribeScalableTargets again — expected: the target is gone because the table's deletion deregistered it; actual: nothing was ever there to deregister
