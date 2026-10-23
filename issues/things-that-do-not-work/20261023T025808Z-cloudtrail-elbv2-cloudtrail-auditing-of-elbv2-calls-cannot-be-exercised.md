Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__elbv2.obs-elbv2-3-idempotent-create-replay-is-audited-as-a-second-success-with-the-same-arn cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes
Issue-Key: cloudtrail__elbv2|CreateTargetGroup|InvalidAction
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# CloudTrail auditing of ELBv2 calls cannot be exercised: CreateTargetGroup is refused with InvalidAction at version 2015-12-01

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
One root cause, grounded in 2 failing behavioral checks whose id's leading token is the composite 'cloudtrail__elbv2'. The already-open issue 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md names only CreateLoadBalancer under the 'acm__elbv2' token and does not carry these replay keys.

Cause 1 of 1: the ELBv2 awsQuery action table does not resolve CreateTargetGroup at API version 2015-12-01, so no auditable ELBv2 control-plane call can be made.
AWS: Smithy com.amazonaws.elasticloadbalancingv2#CreateTargetGroup (service version 2015-12-01, protocol awsQuery, cloudTrailEventSource elasticloadbalancing.amazonaws.com) returns {TargetGroups} on success and is otherwise refused with one of its modeled awsQuery errors DuplicateTargetGroupName / InvalidConfigurationRequest / TooManyTargetGroups / TooManyTags (all HTTP 400). Both a success and a modeled refusal are recorded as a CloudTrail management event — a refusal carrying the exception-suffixed error code. 'InvalidAction' is reserved for an action the service does not have.
Emulator: the call is refused with Error.Code 'InvalidAction' and message 'Could not find operation CreateTargetGroup for version 2015-12-01', so neither the idempotent-replay audit record nor the refused-call audit record can be produced.
Why: a client that relies on CloudTrail to audit ELBv2 changes gets no event at all, and the API refusal misreports a well-formed request as a nonexistent action.
Fix: the awsQuery operation dispatch/version table of services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10; once dispatch works, the CloudTrail event emission for ELBv2 successes and modeled refusals is what these checks assert.
Traces: cloudtrail__elbv2.obs-elbv2-3-idempotent-create-replay-is-audited-as-a-second-success-with-the-same-arn, cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes.

## Evidence
Both checks — botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the CreateTargetGroup operation: Could not find operation CreateTargetGroup for version 2015-12-01.
Expected: HTTP 200 with TargetGroups (first and replayed create), and for the negative case HTTP 400 with a modeled awsQuery code such as DuplicateTargetGroupName; each call then audited by CloudTrail.

## Steps to Reproduce
1. Boot the emulator.
2. boto3.client('elbv2').create_target_group(Name='sim2real-tg', Protocol='HTTP', Port=80, VpcId=<vpc>) -> Error.Code 'InvalidAction', message 'Could not find operation CreateTargetGroup for version 2015-12-01'.
3. Re-replay both check ids in trace_ids.
