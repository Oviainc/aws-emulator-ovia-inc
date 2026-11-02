Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.customized-metric-rejected-on-a-dynamodb-dimension application-autoscaling.predefined-metric-of-the-other-dimension-rejected application-autoscaling.target-value-above-dynamodb-utilization-range-rejected application-autoscaling.second-target-tracking-policy-on-the-same-metric-rejected application-autoscaling.deleted-table-leaves-its-scalable-target-registered application-autoscaling.register-scalable-target-nonexistent-resource-rejected application-autoscaling.register-below-namespace-minimum-rejected application-autoscaling.table-that-exists-only-in-another-region-register-rejected application-autoscaling.target-tracking-policy-manages-cloudwatch-alarms application-autoscaling.target-tracking-alarm-breach-triggers-scaling-activity application-autoscaling.deregister-scalable-target-cascades-policies-and-actions application-autoscaling.concurrent-upserts-of-one-policy-name-leave-one-policy-and-one-alarm-set application-autoscaling.delete-scaling-policy-never-created-rejected application-autoscaling.register-scalable-target-basic application-autoscaling.put-scaling-policy-without-target-rejected application-autoscaling.put-scaling-policy-missing-configuration-rejected application-autoscaling.deploy-cloudformation-scalable-target-and-policy-scale-the-table application-autoscaling.deploy-cloudformation-scalable-target-min-above-max-rejected application-autoscaling.put-scheduled-action-inverted-bounds-rejected application-autoscaling.put-scheduled-action-non-canonical-timezone-rejected application-autoscaling.put-scheduled-action-on-unregistered-dimension-rejected application-autoscaling.put-scheduled-action-start-after-end-rejected application-autoscaling.put-scheduled-action-without-schedule-rejected-on-create application-autoscaling.aws-prefixed-tag-key-rejected-on-scalable-target application-autoscaling.concurrent-register-scalable-target-updates-consistent
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.customized-metric-rejected-on-a-dynamodb-dimension|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling.predefined-metric-of-the-other-dimension-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling.target-value-above-dynamodb-utilization-range-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling.second-target-tracking-policy-on-the-same-metric-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling.deleted-table-leaves-its-scalable-target-registered|_error#16|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling.register-scalable-target-nonexistent-resource-rejected|_client_error#4|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: application-autoscaling.register-below-namespace-minimum-rejected|_error#5|AssertionError|raise AssertionError(
Issue-Key: application-autoscaling.table-that-exists-only-in-another-region-register-rejected|_error#9|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: application-autoscaling.target-tracking-policy-manages-cloudwatch-alarms|run#11|AssertionError|assert alarm_names, "target tracking policy did not create any CloudWatch alarms"
Issue-Key: application-autoscaling.target-tracking-alarm-breach-triggers-scaling-activity|run#12|AssertionError|assert alarm_names, "target tracking policy did not create any CloudWatch alarms"
Issue-Key: application-autoscaling.deregister-scalable-target-cascades-policies-and-actions|run#11|AssertionError|assert alarm_names, "target tracking policy did not create any CloudWatch alarms"
Issue-Key: application-autoscaling.concurrent-upserts-of-one-policy-name-leave-one-policy-and-one-alarm-set|run#23|AssertionError|assert len(owned) == 4, f'the policy owns {len(owned)} alarms, expected 4'
Issue-Key: application-autoscaling.delete-scaling-policy-never-created-rejected|run#14|AssertionError|assert len(live) == len(alarms) == 4, (
Issue-Key: application-autoscaling.register-scalable-target-basic|run#17|AssertionError|assert "AWSServiceRoleForApplicationAutoScaling_DynamoDBTable" in target["RoleARN"], target["RoleARN"]
Issue-Key: application-autoscaling.put-scaling-policy-without-target-rejected|run#10|AssertionError|assert error.get("Message") == (
Issue-Key: application-autoscaling.put-scaling-policy-missing-configuration-rejected|run#11|AssertionError|assert error.get("Message") == "TargetTrackingScaling policy configuration must be specified", (
Issue-Key: application-autoscaling.deploy-cloudformation-scalable-target-and-policy-scale-the-table|run#13|AssertionError|assert table, f"stack outputs carry no TableName: {sorted(outputs)!r}"
Issue-Key: application-autoscaling.deploy-cloudformation-scalable-target-min-above-max-rejected|run#7|AssertionError|assert status == "CREATE_FAILED", (
Issue-Key: application-autoscaling.put-scheduled-action-inverted-bounds-rejected|run#10|AssertionError|assert code == "ValidationException", (
Issue-Key: application-autoscaling.put-scheduled-action-non-canonical-timezone-rejected|run#12|AssertionError|assert error['Code'] == 'ValidationException', (
Issue-Key: application-autoscaling.put-scheduled-action-on-unregistered-dimension-rejected|run#12|AssertionError|assert error['Code'] == 'ObjectNotFoundException', (
Issue-Key: application-autoscaling.put-scheduled-action-start-after-end-rejected|run#14|AssertionError|assert error['Code'] == 'ValidationException', (
Issue-Key: application-autoscaling.put-scheduled-action-without-schedule-rejected-on-create|run#13|AssertionError|assert error['Code'] == 'ValidationException', (
Issue-Key: application-autoscaling.aws-prefixed-tag-key-rejected-on-scalable-target|run#11|AssertionError|assert error['Code'] == 'ValidationException', (
Issue-Key: application-autoscaling.concurrent-register-scalable-target-updates-consistent|run#10|AssertionError|assert collided, (
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Application Auto Scaling control plane diverges in 9 ways: unvalidated PutScalingPolicy/RegisterScalableTarget, no CloudWatch alarms, wrong service-linked RoleARN, error-message rules, CFN resources, scheduled-action and tagging negative paths, no ConcurrentUpdateException

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution

Causes 1–5 and 7–9 are satisfied in the Application Auto Scaling crate.

Already in place when this fix landed (and re-verified here): the registration's resolution
of `ResourceId` against the owning service's store and region, the CloudWatch alarm set a
target-tracking policy owns (with its cascade on `DeleteScalingPolicy` and
`DeregisterScalableTarget`), the two request-derived refusal messages (cause 5),
`PutScheduledAction` including its negative paths (cause 7) and the reserved `aws:` tag
namespace on `TagResource`/`UntagResource` (cause 8).

Added by this fix: the target-tracking metric rules of cause 1 — a predefined metric is
accepted only on a scalable dimension its own service publishes it for, with a target value
inside that metric's published window, a customized metric is refused where the dimension IS
a provisioned read/write throughput, and one metric takes one target-tracking policy per
target; the per-dimension capacity floor of cause 2; the suffixed, per-namespace
service-linked role of cause 4; and the modelled `ConcurrentUpdateException` of cause 9,
raised when one registration's update of a target overlaps another's.

The two message examples the Description itself flags as constant recorded strings with no
derivable rule are left as they are.

Cause 6 is DECLINED here and is NOT fixed: `AWS::ApplicationAutoScaling::ScalableTarget`
/ `ScalingPolicy` provisioning and stack `Outputs` are a CloudFormation capability — that
emulator provisions no resource of ANY type and resolves no `Outputs` for any template
(see `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_stacks.rs`,
which documents it on the `Outputs` member) — and CloudFormation carries its own issue for
it. The two traces `application-auto-scaling.deploy-cloudformation-scalable-target-and-policy-scale-the-table`
and `application-auto-scaling.deploy-cloudformation-scalable-target-min-above-max-rejected`
therefore still fail, on the CloudFormation side; what this service supplies is the refusal
such a provider must propagate into `CREATE_FAILED`.

## Operation
PutScalingPolicy

## Description
Nine distinct root causes in the Application Auto Scaling emulator, all grounded in failing behavioral checks of the `application-auto-scaling` suite. Smithy model consulted: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/smithy-model/com-amazonaws-applicationautoscaling-anyscalefrontendservice.json.

Cause 1 of 9: PutScalingPolicy accepts target-tracking configurations AWS refuses.
AWS: Smithy: PutScalingPolicy declares errors ValidationException (client, httpError 400) and ObjectNotFoundException (client, 400); its input binds ServiceNamespace, ScalableDimension and TargetTrackingScalingPolicyConfiguration. Real AWS validates the policy configuration against the namespace/dimension: a CustomizedMetricSpecification is refused on a DynamoDB dimension, a PredefinedMetricType belonging to the OTHER dimension (read vs write) is refused, a TargetValue outside the DynamoDB utilisation range is refused, and a SECOND TargetTrackingScaling policy on the same metric for the same target is refused. All four are 400 ValidationException in the recordings.
Emulator: every one of the four calls returns HTTP 200 with a PolicyARN and `Alarms: []` (e.g. `{'PolicyARN': 'arn:aws:autoscaling:...:scalingPolicy:...', 'Alarms': []}`), so a policy real AWS never creates is created.
Why: clients validating their configuration against the emulator get a green light for policies AWS rejects at deploy time; a second conflicting policy silently coexists, so the capacity behaviour a user tests locally cannot occur in AWS.
Fix: PutScalingPolicy's validation in the operation handler (ops/put_scaling_policy.rs) — namespace/dimension/metric compatibility, target-value range per predefined metric, and the one-policy-per-metric uniqueness rule.
Traces: application-auto-scaling.customized-metric-rejected-on-a-dynamodb-dimension, application-auto-scaling.predefined-metric-of-the-other-dimension-rejected, application-auto-scaling.target-value-above-dynamodb-utilization-range-rejected, application-auto-scaling.second-target-tracking-policy-on-the-same-metric-rejected.

Cause 2 of 9: RegisterScalableTarget does not check that the scalable resource exists, is in this region, or is within the namespace's capacity range.
AWS: Smithy: RegisterScalableTarget declares ValidationException (client, 400); ResourceId/ScalableDimension/ServiceNamespace are required. Real AWS resolves ResourceId against the real resource and refuses registration (400 ValidationException) when the DynamoDB table does not exist, has been deleted, exists only in ANOTHER region, or when MinCapacity is below the namespace minimum.
Emulator: all four return HTTP 200 with a ScalableTargetARN (e.g. `{'ScalableTargetARN': 'arn:aws:application-autoscaling:us-west-2:000000000000:scalable-target/...'}`).
Why: accepting input AWS refuses is the strongest over-permissiveness signal: a client registers a target against a typo'd or deleted table and sees success, then the same template fails in AWS.
Fix: RegisterScalableTarget handler validation — resolve the resource in the request's region through the owning service's state before persisting the target, and range-check Min/MaxCapacity against the namespace's documented limits.
Traces: application-auto-scaling.deleted-table-leaves-its-scalable-target-registered, application-auto-scaling.register-scalable-target-nonexistent-resource-rejected, application-auto-scaling.register-below-namespace-minimum-rejected, application-auto-scaling.table-that-exists-only-in-another-region-register-rejected.

Cause 3 of 9: target-tracking policies mint no CloudWatch alarms, so nothing downstream of a policy exists.
AWS: Smithy: PutScalingPolicyResponse.Alarms (list of Alarm) is returned by the operation. Real AWS provisions the alarm set for a TargetTrackingScaling policy (recorded names AlarmHigh/AlarmLow/ProvisionedCapacityHigh/ProvisionedCapacityLow), returns them in Alarms, lists them in DescribeScalingPolicies, deletes them when the policy or target is removed, and keeps exactly one set across concurrent upserts of one policy name.
Emulator: `Alarms` is always empty and CloudWatch has no alarms for the policy — checks report 'target tracking policy did not create any CloudWatch alarms', 'the policy owns 0 alarms, expected 4' and '0 of the real policy's 0 alarms survive the refused delete'.
Why: no alarm means no alarm-driven scaling at all, and clients that read Alarms (to wire dashboards, or to assert cleanup on deregister) see an empty list where AWS always returns the provisioned set.
Fix: PutScalingPolicy state/side-effect path — materialise the alarm set in the CloudWatch emulator's state and project it into PutScalingPolicy/DescribeScalingPolicies, with cascade on DeleteScalingPolicy and DeregisterScalableTarget.
Traces: application-auto-scaling.target-tracking-policy-manages-cloudwatch-alarms, application-auto-scaling.target-tracking-alarm-breach-triggers-scaling-activity, application-auto-scaling.deregister-scalable-target-cascades-policies-and-actions, application-auto-scaling.concurrent-upserts-of-one-policy-name-leave-one-policy-and-one-alarm-set, application-auto-scaling.delete-scaling-policy-never-created-rejected.

Cause 4 of 9: the auto-assigned service-linked RoleARN omits the per-resource-type suffix.
AWS: when RoleARN is omitted, AWS assigns the namespace's service-linked role; for DynamoDB tables the role name is AWSServiceRoleForApplicationAutoScaling_DynamoDBTable (recorded behavior; the ARN is .../aws-service-role/dynamodb.application-autoscaling.amazonaws.com/AWSServiceRoleForApplicationAutoScaling_DynamoDBTable).
Emulator: DescribeScalableTargets returns `arn:aws:iam::000000000000:role/aws-service-role/dynamodb.application-autoscaling.amazonaws.com/AWSServiceRoleForApplicationAutoScaling` — the generic role name with no `_<ResourceType>` suffix.
Why: clients that assert on, or pass through, the service-linked role (IAM policy generation, GetRole lookups) resolve a role name that does not exist in AWS.
Fix: the RoleARN defaulting in the RegisterScalableTarget handler — derive the suffixed service-linked role name per namespace/dimension.
Traces: application-auto-scaling.register-scalable-target-basic.

Cause 5 of 9: rejection messages are emulator-invented where the message rule is known.
AWS: ObjectNotFoundException from PutScalingPolicy carries a message built from the request: 'No scalable target registered for service namespace: <ns>, resource ID: <resourceId>, scalable dimension: <dim>'. ValidationException for a TargetTrackingScaling policy with no configuration names the missing member: 'TargetTrackingScaling policy configuration must be specified'.
Emulator: the codes are right but the wording is not: 'No scalable target FOUND for service namespace: ...' and 'A TargetTrackingScaling policy requires TargetTrackingScalingPolicyConfiguration'.
Why: clients that branch on or surface the message (and tests asserting it) diverge even though the error class matches.
Fix: the message templates in those operations' validation/error construction. Note: the same checks also show different wording for the RegisterScalableTarget min-above-max message ('MinCapacity (9) must be less than or equal to MaxCapacity (7)' vs the recorded constant 'Maximum capacity cannot be less than minimum capacity') and for the DescribeScalableTargets invalid-token message; those two are CONSTANT recorded strings with no derivable rule, so they are reported here as examples only and are not claimed as requirements.
Traces: application-auto-scaling.put-scaling-policy-without-target-rejected, application-auto-scaling.put-scaling-policy-missing-configuration-rejected.

Cause 6 of 9: CloudFormation stacks containing AWS::ApplicationAutoScaling resources neither provision nor validate them.
AWS: a stack declaring a ScalableTarget plus a ScalingPolicy reaches CREATE_COMPLETE with its declared Outputs populated (the created table name) and the target actually registered; a stack declaring MinCapacity above MaxCapacity ends CREATE_FAILED because the underlying RegisterScalableTarget call is refused (recorded behavior).
Emulator: the first stack's Outputs are empty ('stack outputs carry no TableName: []'), and the invalid stack ends 'CREATE_COMPLETE' instead of CREATE_FAILED.
Why: infrastructure-as-code users get a stack that reports success while nothing was registered, and a malformed template that AWS would roll back deploys clean.
Fix: the CloudFormation resource provider for AWS::ApplicationAutoScaling::ScalableTarget / ScalingPolicy (resource provisioning plus Outputs resolution), which must call through to the Application Auto Scaling state and propagate its refusal into stack status.
Traces: application-auto-scaling.deploy-cloudformation-scalable-target-and-policy-scale-the-table, application-auto-scaling.deploy-cloudformation-scalable-target-min-above-max-rejected.

Cause 7 of 9: every PutScheduledAction negative path answers NotImplemented instead of the modeled rejection.
AWS: Smithy: PutScheduledAction declares ValidationException (client, 400) and ObjectNotFoundException (client, 400); ScheduledActionName carries pattern ^(?!((^[ ]+.*)|(.*([\u0000-\u001f]|[\u007f-\u009f]|[:/|])+.*)|(.*[ ]+$))).+$ . AWS answers 400 ValidationException for an inverted ScalableTargetAction (MinCapacity above MaxCapacity), a non-canonical Timezone, StartTime after EndTime and a create with no Schedule, and 400 ObjectNotFoundException for an action on a dimension that is not registered.
Emulator: all five return the scaffold answer — error code 'NotImplemented', message 'operation PutScheduledAction is scaffolded but not implemented in this emulator'.
Why: a client cannot distinguish a bad request from an unsupported service; retry/validation logic keyed on ValidationException/ObjectNotFoundException never fires.
Fix: implement PutScheduledAction (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/put_scheduled_action.rs) INCLUDING these validations. The stub itself is already filed as a separate 'unimplemented scaffold stub' work item for PutScheduledAction; this cause records the negative-path contract that must hold once it exists, and these checks must be re-replayed.
Traces: application-auto-scaling.put-scheduled-action-inverted-bounds-rejected, application-auto-scaling.put-scheduled-action-non-canonical-timezone-rejected, application-auto-scaling.put-scheduled-action-on-unregistered-dimension-rejected, application-auto-scaling.put-scheduled-action-start-after-end-rejected, application-auto-scaling.put-scheduled-action-without-schedule-rejected-on-create.

Cause 8 of 9: the reserved aws: tag namespace is not enforced on scalable targets.
AWS: Smithy: TagResource declares ValidationException (client, 400) and requires ResourceARN (pattern ^arn:.+:application-autoscaling:.+:[0-9]+:scalable-target\/[a-zA-Z0-9-]+$) plus Tags. AWS refuses a batch containing an aws:-prefixed key with 400 ValidationException, atomically (no key from the batch lands), and refuses UntagResource of an aws:-prefixed key the same way.
Emulator: TagResource answers code 'NotImplemented' ('operation TagResource is scaffolded but not implemented in this emulator'), so the reserved-namespace rule is unobservable.
Why: clients relying on the reserved prefix being rejected (and on the refusal being atomic) get an unusable tagging surface and the wrong error class.
Fix: implement TagResource/UntagResource for scalable targets with the aws:-prefix rule and atomic batch semantics; the bare stub is already filed as its own 'unimplemented scaffold stub' item for TagResource, this cause records the rule to satisfy.
Traces: application-auto-scaling.aws-prefixed-tag-key-rejected-on-scalable-target.

Cause 9 of 9: ConcurrentUpdateException is never surfaced.
AWS: Smithy: RegisterScalableTarget declares ConcurrentUpdateException (server, httpError 500). Concurrent RegisterScalableTarget calls against one target make AWS fail at least one of them with ConcurrentUpdateException while the surviving state stays self-consistent.
Emulator: ten concurrent RegisterScalableTarget calls all succeed — 'none of the 10 concurrent RegisterScalableTarget calls ever hit ConcurrentUpdateException'.
Why: clients with retry-on-ConcurrentUpdateException paths never exercise them against the emulator, so a code path that is mandatory in AWS stays untested.
Fix: the optimistic-concurrency guard in the scalable-target state layer, raising the modeled ConcurrentUpdateException (500) when a concurrent mutation of one target is detected.
Traces: application-auto-scaling.concurrent-register-scalable-target-updates-consistent.

## Evidence
Suite: simparity-behavioral-aws-v1, 8/145 passed. Representative emulator responses:
- PutScalingPolicy with a customized metric on a DynamoDB dimension: HTTP 200 `{'PolicyARN': 'arn:aws:autoscaling:us-west-2:000000000000:scalingPolicy:...', 'Alarms': []}` — expected 400 ValidationException.
- RegisterScalableTarget for a nonexistent/deleted/other-region table or a sub-minimum MinCapacity: HTTP 200 `{'ScalableTargetARN': 'arn:aws:application-autoscaling:us-west-2:000000000000:scalable-target/...'}` — expected 400 ValidationException.
- Target-tracking policy alarms: 'the policy minted alarms for [], expected [AlarmHigh, AlarmLow, ProvisionedCapacityHigh, ProvisionedCapacityLow]' / 'the policy owns 0 alarms, expected 4'.
- DescribeScalableTargets RoleARN: got '...:role/aws-service-role/dynamodb.application-autoscaling.amazonaws.com/AWSServiceRoleForApplicationAutoScaling', expected the name to contain 'AWSServiceRoleForApplicationAutoScaling_DynamoDBTable'.
- Messages: 'No scalable target found for service namespace: dynamodb, resource ID: table/..., scalable dimension: dynamodb:table:WriteCapacityUnits' vs AWS 'No scalable target registered for service namespace: ..., resource ID: ..., scalable dimension: ...'; 'A TargetTrackingScaling policy requires TargetTrackingScalingPolicyConfiguration' vs 'TargetTrackingScaling policy configuration must be specified'.
- CloudFormation: 'stack outputs carry no TableName: []'; 'a stack declaring MinCapacity 9 above MaxCapacity 8 ended CREATE_COMPLETE, expected CREATE_FAILED'.
- PutScheduledAction negative paths: ClientError (NotImplemented) 'operation PutScheduledAction is scaffolded but not implemented in this emulator' where ValidationException / ObjectNotFoundException is modeled.
- TagResource: ClientError (NotImplemented) where ValidationException is modeled for an aws:-prefixed key.
- Concurrency: 'none of the 10 concurrent RegisterScalableTarget calls ever hit ConcurrentUpdateException'.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (run_behavioral_suite).
2. Re-replay the checks listed in trace_ids; each one exercises one of the nine causes against the application-autoscaling endpoint with a DynamoDB table as the scalable resource.
3. Minimal manual repro for cause 1/2: create a PROVISIONED DynamoDB table, then call RegisterScalableTarget for `table/<a-name-that-does-not-exist>` (expect 400 ValidationException, observe 200 + ScalableTargetARN), and PutScalingPolicy with a CustomizedMetricSpecification on dynamodb:table:ReadCapacityUnits (expect 400 ValidationException, observe 200 + empty Alarms).
