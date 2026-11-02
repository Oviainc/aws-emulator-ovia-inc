Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.describe-dimension-filter-without-a-resource-id-rejected application-autoscaling.register-upsert-carrying-tags-rejected-and-bounds-kept application-autoscaling.keyspaces-missing-table-in-an-existing-keyspace-rejected application-autoscaling.keyspaces-on-demand-table-register-rejected application-autoscaling.target-value-above-dynamodb-utilization-range-rejected application-autoscaling.deregister-denied-by-a-resource-tag-condition application-autoscaling.register-outside-the-allowed-scalable-dimension-denied application-autoscaling.register-with-tags-denied-without-tag-resource-permission application-autoscaling.role-arn-is-ignored-for-a-service-linked-namespace application-autoscaling.deregister-in-one-region-keeps-the-same-named-target-in-another application-autoscaling.deploy-cloudformation-scalable-target-and-policy-scale-the-table application-autoscaling.deploy-cloudformation-scalable-target-min-above-max-rejected application-autoscaling.concurrent-register-scalable-target-updates-consistent
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.describe-dimension-filter-without-a-resource-id-rejected|_error#11|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {listed!r}')
Issue-Key: application-autoscaling.register-upsert-carrying-tags-rejected-and-bounds-kept|_client_error#11|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: application-autoscaling.keyspaces-missing-table-in-an-existing-keyspace-rejected|_error#6|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: application-autoscaling.keyspaces-on-demand-table-register-rejected|_error#7|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: application-autoscaling.target-value-above-dynamodb-utilization-range-rejected|run#8|AssertionError|assert msg == (f"For predefined metric type {_METRIC}, target value must be between "
Issue-Key: application-autoscaling.deregister-denied-by-a-resource-tag-condition|_error#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling.register-outside-the-allowed-scalable-dimension-denied|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling.register-with-tags-denied-without-tag-resource-permission|_error#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling.role-arn-is-ignored-for-a-service-linked-namespace|run#13|AssertionError|assert target["RoleARN"] == slr, (
Issue-Key: application-autoscaling.deregister-in-one-region-keeps-the-same-named-target-in-another|run#17|AssertionError|assert [(t['MinCapacity'], t['MaxCapacity']) for t in far_targets] == [(1, 5)], (
Issue-Key: application-autoscaling.deploy-cloudformation-scalable-target-and-policy-scale-the-table|run#16|AssertionError|assert target is not None, "no scalable target is registered for the stack's table"
Issue-Key: application-autoscaling.deploy-cloudformation-scalable-target-min-above-max-rejected|run#7|AssertionError|assert status == "CREATE_FAILED", (
Issue-Key: application-autoscaling.concurrent-register-scalable-target-updates-consistent|run#10|AssertionError|assert collided, (
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Application Auto Scaling accepts requests real AWS refuses and does not apply authorization, region, role or CloudFormation semantics (10 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterScalableTarget

## Description
Ten distinct root causes in the Application Auto Scaling emulator, all grounded in failing behavioral checks. Each is independent and must be closed separately.

Cause 1 of 10: DescribeScalableTargets accepts a ScalableDimension filter with no resource id.
AWS: Smithy (com-amazonaws-applicationautoscaling-anyscalefrontendservice) models DescribeScalableTargets with optional ResourceIds and optional ScalableDimension and the error ValidationException (client, httpError 400). Recorded behavior: a dimension filter sent without any resource id is refused with HTTP 400 ValidationException; the recorded message is, as an example, 'Scalable dimension cannot be provided without a resource ID.'
Emulator: returns HTTP 200 and a one-element ScalableTargets list, treating the dimension as a namespace-wide filter. The sibling Describe* operations do refuse it (with a different message - see the message issue filed for this service).
Why: a client that filters by dimension only silently gets a namespace-wide listing instead of an error, so it acts on targets it never asked about.
Fix: DescribeScalableTargets handler/input validation.
Traces: application-auto-scaling.describe-dimension-filter-without-a-resource-id-rejected

Cause 2 of 10: RegisterScalableTarget upsert carrying Tags on an already-registered target is accepted.
AWS: RegisterScalableTarget models Tags and ValidationException (400). Recorded behavior: re-registering an EXISTING target while passing Tags is refused with ValidationException; recorded message example: 'The scalable target that you tried to tag already exists. To update tags on an existing scalable target, use the TagResource API.' The bounds in that refused call are not applied.
Emulator: HTTP 200 with a ScalableTargetARN, so the upsert succeeds and the new bounds land.
Why: callers using register-with-tags as an idempotent create get a silent bounds mutation where AWS fails the call.
Fix: RegisterScalableTarget handler (existing-target + Tags precondition).
Traces: application-auto-scaling.register-upsert-carrying-tags-rejected-and-bounds-kept

Cause 3 of 10: Keyspaces (cassandra namespace) registration does not check that the table exists.
AWS: recorded behavior - registering cassandra:table:* for a table that does not exist inside an existing keyspace is refused with HTTP 400 ValidationException.
Emulator: HTTP 200 and a ScalableTargetARN is minted for the nonexistent table.
Why: typos in keyspace/table resource ids register phantom targets that can never scale anything.
Fix: RegisterScalableTarget handler, cassandra resource resolution.
Traces: application-auto-scaling.keyspaces-missing-table-in-an-existing-keyspace-rejected

Cause 4 of 10: Keyspaces ON_DEMAND table registration is accepted.
AWS: recorded behavior - a Keyspaces table in on-demand throughput mode has no provisioned capacity to scale and the registration is refused with HTTP 400 ValidationException.
Emulator: HTTP 200 and a ScalableTargetARN.
Why: a target is registered against capacity that does not exist; policies attached to it can never act.
Fix: RegisterScalableTarget handler, cassandra billing-mode precondition.
Traces: application-auto-scaling.keyspaces-on-demand-table-register-rejected

Cause 5 of 10: the accepted TargetValue range for DynamoDB utilization metrics is wrong (lower bound 20 instead of 10).
AWS: PutScalingPolicy's TargetTrackingScalingPolicyConfiguration.TargetValue is an unconstrained double in the model, so the range is service-enforced. Recorded behavior: for DynamoDBWriteCapacityUtilization / DynamoDBReadCapacityUtilization the valid range is 10.0 to 90.0 inclusive, and a value outside it is refused with HTTP 400 ValidationException whose message names the metric type, both bounds and the offered value (example: "For predefined metric type DynamoDBWriteCapacityUtilization, target value must be between '10.0' and '90.0', but was '90.5'.").
Emulator: enforces 20 to 90 - it refuses 90.5 (correct verdict) but with 'Member must be between 20 and 90 for metric DynamoDBWriteCapacityUtilization', and it REFUSES legitimate values below 20 (a TargetValue of 10 is rejected with ValidationException on the sibling check in the application-autoscaling bucket).
Why: valid policies in the 10-20 band are rejected outright, so a client cannot create the policy AWS accepts.
Fix: PutScalingPolicy validation, per-predefined-metric target-value range table.
Traces: application-auto-scaling.target-value-above-dynamodb-utilization-range-rejected

Cause 6 of 10: IAM authorization is not evaluated - calls that a deny/condition policy forbids succeed.
AWS: recorded behavior - with credentials whose policy denies the action (or allows it only under a resource-tag condition / a specific scalable-dimension condition, or omits the tagging permission), the call is refused with an access-denied error, not performed.
Emulator: every such call returns HTTP 200 (DeregisterScalableTarget returns an empty 200 body; RegisterScalableTarget returns a ScalableTargetARN).
Why: a client or test asserting least-privilege behavior sees permitted where AWS denies, so IAM regressions are invisible and the emulator mutates state it must not touch.
Fix: request authorization layer for this service's operations (tag conditions, dimension conditions, TagResource permission on register-with-tags).
Traces: application-auto-scaling.deregister-denied-by-a-resource-tag-condition, application-auto-scaling.register-outside-the-allowed-scalable-dimension-denied, application-auto-scaling.register-with-tags-denied-without-tag-resource-permission

Cause 7 of 10: RoleARN is echoed back for a service-linked namespace instead of the service-linked role.
AWS: ScalableTarget.RoleARN is a required output member (Smithy: ScalableTarget.RoleARN required). Recorded behavior: for the dynamodb namespace the target always reports the DynamoDB service-linked role, whatever RoleARN the caller supplied.
Emulator: DescribeScalableTargets reports the customer-supplied role ARN.
Why: a client reading RoleARN to discover which principal scales the resource is told a role that has no effect, and IAM-wiring assertions pass on a role AWS ignores.
Fix: RegisterScalableTarget handler / scalable-target state (service-linked-role resolution per namespace).
Traces: application-auto-scaling.role-arn-is-ignored-for-a-service-linked-namespace

Cause 8 of 10: scalable targets are not scoped per region.
AWS: recorded behavior - the same ResourceId/ScalableDimension registered in two regions are two independent targets; deregistering in one region leaves the other describable with its own bounds.
Emulator: after the pair of registrations and one deregistration, the other region's DescribeScalableTargets reads back [] where the registered bounds (1, 5) must still be listed.
Why: cross-region state bleeding destroys targets a client still owns, and multi-region deployments observe deletes they never issued.
Fix: scalable-target state keying (region must be part of the key).
Traces: application-auto-scaling.deregister-in-one-region-keeps-the-same-named-target-in-another

Cause 9 of 10: CloudFormation stacks declaring AWS::ApplicationAutoScaling::ScalableTarget / ::ScalingPolicy create nothing and never fail.
AWS: recorded behavior - a stack with those resources registers the scalable target and the policy (describable through Application Auto Scaling afterwards), and a stack declaring MinCapacity above MaxCapacity fails provisioning and ends CREATE_FAILED.
Emulator: both stacks reach CREATE_COMPLETE and no scalable target is registered for the stack's table.
Why: infrastructure-as-code paths appear to deploy autoscaling that does not exist, and an invalid template is reported as a successful deploy. Same shape as the already-open issue 20261022T171541Z-codedeploy-cloudformation-stacks-declaring-aws-codedeploy-application-deploymentgroup-reach (CodeDeploy resource types); this is a different resource type and not covered by it.
Fix: CloudFormation resource-provider dispatch for the AWS::ApplicationAutoScaling::* types (must call this service's handlers and surface their ValidationException as a failed stack).
Traces: application-auto-scaling.deploy-cloudformation-scalable-target-and-policy-scale-the-table, application-auto-scaling.deploy-cloudformation-scalable-target-min-above-max-rejected

Cause 10 of 10: ConcurrentUpdateException is never produced for concurrent RegisterScalableTarget calls on one target.
AWS: Smithy: RegisterScalableTarget declares ConcurrentUpdateException (server, httpError 500). Recorded behavior: ten concurrent registrations against the same target make at least one caller observe ConcurrentUpdateException while the final state stays consistent.
Emulator: all ten calls return HTTP 200; the conflict error is never raised.
Why: clients that implement the documented retry-on-ConcurrentUpdateException path never exercise it against the emulator, so that path ships untested.
Fix: RegisterScalableTarget handler / per-target update serialization.
Traces: application-auto-scaling.concurrent-register-scalable-target-updates-consistent

## Evidence
Replay report (suite simparity-behavioral-aws-v1), per failing check:
- describe-dimension-filter-without-a-resource-id-rejected: 'expected the call to be refused, but it succeeded: {\'ScalableTargets\': 1}' on describe_scalable_targets(ServiceNamespace='dynamodb', ScalableDimension='dynamodb:table:ReadCapacityUnits').
- register-upsert-carrying-tags-rejected-and-bounds-kept: 'expected the call to be refused, but it succeeded: {\'ScalableTargetARN\': ..., HTTPStatusCode: 200}' for register_scalable_target(..., MinCapacity=1, MaxCapacity=7, Tags={'team':'beta'}) on an already-registered target.
- keyspaces-missing-table-in-an-existing-keyspace-rejected / keyspaces-on-demand-table-register-rejected: both 'expected the call to be refused, but it succeeded' with HTTP 200 + ScalableTargetARN.
- target-value-above-dynamodb-utilization-range-rejected: expected ValidationException message "For predefined metric type DynamoDBWriteCapacityUtilization, target value must be between '10.0' and '90.0', but was '90.5'."; got "Value '90.5' at 'targetTrackingScalingPolicyConfiguration.targetValue' failed to satisfy constraint: Member must be between 20 and 90 for metric DynamoDBWriteCapacityUtilization". The 20-floor is confirmed by application-autoscaling.not-scaled-activity-records-already-at-max-capacity, where PutScalingPolicy with TargetValue 10 raised ValidationException instead of succeeding.
- the three IAM checks: 'expected the call to be refused, but it succeeded: {... HTTPStatusCode: 200 ...}'.
- role-arn-is-ignored-for-a-service-linked-namespace: 'the dynamodb:table:ReadCapacityUnits target ... reports RoleARN ending sim2real-aas-custom-...; a dynamodb target always uses the DynamoDB service-linked role'.
- deregister-in-one-region-keeps-the-same-named-target-in-another: 'the us-east-1 target reads back as [], expected [(1, 5)]'.
- deploy-cloudformation-scalable-target-and-policy-scale-the-table: 'no scalable target is registered for the stack's table'; deploy-cloudformation-scalable-target-min-above-max-rejected: "a stack declaring MinCapacity 9 above MaxCapacity 8 ended 'CREATE_COMPLETE', expected CREATE_FAILED".
- concurrent-register-scalable-target-updates-consistent: 'none of the 10 concurrent RegisterScalableTarget calls ever hit ConcurrentUpdateException -- the race was never exercised'.
Handler surface: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/ (register_scalable_target.rs, describe_scalable_targets.rs, put_scaling_policy.rs).

## Steps to Reproduce
Boot the emulator from this checkout and replay the behavioral suite, or drive the listed checks directly with botocore against the emulator endpoint:
1. describe_scalable_targets(ServiceNamespace='dynamodb', ScalableDimension='dynamodb:table:ReadCapacityUnits') -> expect ValidationException, observe HTTP 200.
2. register_scalable_target on an existing target with Tags -> expect ValidationException, observe 200.
3. register_scalable_target for ServiceNamespace='cassandra' with a nonexistent table, and for an on-demand table -> expect ValidationException, observe 200.
4. put_scaling_policy TargetTrackingScaling with TargetValue 10.0 on dynamodb:table:ReadCapacityUnits -> expect success, observe ValidationException; with 90.5 -> expect the AWS range message naming 10.0/90.0.
5. assume a role whose policy denies the call (resource-tag condition / dimension condition / no TagResource permission) and call deregister_scalable_target / register_scalable_target -> expect access denied, observe 200.
6. register in us-west-2 and us-east-1, deregister in us-west-2, describe in us-east-1 -> expect the target, observe [].
7. create a CloudFormation stack with AWS::ApplicationAutoScaling::ScalableTarget (+ ScalingPolicy), then with MinCapacity>MaxCapacity -> expect registration / CREATE_FAILED, observe nothing registered / CREATE_COMPLETE.
8. fire 10 concurrent register_scalable_target calls on one target -> expect at least one ConcurrentUpdateException.
