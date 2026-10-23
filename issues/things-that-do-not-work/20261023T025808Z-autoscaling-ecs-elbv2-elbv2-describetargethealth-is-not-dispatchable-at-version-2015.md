Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn
Issue-Key: elastic-load-balancing-v2|DescribeTargetHealth|InvalidAction
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# ELBv2 DescribeTargetHealth is not dispatchable at version 2015-12-01, so a CFN-deployed ECS service's ALB target health can never be read

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
One root cause, grounded in 1 failing behavioral check whose id's leading token is the composite 'autoscaling__ecs__elbv2'. The already-open issue 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md names only CreateLoadBalancer under the 'acm__elbv2' token and does not carry this replay key.

Cause 1 of 1: the ELBv2 awsQuery action table does not resolve DescribeTargetHealth at API version 2015-12-01.
AWS: Smithy com.amazonaws.elasticloadbalancingv2#DescribeTargetHealth (service version 2015-12-01, protocol awsQuery) takes TargetGroupArn (required), Targets, Include and returns {TargetHealthDescriptions}; its only modeled errors are TargetGroupNotFound (awsQuery code TargetGroupNotFound, HTTP 400), InvalidTarget (400) and HealthUnavailable (500). A well-formed Action=DescribeTargetHealth&Version=2015-12-01 POST is served 200 or refused with one of those three codes. 'InvalidAction' is reserved for an action the service does not have.
Emulator: the call is refused with Error.Code 'InvalidAction' and message 'Could not find operation DescribeTargetHealth for version 2015-12-01'.
Why: target health is the only way a client can tell whether an ECS service's tasks were registered behind an ALB and became healthy; with the read unreachable, every deploy-then-verify workflow is blind and the failure is misreported to the SDK as a nonexistent action.
Fix: the awsQuery operation dispatch/version table of services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10 (register DescribeTargetHealth and the rest of the 2015-12-01 action set).
Traces: autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn.

## Evidence
autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn — botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the DescribeTargetHealth operation: Could not find operation DescribeTargetHealth for version 2015-12-01.
Expected per Smithy: HTTP 200 with TargetHealthDescriptions, or HTTP 400 TargetGroupNotFound / InvalidTarget, or HTTP 500 HealthUnavailable.

## Steps to Reproduce
1. Boot the emulator.
2. boto3.client('elbv2').describe_target_health(TargetGroupArn='arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/sim2real-tg/0123456789abcdef').
3. Observe Error.Code 'InvalidAction' with message 'Could not find operation DescribeTargetHealth for version 2015-12-01' instead of 200 + TargetHealthDescriptions (or 400 TargetGroupNotFound).
4. Re-replay autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn.
