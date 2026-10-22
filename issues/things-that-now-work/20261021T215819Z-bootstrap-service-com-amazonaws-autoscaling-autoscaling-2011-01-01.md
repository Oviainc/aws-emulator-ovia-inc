Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the autoscaling emulator

_Filed: 2026-10-21T21:58:19Z_
_Operation: service-com-amazonaws-autoscaling-autoscaling-2011-01-01 | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 68 operations of the Auto Scaling (`awsQuery`, sigv4 name `autoscaling`) crate are unmodified scaffolds answering HTTP 501 `NotImplemented` and holding zero state. Implement a FIRST, SMALL stateful slice: the Auto Scaling group lifecycle. Every one of these operations must compute its response from the service's persisted state (`crate::state::service_state(&*self.engine)`), never from canned or echoed values.

Happy path, in order:

1. **CreateLaunchConfiguration** — input `LaunchConfigurationName` (required), `ImageId`, `InstanceType`, `KeyName`, `SecurityGroups`, `UserData`, `InstanceMonitoring`, `IamInstanceProfile`, `EbsOptimized`, `AssociatePublicIpAddress`. Empty output. Must store the config keyed by name, stamp `CreatedTime` and a `LaunchConfigurationARN`, and raise `AlreadyExistsFault` (HTTP 400) on a duplicate name.
2. **CreateAutoScalingGroup** — input `AutoScalingGroupName`, `MinSize`, `MaxSize` (all required), plus `DesiredCapacity`, `LaunchConfigurationName`/`LaunchTemplate`, `AvailabilityZones`, `VPCZoneIdentifier`, `HealthCheckType`, `HealthCheckGracePeriod`, `DefaultCooldown`, `TerminationPolicies`, `NewInstancesProtectedFromScaleIn`, `Tags`. Empty output. Must persist the whole group, default `DesiredCapacity` to `MinSize` when absent, default `HealthCheckType` to `EC2`, stamp `CreatedTime` and `AutoScalingGroupARN`, return `AlreadyExistsFault` for a duplicate name, and `ValidationError` when a referenced `LaunchConfigurationName` does not exist or `MinSize > MaxSize` / `DesiredCapacity` outside `[MinSize, MaxSize]`. Materialize `DesiredCapacity` synthetic in-service instances (`i-` ids, `LifecycleState=InService`, `HealthStatus=Healthy`) attached to the group.
3. **DescribeAutoScalingGroups** — input `AutoScalingGroupNames`, `MaxRecords`, `NextToken`; output `AutoScalingGroups` (required) + `NextToken`. Must return the stored groups with the fields exactly as created (sizes, AZs, tags, health check, launch config, `Instances` list, `CreatedTime`), filter by the requested names, and honor `MaxRecords`/`NextToken` paging (`InvalidNextToken`, HTTP 400, for a bad token). An unknown name simply yields an empty list.
4. **SetDesiredCapacity** — input `AutoScalingGroupName`, `DesiredCapacity` (both required), `HonorCooldown`. Empty output. Must mutate the stored group's `DesiredCapacity`, add/remove synthetic instances to match, record a scaling Activity, and raise `ValidationError` for an unknown group or a capacity outside `[MinSize, MaxSize]`.
5. **UpdateAutoScalingGroup** — input `AutoScalingGroupName` (required) plus any of `MinSize`, `MaxSize`, `DesiredCapacity`, `DefaultCooldown`, `HealthCheckType`, `HealthCheckGracePeriod`, `AvailabilityZones`, `VPCZoneIdentifier`, `TerminationPolicies`, `NewInstancesProtectedFromScaleIn`, `LaunchConfigurationName`. Empty output. Must apply ONLY the supplied members (absent member = unchanged), clamp `DesiredCapacity` into the new bounds, and return `ValidationError` for an unknown group.
6. **DeleteAutoScalingGroup** — input `AutoScalingGroupName` (required), `ForceDelete`. Empty output. Must remove the group and its instances from state, return `ValidationError` for an unknown group, and `ResourceInUseFault` (HTTP 400) when the group still has instances/desired capacity > 0 and `ForceDelete` is not set. A following DescribeAutoScalingGroups must no longer list it.

(Implement **DescribeLaunchConfigurations** / **DeleteLaunchConfiguration** too if cheap — same store — but they are not required for the slice.)

## State Model

In `src/state.rs`, on the service state struct reached via `crate::state::service_state`:

- `launch_configurations: BTreeMap<String, LaunchConfiguration>` where `LaunchConfiguration { name, arn, image_id, instance_type, key_name, security_groups: Vec<String>, user_data, iam_instance_profile, instance_monitoring_enabled: bool, ebs_optimized: bool, associate_public_ip: Option<bool>, created_time }`
- `groups: BTreeMap<String, AutoScalingGroup>` (key = `AutoScalingGroupName`) where `AutoScalingGroup { name, arn, launch_configuration_name: Option<String>, launch_template: Option<LaunchTemplateSpecification>, min_size: i32, max_size: i32, desired_capacity: i32, default_cooldown: i32, availability_zones: Vec<String>, vpc_zone_identifier: Option<String>, health_check_type: String, health_check_grace_period: Option<i32>, termination_policies: Vec<String>, new_instances_protected_from_scale_in: bool, tags: Vec<Tag>, status: Option<String>, created_time, instances: Vec<AsgInstance> }`
- `AsgInstance { instance_id, availability_zone, lifecycle_state, health_status, launch_configuration_name, protected_from_scale_in, instance_type }`
- `activities: Vec<Activity>` (`activity_id`, `auto_scaling_group_name`, `cause`, `description`, `start_time`, `status_code`, `progress`) appended by capacity changes and group deletion — the backing store for DescribeScalingActivities later.
- `next_instance_seq: u64` and `next_activity_seq: u64` for deterministic id generation.

All of it lives in the single shared engine; a create must be visible to every later call in the same process.

Operations outside this slice stay 501 and will be filed as individual gap issues.

## Evidence

OPERATIONS.md at the crate root (`services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/OPERATIONS.md`) lists all 68 modeled operations with exactly the input/output members and errors cited above: CreateLaunchConfiguration (§line 80), CreateAutoScalingGroup (§74, `AutoScalingGroupName`/`MinSize`/`MaxSize` required, errors `AlreadyExistsFault`/`LimitExceededFault`), DescribeAutoScalingGroups (§158, @paginated inputToken=NextToken outputToken=NextToken items=AutoScalingGroups, `AutoScalingGroups` required, error `InvalidNextToken`), SetDesiredCapacity (§405, errors `ScalingActivityInProgressFault`), UpdateAutoScalingGroup (§441), DeleteAutoScalingGroup (§92, errors `ResourceInUseFault`/`ScalingActivityInProgressFault`). README.md documents the awsQuery wire protocol and the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker. A grep for that marker over `src/ops/` matches every handler file in the crate (create_auto_scaling_group.rs, describe_auto_scaling_groups.rs, set_desired_capacity.rs, update_auto_scaling_group.rs, delete_auto_scaling_group.rs, create_launch_configuration.rs, …), proving the service holds no state today. No tests assert any of this behavior.

## Steps to Reproduce

1. aws autoscaling create-launch-configuration --launch-configuration-name lc-1 --image-id ami-12345678 --instance-type t3.micro  → expect 200, empty body (today: HTTP 501 NotImplemented)
2. aws autoscaling create-auto-scaling-group --auto-scaling-group-name asg-1 --launch-configuration-name lc-1 --min-size 1 --max-size 4 --desired-capacity 2 --availability-zones us-east-1a  → expect 200
3. aws autoscaling describe-auto-scaling-groups --auto-scaling-group-names asg-1  → expect one group with MinSize=1, MaxSize=4, DesiredCapacity=2, HealthCheckType=EC2, LaunchConfigurationName=lc-1, a CreatedTime, and 2 Instances in LifecycleState=InService
4. aws autoscaling set-desired-capacity --auto-scaling-group-name asg-1 --desired-capacity 3  → expect 200; re-describe shows DesiredCapacity=3 and 3 instances
5. aws autoscaling update-auto-scaling-group --auto-scaling-group-name asg-1 --max-size 6 --health-check-type ELB --health-check-grace-period 300  → expect 200; re-describe shows MaxSize=6, HealthCheckType=ELB, DesiredCapacity still 3, MinSize still 1
6. aws autoscaling create-auto-scaling-group --auto-scaling-group-name asg-1 --launch-configuration-name lc-1 --min-size 1 --max-size 2  → expect HTTP 400 AlreadyExistsFault
7. aws autoscaling delete-auto-scaling-group --auto-scaling-group-name asg-1  → expect HTTP 400 ResourceInUseFault (group still has instances); then --force-delete  → expect 200
8. aws autoscaling describe-auto-scaling-groups  → expect AutoScalingGroups to be empty
9. aws autoscaling delete-auto-scaling-group --auto-scaling-group-name nope  → expect HTTP 400 ValidationError
