Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.set-desired-capacity-launches-instances
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: auto-scaling|SetDesiredCapacity|NotImplemented
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# SetDesiredCapacity answers 501 NotImplemented for a VPCZoneIdentifier-only group instead of launching instances

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetDesiredCapacity

## Description
ONE net-new root cause on an Auto Scaling path the emulator does answer (the service is up and serving: CreateAutoScalingGroup for this group SUCCEEDED and DescribeAutoScalingGroups returned it at DesiredCapacity=0). The failing check was NOT marked ALREADY FILED and NOT marked OWNED by any open issue.

Cause 1 of 1: SetDesiredCapacity refuses to launch instances for an Auto Scaling group whose placement is expressed only through VPCZoneIdentifier, answering an unmodeled 501 NotImplemented, because the emulator does not resolve a subnet to its Availability Zone in the launch path.

AWS: SetDesiredCapacity sets the group's desired capacity and Auto Scaling then launches (or terminates) instances to reach it; the call returns HTTP 200 with an empty body. Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01 model): SetDesiredCapacity input SetDesiredCapacityType {AutoScalingGroupName required XmlStringMaxLen255 @length 1..255, DesiredCapacity required integer, HonorCooldown boolean}, output Unit, and its ONLY modeled errors are ScalingActivityInProgressFault (awsQueryError ScalingActivityInProgress, 400) and ResourceContentionFault (awsQueryError ResourceContention, 500). There is no NotImplemented error and no error for "AZ unknown" at all. Placement is legitimately VPCZoneIdentifier-only: in CreateAutoScalingGroupType both AvailabilityZones and VPCZoneIdentifier are OPTIONAL members, and for a VPC group AWS derives each AZ from the subnets named in VPCZoneIdentifier — so a group created with VPCZoneIdentifier alone is a fully valid, fully launchable group. Recorded behavior only for the emulator's refusal text.

Emulator: `SetDesiredCapacity(AutoScalingGroupName=<group created with VPCZoneIdentifier only>, DesiredCapacity=2)` is answered with error code `NotImplemented` (HTTP 501) and the message (example) "launching instances for a group that names only VPCZoneIdentifier needs each subnet's Availability Zone, which this emulator does not hold: pass AvailabilityZones as well". No instance is launched and the group stays at 0 instances. The group itself was accepted at create time, so the gap is specifically in the instance-launch path, not in create validation.

Why: the single most common way to scale a VPC-based Auto Scaling group fails outright. A client that created a group the documented way (subnets only, no --availability-zones) can never bring capacity up: no instance is ever launched, so nothing downstream is reachable — EC2 never reports running members, tags never propagate at launch, scale-in has nothing to terminate, and lifecycle/notification/metric behavior that depends on real members can never be observed. NotImplemented is not a modeled Auto Scaling error for this operation, so an SDK cannot branch on it or classify it as retriable; it surfaces as an opaque 501 server failure on a request AWS answers 200.

Fix: the SetDesiredCapacity handler's instance-launch path in the autoscaling service crate (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01). It must resolve each subnet id in the group's VPCZoneIdentifier to that subnet's AvailabilityZone through the emulated EC2 subnet state (the same state DescribeSubnets serves) and place launched instances in those subnets/AZs, instead of requiring the caller to have supplied AvailabilityZones. The same resolution must back the group's reported AvailabilityZones. Related open issues record this refusal on a DIFFERENT operation (CreateAutoScalingGroup) and do not carry this check's replay key: 20261023T205437Z-autoscaling-auto-scaling-createautoscalinggroup-refuses-a-vpczoneidentifier-only-group.md, 20261022T221142Z-autoscaling-ec2-elbv2-auto-scaling-elb-cli-workflows-createautoscalinggroup-refuses-a.md, 20261023T025808Z-autoscaling-ec2-elbv2-auto-scaling-ec2-elbv2-integration-blocked-elbv2-createtargetgroup.md.

Traces: autoscaling.set-desired-capacity-launches-instances

## Evidence
Expected: SetDesiredCapacity(AutoScalingGroupName=<g>, DesiredCapacity=2) -> HTTP 200, empty body; DescribeAutoScalingGroups(<g>) then converges to 2 Instances in LifecycleState 'InService', EC2 DescribeInstances reports both running from the launch template's ImageId/InstanceType in one of the group's subnets with the PropagateAtLaunch tag; SetDesiredCapacity(..., DesiredCapacity=0) then terminates them.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SetDesiredCapacity operation: launching instances for a group that names only VPCZoneIdentifier needs each subnet's Availability Zone, which this emulator does not hold: pass AvailabilityZones as well (HTTP 501). The error is raised on the first SetDesiredCapacity call, before any instance exists, so every later assertion in the check is unreachable. Note the group's CreateAutoScalingGroup (MinSize=0, MaxSize=3, DesiredCapacity=0, VPCZoneIdentifier=<two subnets>, no AvailabilityZones) had already returned 200 and DescribeAutoScalingGroups reported it with 0 instances. Model check: SetDesiredCapacity declares only ScalingActivityInProgressFault (400) and ResourceContentionFault (500) — NotImplemented/501 is unmodeled.

## Steps to Reproduce
1. Point botocore 'autoscaling', 'ec2' and 'ssm' clients at the emulator in us-west-2.
2. ec2.create_vpc(CidrBlock='10.81.0.0/16'); create two subnets in two different available AZs (10.81.1.0/24, 10.81.2.0/24).
3. Resolve an AMI (ssm.get_parameter('/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64'), falling back to ec2.describe_images) and ec2.create_launch_template(LaunchTemplateData={'ImageId': <ami>, 'InstanceType': 't3.micro'}).
4. autoscaling.create_auto_scaling_group(AutoScalingGroupName=<g>, LaunchTemplate={'LaunchTemplateName': <lt>, 'Version': '$Latest'}, MinSize=0, MaxSize=3, DesiredCapacity=0, VPCZoneIdentifier='<subnet-a>,<subnet-b>', HealthCheckGracePeriod=0, Tags=[{'Key':'sim2real','Value':<check>,'PropagateAtLaunch':True}]) — note NO AvailabilityZones. It succeeds.
5. autoscaling.set_desired_capacity(AutoScalingGroupName=<g>, DesiredCapacity=2) — observe HTTP 501 / error code NotImplemented instead of 200 followed by two InService instances.
Equivalently, re-replay behavioral check autoscaling.set-desired-capacity-launches-instances.
