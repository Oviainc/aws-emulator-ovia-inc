Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2.albos-ip-target-availability-zone-resolves-from-the-containing-subnet ec2__elbv2.albos-reregistering-the-same-stale-target-is-idempotent ec2__elbv2.asghc-target-group-health-check-port-out-of-range-rejected ec2__elbv2.ecsnt-ip-target-group-applies-health-check-and-attribute-defaults ec2__elbv2.ecsnt-ip-target-outside-the-vpc-rejected ec2__elbv2.ip-target-from-a-public-range-rejected ec2__elbv2.ip-target-in-the-vpc-cidr-but-outside-every-subnet-rejected ec2__elbv2.ip-targets-of-a-group-no-listener-forwards-to-report-not-in-use
Issue-Key: ec2__elbv2|CreateTargetGroup|InvalidAction
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# The whole EC2/ELBv2 IP-target surface is unreachable: CreateTargetGroup is refused with InvalidAction at version 2015-12-01

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
One root cause, grounded in 8 failing behavioral checks whose id's leading token is the composite 'ec2__elbv2'. The already-open issue 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md names only CreateLoadBalancer under the 'acm__elbv2' token and does not carry these replay keys.

Cause 1 of 1: the ELBv2 awsQuery action table does not resolve CreateTargetGroup at API version 2015-12-01, so no ip-type target group can be created and none of the VPC/subnet-scoped target validation can run.
AWS: Smithy com.amazonaws.elasticloadbalancingv2#CreateTargetGroup (service version 2015-12-01, protocol awsQuery) takes Name (required), Protocol, Port @range 1..65535, VpcId, TargetType, HealthCheck* members (HealthCheckIntervalSeconds @range 5..300, HealthCheckTimeoutSeconds @range 2..120, Healthy/UnhealthyThresholdCount @range 2..10, HealthCheckPath @length 1..1024) and returns {TargetGroups}; it is otherwise refused with one of its modeled awsQuery errors DuplicateTargetGroupName / InvalidConfigurationRequest / TooManyTargetGroups / TooManyTags, all HTTP 400. A health-check port outside 1..65535 is a ValidationError, and registering an ip target outside the VPC/subnet is refused with InvalidTarget by RegisterTargets. 'InvalidAction' is reserved for an action the service does not have.
Emulator: every one of these checks is refused on its first ELBv2 call with Error.Code 'InvalidAction' and message 'Could not find operation CreateTargetGroup for version 2015-12-01'.
Why: ip-target groups are how ECS awsvpc tasks, ENI-backed targets and cross-subnet routing are expressed; with the action unreachable a client can neither create one nor observe any of the refusals AWS makes (public-range ip rejected, ip outside every subnet rejected, port out of range rejected, stale-target idempotency), and the SDK is told the action does not exist.
Fix: the awsQuery operation dispatch/version table of services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10; the per-member range/length validation and the ip-target VPC checks are what the handlers must then enforce.
Traces: the 8 'ec2__elbv2.*' check ids in trace_ids.

## Evidence
All 8 checks — botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the CreateTargetGroup operation: Could not find operation CreateTargetGroup for version 2015-12-01, raised on the first ELBv2 call so nothing downstream was exercised.
Expected per Smithy: HTTP 200 with TargetGroups, or HTTP 400 with a modeled awsQuery code (DuplicateTargetGroupName / InvalidConfigurationRequest / TooManyTargetGroups / TooManyTags), and a ValidationError for an out-of-range health-check port.

## Steps to Reproduce
1. Boot the emulator.
2. boto3.client('elbv2').create_target_group(Name='sim2real-ip-tg', Protocol='HTTP', Port=80, VpcId=<vpc>, TargetType='ip').
3. Observe Error.Code 'InvalidAction' with message 'Could not find operation CreateTargetGroup for version 2015-12-01' instead of 200 + TargetGroups.
4. Re-replay the 8 check ids in trace_ids.
