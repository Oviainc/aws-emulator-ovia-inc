Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.desired-capacity-type-vcpu-counts-vcpus-not-instances autoscaling.instance-refresh-checkpoint-delay autoscaling.rollback-instance-refresh-irreversible-fails
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.desired-capacity-type-vcpu-counts-vcpus-not-instances|_describe_groups#11|Timeout|return groups
Issue-Key: autoscaling.instance-refresh-checkpoint-delay|run#17|AssertionError|assert held_seconds >= CHECKPOINT_DELAY_S * 0.75, (
Issue-Key: autoscaling.rollback-instance-refresh-irreversible-fails|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: d38d8ab45cd7905c8c6149b13ae22553e33f698d

# Auto Scaling: 3 net-new causes — a DesiredCapacityType=vcpu group never converges, StartInstanceRefresh ignores CheckpointDelay, and the $Latest RollbackInstanceRefresh refusal uses the wrong sentence

> _Note: this issue was identified against the code base at commit `d38d8ab45cd7905c8c6149b13ae22553e33f698d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RollbackInstanceRefresh

## Description
Three distinct Auto Scaling defects found by the behavioral replay. All three are in the autoscaling service and are net-new (no open issue carries their replay keys).

Cause 1 of 4 — wait, there are 3 causes. Cause 1 of 3: a group created with DesiredCapacityType='vcpu' never reaches a steady InService fleet, so the vCPU capacity-unit contract is unobservable.
AWS: Smithy: CreateAutoScalingGroup.DesiredCapacityType documents "The unit of measurement for the value specified for desired capacity ... Valid values: units | vcpu | memory-mib", and Amazon EC2 Auto Scaling supports it for attribute-based instance type selection only. With DesiredCapacityType='vcpu' and DesiredCapacity=N, the group launches instances until the SUM of their vCPUs equals N — for a pool constrained by InstanceRequirements.VCpuCount {Min:2,Max:2} that is N/2 instances, not N instances. DescribeAutoScalingGroups must echo DesiredCapacityType='vcpu' and DesiredCapacity=N (the vCPU budget, unchanged), and the group's Instances must settle at N/2 members in LifecycleState=InService.
Emulator: the check never gets past polling for InService members summing to the vCPU budget; it is killed by the suite's 180s wall clock ("Check timed out after 180.0s") with no instance ever InService. The emulator therefore neither counts vCPUs nor counts instances — it converges on nothing.
Why: a client that sizes a mixed-instances group by vCPU budget (the documented attribute-based-selection path) sees a group that never becomes capacity; any wait-for-InService loop hangs forever rather than failing, so autoscaling-driven provisioning cannot be exercised at all.
Fix: the Auto Scaling group reconciliation/launch path (the component that turns DesiredCapacity into Instances) plus its DesiredCapacityType/InstanceRequirements capacity accounting. Related but NOT covering this check: open issue 20261107T124656Z-autoscaling-a-group-with-desiredcapacity-0-never-reports-an.md describes a group with DesiredCapacity>0 never reporting an InService instance and mentions vCPU-capacity flows; the replay did not bind this check to it, so it is carried here.
Traces: autoscaling.desired-capacity-type-vcpu-counts-vcpus-not-instances.

Cause 2 of 3: StartInstanceRefresh accepts Preferences.CheckpointDelay but never holds at the checkpoint.
AWS: Smithy: StartInstanceRefresh.Preferences -> RefreshPreferences.CheckpointDelay is documented as "(Optional) The amount of time, in seconds, to wait after a checkpoint before continuing. This property is optional, but if you specify a value for it, you must also specify a value for CheckpointPercentages". So a refresh that reaches a checkpoint percentage must stay at that checkpoint — observable through DescribeInstanceRefreshes (PercentageComplete pinned at the checkpoint, Status still InProgress) — for approximately the configured CheckpointDelay seconds before advancing.
Emulator: the refresh is never held: the check measures 0.0s spent at/past the checkpoint against a configured CheckpointDelay of 20s ("checkpoint delay was not honored: only held at/past the checkpoint for 0.0s"). The configured delay is accepted and then discarded.
Why: a client that uses checkpoints to gate a staged rollout (bake time between batches, manual verification windows) gets an instantaneous full rollout instead; nothing a client does during the window can take effect, and tests that assert the pause see a refresh that has already moved on.
Fix: the StartInstanceRefresh handler's refresh-progression state machine — it must persist Preferences.CheckpointPercentages/CheckpointDelay and gate advancement on wall-clock time, rather than completing the refresh in one step.
Traces: autoscaling.instance-refresh-checkpoint-delay.

Cause 3 of 3: RollbackInstanceRefresh on a group pinned to launch-template version '$Latest' is refused with the wrong message.
AWS: Smithy: RollbackInstanceRefresh declares IrreversibleInstanceRefreshFault (errorKind client, awsQueryError code IrreversibleInstanceRefresh, HTTP 400). When the group references its launch template by the symbolic '$Latest' or '$Default' version, the rollback is not supported and AWS words that refusal, verbatim: "This Auto Scaling group's launch template version isn't compatible with a rollback. Rollbacks are not supported when the Auto Scaling group uses the launch template's '$Latest' or '$Default' version." The whole sentence is fixed text — it has no request- or state-derived variable parts; only the literal '$Latest'/'$Default' token list is part of the template. The active refresh must also NOT be switched into a rollback state.
Emulator: same error class but a different sentence — "The instance refresh cannot be rolled back because there is no desired configuration specified for the instance refresh." That is AWS's wording for a different, unrelated cause (a refresh started without DesiredConfiguration), so the emulator misdiagnoses the symbolic-version case as a missing-desired-configuration case.
Why: a client branching on the message (or surfacing it to an operator) is told to supply a DesiredConfiguration, which will not help — the actual remedy is to pin the group to a numeric launch-template version. Message-matching retry/diagnostic logic takes the wrong branch.
Fix: the RollbackInstanceRefresh handler's eligibility check — distinguish "group uses $Latest/$Default" from "refresh has no DesiredConfiguration" and emit each case's own IrreversibleInstanceRefresh sentence. Related but NOT covering this check: open issue 20261107T081710Z-autoscaling-auto-scaling-not-found-deletion-protection-and-instance.md covers the no-active-refresh RollbackInstanceRefresh refusals (rollback-instance-refresh-no-active-fails, rollback-without-active-refresh-fails); the $Latest/$Default incompatibility sentence is a separate branch and is carried here.
Traces: autoscaling.rollback-instance-refresh-irreversible-fails.

## Evidence
Cause 1 — autoscaling.desired-capacity-type-vcpu-counts-vcpus-not-instances
  Expected: DescribeAutoScalingGroups reports DesiredCapacityType='vcpu' and DesiredCapacity=4, and the group settles at 2 InService instances of 2 vCPUs each (vCPU sum == 4).
  Actual: "Error: Check timed out after 180.0s" — the poll for InService instances summing to the vCPU budget (timeout_s=1200, interval_s=2) never satisfied its predicate; no instance ever reached InService.

Cause 2 — autoscaling.instance-refresh-checkpoint-delay
  Expected: with CheckpointDelay=20 and CheckpointPercentages set, DescribeInstanceRefreshes shows the refresh held at the checkpoint for ~>=15s (0.75 x 20s) before advancing.
  Actual: AssertionError: "checkpoint delay was not honored: only held at/past the checkpoint for 0.0s (expected close to the configured 20s CheckpointDelay)" (check_autoscaling_instance_refresh_checkpoint_delay.py line 82).

Cause 3 — autoscaling.rollback-instance-refresh-irreversible-fails
  Expected (HTTP 400, code IrreversibleInstanceRefresh): message "This Auto Scaling group's launch template version isn't compatible with a rollback. Rollbacks are not supported when the Auto Scaling group uses the launch template's '$Latest' or '$Default' version."
  Actual: message 'The instance refresh cannot be rolled back because there is no desired configuration specified for the instance refresh.'
  AssertionError: expected message "This Auto Scaling group's launch template version isn't compatible with a rollback. ...", got 'The instance refresh cannot be rolled back because there is no desired configuration specified for the instance refresh.'

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these checks:

Cause 1 — autoscaling.desired-capacity-type-vcpu-counts-vcpus-not-instances
  1. CreateLaunchTemplate with an AL2023 ImageId.
  2. CreateAutoScalingGroup with MinSize=0, MaxSize=16, DesiredCapacity=4, DesiredCapacityType='vcpu', and a MixedInstancesPolicy whose Overrides carry InstanceRequirements {VCpuCount:{Min:2,Max:2}, MemoryMiB:{Min:1024}, AllowedInstanceTypes:[2-vCPU types offered in the AZ]} and InstancesDistribution {OnDemandBaseCapacity:4, OnDemandPercentageAboveBaseCapacity:100}.
  3. Poll DescribeAutoScalingGroups for Instances in LifecycleState=InService whose EC2 vCPU counts sum to 4. Expect 2 instances; observe the poll never satisfied (check killed at the 180s suite wall clock).

Cause 2 — autoscaling.instance-refresh-checkpoint-delay
  1. Create a launch template and an ASG with MinSize=MaxSize=DesiredCapacity=2, DefaultInstanceWarmup=0; wait for both instances InService.
  2. StartInstanceRefresh with Preferences containing CheckpointPercentages and CheckpointDelay=20.
  3. Poll DescribeInstanceRefreshes and measure how long the refresh stays at/past the checkpoint. Expect ~20s; observe 0.0s.

Cause 3 — autoscaling.rollback-instance-refresh-irreversible-fails
  1. CreateLaunchTemplate; CreateAutoScalingGroup with LaunchTemplate={LaunchTemplateName:<name>, Version:'$Latest'}, MinSize=MaxSize=DesiredCapacity=1, HealthCheckType='EC2'.
  2. Wait for the instance to reach InService, then StartInstanceRefresh.
  3. Call RollbackInstanceRefresh on the group and read the ClientError. Expect HTTP 400 / IrreversibleInstanceRefresh with the '$Latest' or '$Default' sentence; observe the no-desired-configuration sentence instead. Also verify the active refresh was not flipped into a rollback state.
