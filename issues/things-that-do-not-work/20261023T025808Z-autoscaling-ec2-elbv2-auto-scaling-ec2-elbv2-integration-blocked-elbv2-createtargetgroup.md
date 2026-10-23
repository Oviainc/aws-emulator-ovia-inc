Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.cli-asg-attach-load-balancer-target-groups-command autoscaling__ec2__elbv2.cli-create-asg-with-target-group-and-verify-ec2-registration autoscaling__ec2__elbv2.target-group-register-fails-instance-wrong-vpc autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule autoscaling__ec2__elbv2.asg-launches-ec2-instance-with-correct-launch-template autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy
Issue-Key: autoscaling__ec2__elbv2|CreateTargetGroup|InvalidAction
Issue-Key: autoscaling__ec2__elbv2|CreateLoadBalancer|InvalidAction
Issue-Key: auto-scaling|CreateAutoScalingGroup|NotImplemented
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# Auto Scaling + EC2 + ELBv2 integration blocked: ELBv2 CreateTargetGroup/CreateLoadBalancer answer InvalidAction, and CreateAutoScalingGroup refuses a VPCZoneIdentifier-only group with NotImplemented (2 causes)

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
Two root causes, grounded in 6 failing behavioral checks whose id's leading token is the composite 'autoscaling__ec2__elbv2'. Related already-open issues that do NOT carry these replay keys: 20261022T221142Z-autoscaling-ec2-elbv2-auto-scaling-elb-cli-workflows-createautoscalinggroup-refuses-a.md (same two symptoms, bound to different check ids) and 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md (CreateLoadBalancer only, under the acm__elbv2 token).

Cause 1 of 2: the ELBv2 awsQuery action table does not resolve CreateTargetGroup or CreateLoadBalancer at API version 2015-12-01, so an Auto Scaling group can never be attached to a target group or load balancer.
AWS: Smithy com.amazonaws.elasticloadbalancingv2 (service version 2015-12-01, protocol awsQuery) binds CreateTargetGroup(Name required, Protocol, Port @range 1..65535, VpcId, TargetType, ...) -> {TargetGroups} with modeled awsQuery errors DuplicateTargetGroupName / InvalidConfigurationRequest / TooManyTargetGroups / TooManyTags (all HTTP 400), and CreateLoadBalancer -> {LoadBalancers}. A well-formed Action=CreateTargetGroup&Version=2015-12-01 POST is served 200 or refused with one of those codes; 'InvalidAction' means the service has no such action.
Emulator: HTTP error with Error.Code 'InvalidAction', message 'Could not find operation CreateTargetGroup for version 2015-12-01' (and the same for CreateLoadBalancer).
Why: AttachLoadBalancerTargetGroups, target registration on scale-out and the wrong-VPC registration refusal cannot be exercised at all, and the client is told its action does not exist rather than that the server lacks it.
Fix: the awsQuery operation dispatch/version table of services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.
Traces: autoscaling__ec2__elbv2.cli-asg-attach-load-balancer-target-groups-command, autoscaling__ec2__elbv2.cli-create-asg-with-target-group-and-verify-ec2-registration, autoscaling__ec2__elbv2.target-group-register-fails-instance-wrong-vpc, autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule.

Cause 2 of 2: CreateAutoScalingGroup refuses a group that names only VPCZoneIdentifier.
AWS: Smithy com.amazonaws.autoscaling#CreateAutoScalingGroup models AvailabilityZones and VPCZoneIdentifier as OPTIONAL members; a VPC Auto Scaling group is normally created with VPCZoneIdentifier alone and AWS derives each subnet's Availability Zone itself, answering 200 and then launching instances into those subnets. There is no modeled error for 'AvailabilityZones omitted'.
Emulator: the call is refused with Error.Code 'NotImplemented' and the message 'launching instances for a group that names only VPCZoneIdentifier needs each subnet's Availability Zone, which this emulator does not hold: pass AvailabilityZones as well'.
Why: the single most common way to create a VPC Auto Scaling group is rejected, so no instance is ever launched and launch-template fidelity, termination-policy scale-in and target-group registration are all unreachable. A client would have to pass a parameter AWS does not require.
Fix: the CreateAutoScalingGroup handler / ASG launch state in the autoscaling service crate — resolve each subnet's Availability Zone from the EC2 subnet state instead of demanding AvailabilityZones from the caller.
Traces: autoscaling__ec2__elbv2.asg-launches-ec2-instance-with-correct-launch-template, autoscaling__ec2__elbv2.asg-scale-in-respects-termination-policy.

## Evidence
Cause 1:
- autoscaling__ec2__elbv2.cli-asg-attach-load-balancer-target-groups-command / .cli-create-asg-with-target-group-and-verify-ec2-registration / .target-group-register-fails-instance-wrong-vpc: botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the CreateTargetGroup operation: Could not find operation CreateTargetGroup for version 2015-12-01
- autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule: An error occurred (InvalidAction) when calling the CreateLoadBalancer operation: Could not find operation CreateLoadBalancer for version 2015-12-01
Expected: HTTP 200 with TargetGroups/LoadBalancers, or HTTP 400 with a modeled awsQuery code.

Cause 2:
- autoscaling__ec2__elbv2.asg-launches-ec2-instance-with-correct-launch-template and .asg-scale-in-respects-termination-policy: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAutoScalingGroup operation: launching instances for a group that names only VPCZoneIdentifier needs each subnet's Availability Zone, which this emulator does not hold: pass AvailabilityZones as well
Expected: HTTP 200 (empty CreateAutoScalingGroup response) and a group whose instances launch into the named subnets.

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: boto3.client('elbv2').create_target_group(Name='sim2real-tg', Protocol='HTTP', Port=80, VpcId=<vpc>) -> Error.Code 'InvalidAction'.
3. Cause 2: boto3.client('autoscaling').create_auto_scaling_group(AutoScalingGroupName='sim2real-asg', LaunchTemplate={...}, MinSize=1, MaxSize=1, DesiredCapacity=1, VPCZoneIdentifier='<subnet-a>,<subnet-b>') with NO AvailabilityZones -> Error.Code 'NotImplemented'.
4. Re-replay the check ids in trace_ids.
