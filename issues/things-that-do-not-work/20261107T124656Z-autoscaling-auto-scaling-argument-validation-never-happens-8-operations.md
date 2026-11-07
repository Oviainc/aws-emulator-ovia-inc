Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.attach-instance-wrong-availability-zone-rejected autoscaling.attach-instances-exceeding-max-size-rejected autoscaling.attach-nonexistent-instance-fails autoscaling.attach-load-balancers-nonexistent-rejected autoscaling.describe-invalid-next-token-multi-endpoint-fails autoscaling.metrics-collection-invalid-granularity-rejected autoscaling.put-notification-configuration-invalid-topic-arn-rejected autoscaling.put-scaling-policy-invalid-adjustment-type-fails autoscaling.put-scaling-policy-missing-required-param-rejected autoscaling.put-scaling-policy-negative-cooldown-rejected autoscaling.put-scheduled-action-malformed-recurrence-rejected autoscaling.set-instance-health-inside-the-grace-period-rejected-and-the-instance-is-not-replaced
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.attach-instance-wrong-availability-zone-rejected|run#14|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code}"
Issue-Key: autoscaling.attach-instances-exceeding-max-size-rejected|run#17|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code!r}"
Issue-Key: autoscaling.attach-nonexistent-instance-fails|run#13|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code}"
Issue-Key: autoscaling.attach-load-balancers-nonexistent-rejected|run#8|AssertionError|assert code == "ValidationError", f"expected ValidationError attaching a nonexistent CLB, got {code}"
Issue-Key: autoscaling.describe-invalid-next-token-multi-endpoint-fails|run#5|AssertionError|assert code_instances == "InvalidNextToken", f"DescribeAutoScalingInstances: expected InvalidNextToken, got {code_instances}"
Issue-Key: autoscaling.metrics-collection-invalid-granularity-rejected|run#12|AssertionError|assert code == "ValidationError", (
Issue-Key: autoscaling.put-notification-configuration-invalid-topic-arn-rejected|run#6|AssertionError|assert code == "ValidationError", f"expected ValidationError for a malformed TopicARN, got {code!r}"
Issue-Key: autoscaling.put-scaling-policy-invalid-adjustment-type-fails|run#11|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code}"
Issue-Key: autoscaling.put-scaling-policy-missing-required-param-rejected|run#5|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code}"
Issue-Key: autoscaling.put-scaling-policy-negative-cooldown-rejected|run#13|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code!r}"
Issue-Key: autoscaling.put-scheduled-action-malformed-recurrence-rejected|run#8|AssertionError|assert code == "ValidationError", f"expected ValidationError, got {code}"
Issue-Key: autoscaling.set-instance-health-inside-the-grace-period-rejected-and-the-instance-is-not-replaced|run#12|AssertionError|assert code == "ValidationError", (
Issue-Commit: ee9893ed79b0abeb742411b16639d26129234894

# Auto Scaling argument validation never happens: 8 operations answer 501 NotImplemented instead of AWS's ValidationError/InvalidNextToken refusals (8 causes)

> _Note: this issue was identified against the code base at commit `ee9893ed79b0abeb742411b16639d26129234894`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachInstances (also AttachLoadBalancers, DescribeAutoScalingInstances, EnableMetricsCollection, PutNotificationConfiguration, PutScalingPolicy, PutScheduledUpdateGroupAction, SetInstanceHealth)

## Description
Twelve behavioral checks that assert Auto Scaling's *negative* contract (the error code, HTTP status and message AWS returns for a malformed or out-of-state request) all receive the emulator's scaffold answer `NotImplemented` ("operation <X> is scaffolded but not implemented in this emulator", HTTP 501) instead of the modelled client refusal. Each of the open "Implement autoscaling <Op>: unimplemented scaffold stub" issues (20261104T042502Z-autoscaling-attachinstances-stub.md, 20261107T081710Z-autoscaling-attachloadbalancers-stub.md, 20261107T081710Z-autoscaling-describeautoscalinginstances-stub.md, 20261104T042502Z-autoscaling-enablemetricscollection-stub.md, 20261104T042502Z-autoscaling-putnotificationconfiguration-stub.md, 20261107T081710Z-autoscaling-putscalingpolicy-stub.md, 20261104T042502Z-autoscaling-putscheduledupdategroupaction-stub.md, 20261107T081710Z-autoscaling-setinstancehealth-stub.md) asks only for the operation to exist; none of them states the validation rules and refusal wording below, so a handler that implements only the happy path closes those issues while every check listed here still fails. The open issues "Auto Scaling argument-validation refusals use the wrong error code and wording, or are skipped entirely" (20261107T071147Z) and "Auto Scaling client-error refusals diverge" (20261029T211352Z) describe the same family but do not carry any of these checks' replay keys. Protocol note for every cause: autoscaling is awsQuery, so a client refusal is HTTP 400 with <Error><Type>Sender</Type><Code>...</Code><Message>...</Message></Error>; the emulator's 501/NotImplemented is a server-class answer, so botocore does not even classify it as a client error.

Cause 1 of 8 — AttachInstances performs none of its three admission checks.
AWS: Smithy declares AttachInstances with only ResourceContentionFault/ServiceLinkedRoleFailure, so the three refusals below are recorded behavior only; all three are HTTP 400, Code=ValidationError, with these message templates (fixed text exact; bracketed parts taken from the request and the group's stored state): (a) instance in an AZ the group does not cover — "The list of availability zones [<group AvailabilityZones, comma-separated>] for the group <AutoScalingGroupName> does not contain the zone occupied by the instance <InstanceId> [<instance AZ>]"; (b) attaching more instances than MaxSize allows — "AutoScalingGroup <AutoScalingGroupName> has min-size=<MinSize>, max-size=<MaxSize>, and desired-size=<DesiredCapacity>. To attach <len(InstanceIds)> instance, please update the AutoScalingGroup sizes appropriately."; (c) an instance id that does not exist — "Instance [<InstanceId>] is invalid.". In all three cases the group's Instances list is unchanged.
Emulator: every AttachInstances call answers NotImplemented (501), so the code the client sees is `NotImplemented`, never `ValidationError`, and no message is produced.
Why: a client cannot distinguish "your attach was refused because of the AZ/size/instance id" from "this service is down"; retry logic keyed on ValidationError (non-retryable) treats a 501 as retryable and loops, and callers that attach then assume membership get no signal at all.
Fix: the AttachInstances handler and its validation, in services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/attach_instances.rs — resolve the instance in EC2 state, compare its AZ with the group's AvailabilityZones, and check the resulting count against MaxSize before mutating group state.
Traces: autoscaling.attach-instance-wrong-availability-zone-rejected, autoscaling.attach-instances-exceeding-max-size-rejected, autoscaling.attach-nonexistent-instance-fails.

Cause 2 of 8 — AttachLoadBalancers does not refuse a Classic Load Balancer that does not exist.
AWS: recorded behavior only (Smithy declares only InstanceRefreshInProgressFault/ResourceContentionFault/ServiceLinkedRoleFailure). Attaching a CLB name that does not exist is HTTP 400, Code=ValidationError, Message exactly "Provided Load Balancers may not be valid. Please ensure they exist and try again." — the whole sentence is fixed text, no request values are interpolated — and DescribeLoadBalancers for the group stays empty.
Emulator: NotImplemented (501) for the attach call; the check reports `got NotImplemented` where ValidationError was required.
Why: a caller wiring an ASG to a CLB gets a server error for its own typo, and cannot tell a bad name from an outage.
Fix: the AttachLoadBalancers handler and its validation, services/.../src/ops/attach_load_balancers.rs — look the names up in Classic ELB state and refuse the whole call before recording any attachment.
Traces: autoscaling.attach-load-balancers-nonexistent-rejected.

Cause 3 of 8 — DescribeAutoScalingInstances does not reject a malformed NextToken.
AWS: Smithy: DescribeAutoScalingInstances declares error InvalidNextToken, errorKind=client, httpError 400, awsQueryError code "InvalidNextToken". The message is "The token '<NextToken>' is invalid" — fixed text with the rejected token value from the request.
Emulator: NotImplemented (501); the check reports "expected InvalidNextToken, got NotImplemented".
Why: a paginator handed a stale or corrupted token sees a 501 instead of the modelled InvalidNextToken it is written to catch, so it cannot restart the page walk.
Fix: the DescribeAutoScalingInstances handler and its pagination/validation, services/.../src/ops/describe_auto_scaling_instances.rs.
Traces: autoscaling.describe-invalid-next-token-multi-endpoint-fails.

Cause 4 of 8 — EnableMetricsCollection accepts an unsupported Granularity.
AWS: recorded behavior only (Smithy declares only ResourceContentionFault; Granularity is a free-form XmlStringMaxLen255 whose single valid value is documented, not modelled). Any Granularity other than "1Minute" is HTTP 400, Code=ValidationError, Message exactly "Valid metrics granularity type is: [1Minute]." (entirely fixed text), and the group's EnabledMetrics stays empty; the same call with "1Minute" succeeds and persists.
Emulator: NotImplemented (501); the check reports `got 'NotImplemented'`.
Why: a client that mistypes the granularity is told the service is unimplemented rather than told the one accepted value, and cannot learn the constraint from the error.
Fix: the EnableMetricsCollection handler and its validation, services/.../src/ops/enable_metrics_collection.rs.
Traces: autoscaling.metrics-collection-invalid-granularity-rejected.

Cause 5 of 8 — PutNotificationConfiguration accepts a non-ARN-shaped TopicARN.
AWS: recorded behavior only (Smithy declares LimitExceededFault/ResourceContentionFault/ServiceLinkedRoleFailure; TopicARN is XmlStringMaxLen255 with no ARN pattern). A TopicARN that is not ARN-shaped is HTTP 400, Code=ValidationError, Message "Invalid TopicARN: <TopicARN>" — fixed prefix "Invalid TopicARN: " plus the rejected value from the request — and no notification configuration is created for the group.
Emulator: NotImplemented (501); the check reports `got 'NotImplemented'`.
Why: a malformed topic ARN is silently indistinguishable from an outage, and a caller that assumes the configuration landed never receives lifecycle notifications.
Fix: the PutNotificationConfiguration handler and its validation, services/.../src/ops/put_notification_configuration.rs.
Traces: autoscaling.put-notification-configuration-invalid-topic-arn-rejected.

Cause 6 of 8 — PutScalingPolicy performs none of its three input checks.
AWS: recorded behavior only (Smithy declares LimitExceededFault/ResourceContentionFault/ServiceLinkedRoleFailure; AdjustmentType is XmlStringMaxLen255 and Cooldown is an unconstrained integer in the model). All three refusals are HTTP 400, Code=ValidationError: (a) unknown AdjustmentType — Message exactly "Valid adjustment types are: [PercentChangeInCapacity, ChangeInCapacity, ExactCapacity]." (all fixed text); (b) PolicyType=StepScaling with no StepAdjustments — Message exactly "You should specify at least one StepAdjustment with a StepScaling policy." (all fixed text); (c) negative Cooldown — AWS's standard constraint sentence "1 validation error detected: Value '<Cooldown>' at 'cooldown' failed to satisfy constraint: Member must have value greater than or equal to 0", whose only variable part is the rejected value. In each case DescribePolicies must show the policy was not persisted.
Emulator: NotImplemented (501) for all three; the checks report `got NotImplemented` / `got 'NotImplemented'`.
Why: step-scaling configuration errors are the most common client mistake here; a 501 gives the caller no field name, no accepted value list and no constraint, and the "not persisted" guarantee cannot be observed at all.
Fix: the PutScalingPolicy handler and its validation, services/.../src/ops/put_scaling_policy.rs — validate AdjustmentType against the enumerated set, require StepAdjustments for PolicyType=StepScaling, and range-check Cooldown before storing.
Traces: autoscaling.put-scaling-policy-invalid-adjustment-type-fails, autoscaling.put-scaling-policy-missing-required-param-rejected, autoscaling.put-scaling-policy-negative-cooldown-rejected.

Cause 7 of 8 — PutScheduledUpdateGroupAction accepts an unparseable Recurrence.
AWS: recorded behavior only (Smithy declares AlreadyExistsFault/LimitExceededFault/ResourceContentionFault; Recurrence is XmlStringMaxLen255 with no cron pattern). A Recurrence that is not a valid 5-field cron expression is HTTP 400, Code=ValidationError, Message "Given recurrence string: <Recurrence> is invalid" — fixed text "Given recurrence string: " / " is invalid" around the rejected value — and DescribeScheduledActions shows no stored action.
Emulator: NotImplemented (501); the check reports `got NotImplemented`.
Why: without the refusal a client believes it scheduled a scaling action that can never fire, and the emulator cannot even report that it stored nothing.
Fix: the PutScheduledUpdateGroupAction handler and its validation, services/.../src/ops/put_scheduled_update_group_action.rs.
Traces: autoscaling.put-scheduled-action-malformed-recurrence-rejected.

Cause 8 of 8 — SetInstanceHealth does not refuse an Unhealthy mark inside the health-check grace period.
AWS: recorded behavior only (Smithy declares only ResourceContentionFault). SetInstanceHealth with HealthStatus=Unhealthy and ShouldRespectGracePeriod=true against an instance whose age is below the group's HealthCheckGracePeriod is HTTP 400, Code=ValidationError, Message "The age of the instance (<age> seconds) is less than the configured grace period of <HealthCheckGracePeriod> seconds" — fixed text with the instance's server-measured age (a per-run value, not a requirement) and the group's configured grace period from state — and no replacement is started.
Emulator: NotImplemented (501); the check reports "... was accepted, or refused with the wrong code: 'NotImplemented'".
Why: the grace-period contract is what stops a deploy script from killing instances that are still booting; with a 501 the caller cannot tell the mark was rejected, and the emulator can model neither the refusal nor the "no replacement started" effect.
Fix: the SetInstanceHealth handler and its validation, services/.../src/ops/set_instance_health.rs — compare instance age against the group's HealthCheckGracePeriod when ShouldRespectGracePeriod is true.
Traces: autoscaling.set-instance-health-inside-the-grace-period-rejected-and-the-instance-is-not-replaced.

## Evidence
All 12 checks are graded `mismatch`: the emulator answered, with the wrong error class.

Expected (AWS) vs actual (emulator) per cause:
1. AttachInstances: expected HTTP 400 ValidationError + "The list of availability zones [...] for the group ... does not contain the zone occupied by the instance ... [...]" / "AutoScalingGroup <name> has min-size=.., max-size=.., and desired-size=... To attach 1 instance, please update the AutoScalingGroup sizes appropriately." / "Instance [<id>] is invalid." — actual: `AssertionError: expected ValidationError, got NotImplemented` (ClientError (NotImplemented): "operation AttachInstances is scaffolded but not implemented in this emulator", HTTP 501).
2. AttachLoadBalancers: expected 400 ValidationError + "Provided Load Balancers may not be valid. Please ensure they exist and try again." — actual: `expected ValidationError attaching a nonexistent CLB, got NotImplemented`.
3. DescribeAutoScalingInstances: expected 400 InvalidNextToken + "The token '<token>' is invalid" — actual: `DescribeAutoScalingInstances: expected InvalidNextToken, got NotImplemented`.
4. EnableMetricsCollection: expected 400 ValidationError + "Valid metrics granularity type is: [1Minute]." — actual: `an unsupported granularity must be rejected with ValidationError, got 'NotImplemented'`.
5. PutNotificationConfiguration: expected 400 ValidationError + "Invalid TopicARN: <value>" — actual: `expected ValidationError for a malformed TopicARN, got 'NotImplemented'`.
6. PutScalingPolicy: expected 400 ValidationError + "Valid adjustment types are: [PercentChangeInCapacity, ChangeInCapacity, ExactCapacity]." / "You should specify at least one StepAdjustment with a StepScaling policy." / "1 validation error detected: Value '-1' at 'cooldown' failed to satisfy constraint: Member must have value greater than or equal to 0" — actual: `expected ValidationError, got NotImplemented` (x3).
7. PutScheduledUpdateGroupAction: expected 400 ValidationError + "Given recurrence string: <value> is invalid" — actual: `expected ValidationError, got NotImplemented`.
8. SetInstanceHealth: expected 400 ValidationError + "The age of the instance (<age> seconds) is less than the configured grace period of <grace> seconds" — actual: refused with `'NotImplemented'`.

In every case the follow-up "effect absent" assertions (DescribePolicies/DescribeScheduledActions/DescribeNotificationConfigurations empty, group Instances unchanged) are unreachable because the call never produced a client refusal.

## Steps to Reproduce
1. Boot the emulator and point an awsQuery autoscaling client at it.
2. Create a group (CreateAutoScalingGroup, MinSize=0/MaxSize=0 or 1, a launch template) so each call below is state-valid apart from the one broken field.
3. Call each of: AttachInstances with an instance in another AZ / with MaxSize=0 / with InstanceIds=["i-0123456789abcdef0"]; AttachLoadBalancers with a CLB name that does not exist; DescribeAutoScalingInstances with NextToken="sim2real-not-a-real-next-token-xyz123"; EnableMetricsCollection with Granularity="5Minutes"; PutNotificationConfiguration with a non-ARN TopicARN; PutScalingPolicy with AdjustmentType="NotAType", with PolicyType="StepScaling" and no StepAdjustments, and with Cooldown=-1; PutScheduledUpdateGroupAction with Recurrence="not a cron expression"; SetInstanceHealth(HealthStatus="Unhealthy", ShouldRespectGracePeriod=True) on an instance younger than HealthCheckGracePeriod.
4. Observe HTTP 501 with Code=NotImplemented and the message "operation <Op> is scaffolded but not implemented in this emulator" for every call, instead of HTTP 400 with Code=ValidationError (InvalidNextToken for DescribeAutoScalingInstances) and the message text quoted above.
5. Re-replay the checks in trace_ids to confirm a fix.
