Issue-Source: trace-sweep-divergence
Issue-Trace: asg-err-zone-placement
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: asg-err-zone-placement asglf-activity-arch-mismatch asglf-activity-instance-profile-absent asglf-activity-instance-type-invalid asglf-activity-security-group-other-vpc autoscaling-asgdx-traj-suspended-launch-leaves-desired-unfilled autoscaling-asgdx-traj-standby-park-and-return autoscaling-asghc-traj-health-check-type-and-grace-update-refusals autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|DescribeAutoScalingGroups|2xx->2xx|$.DescribeAutoScalingGroupsResponse.DescribeAutoScalingGroupsResult.AutoScalingGroups.member.AvailabilityZones.member
Issue-Key: autoscaling|DescribeAutoScalingGroups|2xx->2xx|$.DescribeAutoScalingGroupsResponse.DescribeAutoScalingGroupsResult.AutoScalingGroups.member.Tags
Issue-Key: autoscaling|DescribeAutoScalingGroups|2xx->2xx|$.DescribeAutoScalingGroupsResponse.DescribeAutoScalingGroupsResult.AutoScalingGroups.member.Instances
Issue-Key: autoscaling|DescribeAutoScalingGroups|2xx->2xx|$.DescribeAutoScalingGroupsResponse.DescribeAutoScalingGroupsResult.AutoScalingGroups.member.DesiredCapacity
Issue-Key: autoscaling|DescribeAutoScalingGroups|2xx->2xx|$.DescribeAutoScalingGroupsResponse.DescribeAutoScalingGroupsResult.AutoScalingGroups.member.Instances.member.AvailabilityZoneId
Issue-Key: autoscaling|DescribeAutoScalingGroups|2xx->2xx|$.DescribeAutoScalingGroupsResponse.DescribeAutoScalingGroupsResult.AutoScalingGroups.member.Instances.member.LaunchTemplate.LaunchTemplateName
Issue-Key: autoscaling|DescribeAutoScalingGroups|2xx->2xx|$.DescribeAutoScalingGroupsResponse.DescribeAutoScalingGroupsResult.AutoScalingGroups.member.Instances.member.LaunchTemplate.Version
Issue-Key: autoscaling|DescribeAutoScalingGroups|2xx->2xx|$.DescribeAutoScalingGroupsResponse.DescribeAutoScalingGroupsResult.AutoScalingGroups.member.HealthCheckGracePeriod
Issue-Key: autoscaling|DescribeInstanceRefreshes|2xx->2xx|$.DescribeInstanceRefreshesResponse.DescribeInstanceRefreshesResult.InstanceRefreshes.member.InstanceRefreshId
Issue-Key: autoscaling|DescribeInstanceRefreshes|2xx->2xx|$.DescribeInstanceRefreshesResponse.DescribeInstanceRefreshesResult.InstanceRefreshes.member.Preferences.AlarmSpecification
Issue-Key: autoscaling|DescribeInstanceRefreshes|2xx->2xx|$.DescribeInstanceRefreshesResponse.DescribeInstanceRefreshesResult.InstanceRefreshes.member.Preferences.AutoRollback
Issue-Key: autoscaling|DescribeInstanceRefreshes|2xx->2xx|$.DescribeInstanceRefreshesResponse.DescribeInstanceRefreshesResult.InstanceRefreshes.member.Preferences.InstanceWarmup
Issue-Key: autoscaling|DescribeInstanceRefreshes|2xx->2xx|$.DescribeInstanceRefreshesResponse.DescribeInstanceRefreshesResult.InstanceRefreshes.member.Preferences.MinHealthyPercentage
Issue-Key: autoscaling|DescribeInstanceRefreshes|2xx->2xx|$.DescribeInstanceRefreshesResponse.DescribeInstanceRefreshesResult.InstanceRefreshes.member.Preferences.SkipMatching
Issue-Related: 20261022T112459Z-autoscaling-group-round-trip-create-skips-min-max.md
Issue-Related: 20261102T114505Z-autoscaling-auto-scaling-group-scoped-operations-refuse-an-existing.md
Issue-Related: 20261102T114505Z-autoscaling-describeautoscalinggroups-misreports-the-group-s-launch-template-and.md
Issue-Commit: 5e1f0b507a128238d75b745ce1d1dcae4dc5a977

# Auto Scaling group and instance state is never applied, so describes read back stale or invented values

> _Note: this issue was identified against the code base at commit `5e1f0b507a128238d75b745ce1d1dcae4dc5a977`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
Five net-new root causes, all in the autoscaling read path: the group/instance/instance-refresh state the emulator keeps does not match what the preceding mutations should have produced, and the awsQuery projections drop modeled members. Each cause must be closed separately.

Cause 1 of 5 - DescribeAutoScalingGroups accepts and echoes an Availability Zone that does not exist, and the group projection omits most modeled members.
AWS: Smithy: AutoScalingGroup members AvailabilityZones (@required list), AvailabilityZoneIds, LoadBalancerNames, TargetGroupARNs, EnabledMetrics, SuspendedProcesses, TrafficSources, ServiceLinkedRoleARN, AvailabilityZoneDistribution, CapacityReservationSpecification, InstanceLifecyclePolicy, Tags (TagDescriptionList), VPCZoneIdentifier. AWS refuses CreateAutoScalingGroup for a launch template whose zone is not a real AZ (recorded response: HTTP 400, Error.Type Sender, Error.Code ValidationError, message "You must use a valid fully-formed launch template. Invalid availability zone: [<zone>]" - fixed text "You must use a valid fully-formed launch template. Invalid availability zone: [" + the zone taken from the request + "]"), so a described group never carries a non-existent zone. For the groups AWS does serve, every member above is emitted, empty lists as empty elements, and ServiceLinkedRoleARN as the account's AWSServiceRoleForAutoScaling role ARN.
Emulator: the create is accepted (200), and the subsequent DescribeAutoScalingGroups returns AvailabilityZones.member = the bogus zone from the request instead of the real AZ, omits CapacityReservationSpecification, EnabledMetrics, TargetGroupARNs, AvailabilityZoneIds, TrafficSources, LoadBalancerNames, InstanceLifecyclePolicy, ServiceLinkedRoleARN, AvailabilityZoneDistribution, SuspendedProcesses and LaunchTemplate.LaunchTemplateName entirely (got None), and serves Tags/VPCZoneIdentifier with the wrong type/value.
Why: a client that reads back a group to learn where it will launch is told about a zone that cannot exist, and any client that reads the omitted members (health-check wiring, enabled metrics, suspended processes, traffic sources, service-linked role) gets a null where AWS always sends a value, so SDK deserialization yields empty optionals and conditional logic silently takes the wrong branch.
Fix: the CreateAutoScalingGroup handler's launch-template/zone validation, plus the DescribeAutoScalingGroups serializer/group projection in the autoscaling service. Related open issues 20261022T112459Z-autoscaling-group-round-trip-create-skips-min-max.md and 20261102T114505Z-autoscaling-describeautoscalinggroups-misreports-the-group-s-launch-template-and.md describe neighbouring symptoms but do not carry this replay key.
Traces: asg-err-zone-placement#task#10.

Cause 2 of 5 - an instance that failed to launch stays in the group's Instances list forever.
AWS: Smithy: AutoScalingGroup.Instances (list of Instance). When a launch is cancelled/fails (architecture mismatch, absent instance profile, invalid instance type, security group in another VPC), AWS removes the instance from the group, so the recorded DescribeAutoScalingGroups serves Instances as an empty element.
Emulator: Instances carries one member stuck at LifecycleState 'Terminating' (with LaunchTemplate.Version '$Latest'), and the same modeled group members listed in cause 1 are omitted.
Why: a client polling the group to see a failed launch drain never observes an empty group, so scale/refresh orchestration waits forever on an instance AWS has already dropped.
Fix: the launch/terminate state transition in the autoscaling group state, plus the DescribeAutoScalingGroups projection.
Traces: asglf-activity-arch-mismatch#task#12 (cluster also covers asglf-activity-instance-profile-absent, asglf-activity-instance-type-invalid, asglf-activity-security-group-other-vpc, autoscaling-asgdx-traj-suspended-launch-leaves-desired-unfilled).

Cause 3 of 5 - EnterStandby is answered 200 but the instance never enters Standby, and the instance projection drops modeled members.
AWS: Smithy: EnterStandby (input AutoScalingGroupName @required, InstanceIds, ShouldDecrementDesiredCapacity @required; output Activities). After a successful EnterStandby, the instance's Instance.LifecycleState is 'Standby'. Smithy: Instance members AvailabilityZoneId, LaunchTemplate (LaunchTemplateSpecification with LaunchTemplateName and the resolved Version).
Emulator: after an accepted EnterStandby the described instance still reads LifecycleState 'InService', AvailabilityZoneId and LaunchTemplate.LaunchTemplateName are omitted (got None), and LaunchTemplate.Version serves the literal '$Latest' instead of the version number the instance launched with.
Why: a standby/return workflow cannot park an instance: the client sees the call succeed and the state unchanged, so any wait for Standby (or ExitStandby back to InService) never converges.
Fix: the EnterStandby handler's group state transition, and the Instance projection in the DescribeAutoScalingGroups serializer.
Traces: autoscaling-asgdx-traj-standby-park-and-return#task#11.

Cause 4 of 5 - a refused HealthCheckGracePeriod update is persisted anyway.
AWS: Smithy: AutoScalingGroup.HealthCheckGracePeriod / UpdateAutoScalingGroup.HealthCheckGracePeriod (integer, seconds). A negative grace period is refused with HTTP 400 ValidationError and the group keeps its previous value; the recorded describe serves 0.
Emulator: the described group reads HealthCheckGracePeriod '-1', i.e. the rejected/invalid value was written into the group's state.
Why: the group is left in a state AWS cannot produce; a client reading it back computes health-check windows from a negative number.
Fix: the UpdateAutoScalingGroup handler's argument validation and the group-state write (reject before mutating).
Traces: autoscaling-asghc-traj-health-check-type-and-grace-update-refusals#task#9.

Cause 5 of 5 - DescribeInstanceRefreshes omits the refresh Preferences defaults and echoes AutoRollback instead of the effective value.
AWS: Smithy: RefreshPreferences members MinHealthyPercentage (IntPercent 0-100), InstanceWarmup, SkipMatching ("The default is false"), AutoRollback ("The default is false"; rollback is not supported when no desired configuration is specified or the group uses the launch template's $Latest/$Default version), AlarmSpecification. The recorded response carries MinHealthyPercentage 90, InstanceWarmup 0, SkipMatching 'false', AlarmSpecification as an empty element and AutoRollback 'false'.
Emulator: Preferences.MinHealthyPercentage, InstanceWarmup, AlarmSpecification and SkipMatching are all omitted (got None) and AutoRollback reads 'true'.
Why: a client inspecting a running instance refresh cannot see the effective preferences AWS applied, and is told a rollback will happen in a configuration where AWS does not support one.
Fix: the StartInstanceRefresh handler's preference defaulting and the DescribeInstanceRefreshes serializer. (The InstanceRefreshId difference in the same step is a per-run generated id and is not part of this finding.)
Traces: autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block#task#15.

## Evidence
Cause 1 (asg-err-zone-placement#task#10, DescribeAutoScalingGroups, 200 vs 200): AvailabilityZones.member expected 'us-west-2a', got 'us-west-2a9'; CapacityReservationSpecification expected {'CapacityReservationPreference': 'default'}, got None; EnabledMetrics/TargetGroupARNs/TrafficSources/LoadBalancerNames expected '' (empty element), got None; AvailabilityZoneIds expected {'member': 'usw2-az1'}, got None; InstanceLifecyclePolicy expected {'RetentionTriggers': {'TerminateHookAbandon': 'terminate'}}, got None; ServiceLinkedRoleARN expected the AWSServiceRoleForAutoScaling role ARN, got None; AvailabilityZoneDistribution expected {'CapacityDistributionStrategy': 'balanced-best-effort'}, got None; Tags type_mismatch; LaunchTemplate.LaunchTemplateName missing. Upstream, asg-err-zone-placement#task#7 CreateAutoScalingGroup expected 400 ValidationError 'You must use a valid fully-formed launch template. Invalid availability zone: [us-west-2a9]' and got 200.
Cause 2 (asglf-activity-arch-mismatch#task#12, 200 vs 200): Instances expected '' (empty), got {'member': {'InstanceId': ..., 'LifecycleState': 'Terminating', 'LaunchTemplate': {'Version': '$Latest'}, ...}}; same modeled members omitted; 10 steps in cluster.
Cause 3 (autoscaling-asgdx-traj-standby-park-and-return#task#11, 200 vs 200): Instances.member.LifecycleState expected 'Standby', got 'InService'; Instances.member.AvailabilityZoneId expected 'usw2-az1', got None; Instances.member.LaunchTemplate.Version expected '1', got '$Latest'; Instances.member.LaunchTemplate.LaunchTemplateName missing.
Cause 4 (autoscaling-asghc-traj-health-check-type-and-grace-update-refusals#task#9, 200 vs 200): HealthCheckGracePeriod expected '0', got '-1'.
Cause 5 (autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block#task#15, 200 vs 200): Preferences.MinHealthyPercentage expected '90', got None; Preferences.InstanceWarmup expected '0', got None; Preferences.AlarmSpecification expected '', got None; Preferences.SkipMatching expected 'false', got None; Preferences.AutoRollback expected 'false', got 'true'.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus against the emulator and inspect these steps: asg-err-zone-placement#task#10 (after #task#7 CreateAutoScalingGroup with a launch template naming a non-existent AZ), asglf-activity-arch-mismatch#task#12 (after a launch that must fail on architecture mismatch), autoscaling-asgdx-traj-standby-park-and-return#task#11 (after a successful EnterStandby), autoscaling-asghc-traj-health-check-type-and-grace-update-refusals#task#9 (after an UpdateAutoScalingGroup with HealthCheckGracePeriod=-1), autoscaling-asgdx-traj-instance-refresh-refusals-and-in-progress-block#task#15 (DescribeInstanceRefreshes after StartInstanceRefresh).
