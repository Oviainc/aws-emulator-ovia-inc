Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.create-asg-invalid-availability-zone-rejected autoscaling.create-asg-invalid-launch-template-id-rejected autoscaling.deep-autoscaling-1-asg-recon-healthy-spread-over-100-rejected autoscaling.deep-autoscaling-1-asg-recon-max-healthy-without-min-rejected autoscaling.mixed-instances-duplicate-override-rejected autoscaling.deep-autoscaling-2-asg-enforce-terminate-hook-abandon-retain-rejects-force-delete autoscaling.create-asg-both-launch-template-and-config-rejected autoscaling.launch-instances-missing-iam-permission-denied autoscaling.delete-auto-scaling-group-concurrent-contention-fails
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.create-asg-invalid-availability-zone-rejected|_client_error#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: autoscaling.create-asg-invalid-launch-template-id-rejected|_error_code#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: autoscaling.deep-autoscaling-1-asg-recon-healthy-spread-over-100-rejected|_error#8|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: autoscaling.deep-autoscaling-1-asg-recon-max-healthy-without-min-rejected|_error#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: autoscaling.mixed-instances-duplicate-override-rejected|_error_code#5|AssertionError|raise AssertionError("the request was ACCEPTED; real AWS rejects it with ValidationError")
Issue-Key: autoscaling.deep-autoscaling-2-asg-enforce-terminate-hook-abandon-retain-rejects-force-delete|_error_code#9|AssertionError|raise AssertionError("call unexpectedly SUCCEEDED where real AWS refuses it")
Issue-Key: autoscaling.create-asg-both-launch-template-and-config-rejected|run#13|AssertionError|assert message == (
Issue-Key: autoscaling.launch-instances-missing-iam-permission-denied|_error#15|AssertionError|raise AssertionError(f"expected AccessDenied, but the call SUCCEEDED and returned {result!r}")
Issue-Key: autoscaling.delete-auto-scaling-group-concurrent-contention-fails|run#15|AssertionError|assert len(successes) == 8, (
Issue-Commit: 38b869ac761e6c1ceaf9cdd20433d5027a2449ee

# Auto Scaling accepts requests AWS refuses: 8 net-new causes — bogus AZ, unknown launch template, InstanceMaintenancePolicy spread, duplicate overrides, retain-trigger force delete, launch-source precedence, unauthorized create, and racing ForceDelete

> _Note: this issue was identified against the code base at commit `38b869ac761e6c1ceaf9cdd20433d5027a2449ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
EIGHT net-new root causes where the Auto Scaling control plane accepts (HTTP 200) a request real AWS refuses, or refuses one it accepts. Each must be closed separately. Related open issues 20261022T112459Z-autoscaling-group-round-trip-create-skips-min-max.md and 20261102T114505Z-autoscaling-createautoscalinggroup-and-updateautoscalinggroup-launch-template-and-argument-validation.md enumerate other create-time validation gaps; none lists the check ids below.

Cause 1 of 8: CreateAutoScalingGroup accepts an AvailabilityZones entry that does not exist.
AWS: HTTP 400, Error.Code ValidationError, Error.Message 'You must use a valid fully-formed launch template. Invalid availability zone: [<zone>]' (fixed text with the rejected zone string from the request in brackets); no group is created. Smithy: CreateAutoScalingGroupType.AvailabilityZones targets AvailabilityZones (list of XmlStringMaxLen255, no enum), so the zone-existence rule is recorded behavior only.
Emulator: HTTP 200 with an empty CreateAutoScalingGroupResponse and the group is created on a nonexistent zone.
Why: a typo'd zone yields a group that can never launch, and the caller gets no signal at the only point AWS gives one.
Fix: availability-zone resolution in the CreateAutoScalingGroup handler's validation.
Traces: autoscaling.create-asg-invalid-availability-zone-rejected.

Cause 2 of 8: CreateAutoScalingGroup accepts a LaunchTemplateId that does not exist.
AWS: HTTP 400 / ValidationError and no group; the launch template reference is resolved against EC2 before the group is written.
Emulator: HTTP 200, group created against a dangling template id.
Why: the group is permanently unable to launch and DescribeAutoScalingGroups advertises a template that EC2 does not have.
Fix: launch-template resolution in the CreateAutoScalingGroup handler.
Traces: autoscaling.create-asg-invalid-launch-template-id-rejected.

Cause 3 of 8: InstanceMaintenancePolicy cross-field rules are not enforced on Create/Update.
AWS: HTTP 400 / ValidationError for (a) a Min/Max healthy-percentage spread greater than 100 and (b) MaxHealthyPercentage supplied without MinHealthyPercentage; the group/policy is left untouched. Smithy: InstanceMaintenancePolicy.MinHealthyPercentage/MaxHealthyPercentage target IntPercentResettable with no modeled cross-field rule, so these are recorded behavior only.
Emulator: both requests are accepted with HTTP 200 and the policy is stored.
Why: the stored policy violates an invariant the service guarantees, so later refreshes compute nonsensical healthy bounds; a caller that relies on the refusal to catch an operator mistake never sees it.
Fix: the InstanceMaintenancePolicy cross-field validation in the CreateAutoScalingGroup and UpdateAutoScalingGroup handlers.
Traces: autoscaling.deep-autoscaling-1-asg-recon-healthy-spread-over-100-rejected, autoscaling.deep-autoscaling-1-asg-recon-max-healthy-without-min-rejected.

Cause 4 of 8: a MixedInstancesPolicy with the same instance type listed twice in Overrides is accepted.
AWS: HTTP 400 / ValidationError; duplicate LaunchTemplateOverrides entries for one InstanceType are refused and no group is created. Recorded behavior only (Smithy's Overrides list carries no @uniqueItems).
Emulator: the request is ACCEPTED.
Why: the group's allocation strategy is then driven by a weighting list AWS would never store, so capacity decisions silently differ from production.
Fix: duplicate-override detection in MixedInstancesPolicy validation (CreateAutoScalingGroup/UpdateAutoScalingGroup handlers).
Traces: autoscaling.mixed-instances-duplicate-override-rejected.

Cause 5 of 8: DeleteAutoScalingGroup with ForceDelete=true is accepted while a TerminateHookAbandon retention trigger is set to retain.
AWS: HTTP 400 / ValidationError (undeclared in the model) with Error.Message 'You can\'t force delete this Auto Scaling group because the TerminateHookAbandon retention trigger is set to retain. Change the retention trigger to terminate and try again.' — fixed text, no variable parts; the group survives.
Emulator: the ForceDelete SUCCEEDS and the group is deleted.
Why: a retention guard whose whole purpose is to block a destructive call is inert, so instances the operator asked to retain are destroyed.
Fix: the retention-trigger precondition in the DeleteAutoScalingGroup handler (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/delete_auto_scaling_group.rs).
Traces: autoscaling.deep-autoscaling-2-asg-enforce-terminate-hook-abandon-retain-rejects-force-delete.

Cause 6 of 8: when both LaunchTemplate and LaunchConfigurationName are supplied, the launch-configuration existence lookup runs before the mutual-exclusion check.
AWS: HTTP 400 / ValidationError with Error.Message 'Valid requests must contain either LaunchTemplate, LaunchConfigurationName, InstanceId or MixedInstancesPolicy parameter.' — fixed text, no variable parts; the mutual-exclusion rule is evaluated FIRST, so the named launch configuration need not exist.
Emulator: ValidationError 'Launch configuration name not found - Launch configuration <name> not found' — it resolves the launch configuration first and reports a not-found instead of the exclusivity violation.
Why: the caller is told to create a launch configuration, when AWS is telling it to remove one of two mutually exclusive parameters; the error class is wrong even though the code happens to match.
Fix: order the launch-source checks in the CreateAutoScalingGroup handler so the exclusivity/one-of rule precedes any existence lookup, and emit AWS's sentence.
Traces: autoscaling.create-asg-both-launch-template-and-config-rejected.

Cause 7 of 8: CreateAutoScalingGroup succeeds for a caller whose IAM role grants no Auto Scaling permission.
AWS: HTTP 403 with Error.Code AccessDenied and the standard not-authorized sentence naming the assumed-role principal and the autoscaling:CreateAutoScalingGroup action; no group is created. Recorded behavior only for the exact sentence (the principal ARN and session name are per-run values).
Emulator: HTTP 200, group created — authorization is never evaluated.
Why: every least-privilege test passes vacuously; a deployment that would be denied in production succeeds against the emulator.
Fix: request authorization for the Auto Scaling control plane (the same gap open issue 20261104T042502Z-autoscaling-ec2-elbv2-4-net-new-root-causes.md reports for other call sites; this check id is not in it).
Traces: autoscaling.launch-instances-missing-iam-permission-denied.

Cause 8 of 8: concurrent ForceDelete calls on one group are not idempotent — only one is accepted.
AWS: all racing DeleteAutoScalingGroup(ForceDelete=true) calls on the same group return HTTP 200 (none fails with ResourceContention or a not-found ValidationError) and the group is gone afterwards.
Emulator: exactly one racer gets 200; the other seven are refused with Error.Code ValidationError (the group-not-found refusal), because the first delete removes the record before the others are served.
Why: a retrying or parallel client (CloudFormation rollback, Terraform, teardown scripts) sees spurious failures for a delete that in fact succeeded, and treats an idempotent operation as failed.
Fix: make the DeleteAutoScalingGroup handler's delete path idempotent for a group already in pending-delete (accept and return success) rather than refusing after the record is gone.
Traces: autoscaling.delete-auto-scaling-group-concurrent-contention-fails.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (50/414 passed; emulator up).

Cause 1 — autoscaling.create-asg-invalid-availability-zone-rejected [mismatch]: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. The check asserts ValidationError with message 'You must use a valid fully-formed launch template. Invalid availability zone: [<bogus_az>]'.
Cause 2 — autoscaling.create-asg-invalid-launch-template-id-rejected [mismatch]: same 'SUCCEEDED and returned ... HTTPStatusCode 200' against a nonexistent LaunchTemplateId; check asserts ValidationError.
Cause 3 — autoscaling.deep-autoscaling-1-asg-recon-healthy-spread-over-100-rejected and autoscaling.deep-autoscaling-1-asg-recon-max-healthy-without-min-rejected [mismatch]: 'expected a ClientError, but the call succeeded and returned {... HTTPStatusCode: 200 ...}'. Expected code ValidationError; expected messages in the same corpus are 'The difference between MaxHealthyPercentage and MinHealthyPercentage must be less than or equal to 100.' and 'MinHealthyPercentage is required to use MaxHealthyPercentage. Specify MinHealthyPercentage, or remove MaxHealthyPercentage, and try again.'
Cause 4 — autoscaling.mixed-instances-duplicate-override-rejected [mismatch]: AssertionError: the request was ACCEPTED; real AWS rejects it with ValidationError.
Cause 5 — autoscaling.deep-autoscaling-2-asg-enforce-terminate-hook-abandon-retain-rejects-force-delete [mismatch]: AssertionError: call unexpectedly SUCCEEDED where real AWS refuses it; the check's expected message constant is 'You can't force delete this Auto Scaling group because the TerminateHookAbandon retention trigger is set to retain. Change the retention trigger to terminate and try again.'
Cause 6 — autoscaling.create-asg-both-launch-template-and-config-rejected [mismatch]: expected the exact refusal message 'Valid requests must contain either LaunchTemplate, LaunchConfigurationName, InstanceId or MixedInstancesPolicy parameter.', got 'Launch configuration name not found - Launch configuration sim2real-behav-lc-07e03dee99e0 not found' (the launch-configuration name is a per-run value).
Cause 7 — autoscaling.launch-instances-missing-iam-permission-denied [mismatch]: 'expected AccessDenied, but the call SUCCEEDED and returned {... HTTPStatusCode: 200 ...}'; the asserted call is CreateAutoScalingGroup made with a freshly created role that has no policies.
Cause 8 — autoscaling.delete-auto-scaling-group-concurrent-contention-fails [mismatch]: AssertionError: every racing ForceDelete of a pending-delete group is accepted; 1 of 8 succeeded, failure codes ['ValidationError' x7]. The counts 1/8 are the run's observation; the requirement is that all concurrent ForceDeletes are accepted.

## Steps to Reproduce
1. Replay the behavioral checks in trace_ids against the emulator built from this checkout.
2. By hand (awsQuery POSTs to the Auto Scaling endpoint):
   a. CreateAutoScalingGroup with AvailabilityZones=[<zone that does not exist>] -> expect 400 ValidationError 'You must use a valid fully-formed launch template. Invalid availability zone: [<zone>]'.
   b. CreateAutoScalingGroup with LaunchTemplate.LaunchTemplateId=lt-00000000000000000 -> expect 400 ValidationError and no group.
   c. CreateAutoScalingGroup/UpdateAutoScalingGroup with InstanceMaintenancePolicy MinHealthyPercentage=49, MaxHealthyPercentage=150, then with MaxHealthyPercentage=150 alone -> expect 400 ValidationError both times and no stored policy change.
   d. CreateAutoScalingGroup with a MixedInstancesPolicy whose Overrides list the same InstanceType twice -> expect 400 ValidationError.
   e. On a group whose TerminateHookAbandon retention trigger is 'retain', DeleteAutoScalingGroup ForceDelete=true -> expect 400 ValidationError with the retention-trigger sentence and the group still present.
   f. CreateAutoScalingGroup supplying BOTH LaunchTemplate and a LaunchConfigurationName that does not exist -> expect the 'Valid requests must contain either ...' sentence, not a launch-configuration not-found.
   g. Assume a role with no policies and call CreateAutoScalingGroup -> expect 403 AccessDenied and no group.
   h. Fire 8 concurrent DeleteAutoScalingGroup(ForceDelete=true) for one empty group -> expect all 8 to return 200 and the group to disappear.
