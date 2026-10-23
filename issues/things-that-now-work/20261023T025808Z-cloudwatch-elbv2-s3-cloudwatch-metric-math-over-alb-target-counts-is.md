Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts
Issue-Key: cloudwatch__elbv2__s3|CreateTargetGroup|InvalidAction
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# CloudWatch metric-math over ALB target counts is unreachable: ELBv2 CreateTargetGroup answers InvalidAction at version 2015-12-01

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
One root cause, grounded in 1 failing behavioral check whose id's leading token is the composite 'cloudwatch__elbv2__s3'. The already-open issue 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md names only CreateLoadBalancer under the 'acm__elbv2' token and does not carry this replay key.

Cause 1 of 1: the ELBv2 awsQuery action table does not resolve CreateTargetGroup at API version 2015-12-01.
AWS: Smithy com.amazonaws.elasticloadbalancingv2#CreateTargetGroup (service version 2015-12-01, protocol awsQuery) returns {TargetGroups} and is otherwise refused with one of its modeled awsQuery errors DuplicateTargetGroupName / InvalidConfigurationRequest / TooManyTargetGroups / TooManyTags (all HTTP 400). 'InvalidAction' is reserved for an action the service does not have.
Emulator: the call is refused with Error.Code 'InvalidAction' and message 'Could not find operation CreateTargetGroup for version 2015-12-01', so no target group exists to emit HealthyHostCount/RequestCount/HTTPCode_Target_5XX_Count metrics and the metric-math 5xx-rate expression has nothing to derive from.
Why: a client cannot build any ALB observability on top of the emulator, and the refusal misreports a well-formed request as a nonexistent action.
Fix: the awsQuery operation dispatch/version table of services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.
Traces: cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts.

## Evidence
cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts — botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the CreateTargetGroup operation: Could not find operation CreateTargetGroup for version 2015-12-01.
Expected: HTTP 200 with TargetGroups, or HTTP 400 with a modeled awsQuery code.

## Steps to Reproduce
1. Boot the emulator.
2. boto3.client('elbv2').create_target_group(Name='sim2real-tg', Protocol='HTTP', Port=80, VpcId=<vpc>, TargetType='ip').
3. Observe Error.Code 'InvalidAction' / 'Could not find operation CreateTargetGroup for version 2015-12-01'.
4. Re-replay cloudwatch__elbv2__s3.metric-math-5xx-rate-derives-from-alb-target-counts.
