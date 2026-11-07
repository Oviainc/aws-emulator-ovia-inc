Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.deploy-cfn-iam-role-trust-events-service-cloudfront-target cloudfront__events.cfn-deploy-iam-role-mistrust-blocks-rule-target
Issue-Key: cloudfront__events.deploy-cfn-iam-role-trust-events-service-cloudfront-target|run#26|AssertionError|assert final["status"] == "SUCCEEDED", (
Issue-Key: cloudfront__events.cfn-deploy-iam-role-mistrust-blocks-rule-target|_await_good_execution#12|Timeout|matches = [e for e in _new_executions(good_arn) if e["executionArn"] not in seen]
Issue-Commit: 69fbb491696def14b5ee7dd45913958907d046a9

# cloudfront__events: the in-Lambda AWS SDK has no CloudFront client (UnknownServiceError), and a CloudFront CloudTrail-derived event never reaches the us-east-1 default bus (2 causes)

> _Note: this issue was identified against the code base at commit `69fbb491696def14b5ee7dd45913958907d046a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two net-new root causes in the CloudFront/EventBridge grouping. Both are net-new: neither check was reported as owned by an open issue.

=== Cause 1 of 2: the Lambda runtime's bundled AWS SDK registers only four services, so boto3.client('cloudfront') is refused UnknownServiceError. Failing check: cloudfront__events.deploy-cfn-iam-role-trust-events-service-cloudfront-target ===

AWS: Lambda's managed python3.x runtimes ship the AWS SDK for Python with a client for EVERY AWS service, so `boto3.client('cloudfront')` resolves and `list_distributions()` answers normally. Smithy (cloudfront, ListDistributions): the response carries DistributionList with a required integer member Quantity, which is exactly what the handler returns. botocore raises UnknownServiceError only for a name that is not an AWS service at all; `cloudfront` is one. A synchronous/async invoke of that handler therefore answers HTTP 200 with no X-Amz-Function-Error, and the Step Functions execution driven by it reaches SUCCEEDED carrying the integer distribution count.

Emulator: the in-function SDK shim refuses the call. The handler faults with errorType `UnknownServiceError` and errorMessage `Unknown service: 'cloudfront'. Valid service names are: sqs, events, eventbridge, dynamodb` — fixed text `Unknown service: '` + `'. Valid service names are: ` with the requested service name from the handler's boto3.client(...) call and, after it, the shim's hard-coded four-name registry. The stack trace shows botocore/client.py `_raise_refusal`. The driving Step Functions execution then ends FAILED with `UnknownServiceError` instead of SUCCEEDED.

Why: any tenant handler that calls a service outside {sqs, events, eventbridge, dynamodb} cannot run at all, and the fault is reported as if the tenant named a non-existent AWS service. Every "EventBridge rule -> Lambda -> <some service>" reference pattern is unusable, and a negative permission check cannot be distinguished from a missing client.

Fix: the service registry of the AWS SDK shim the Lambda invoke runtime exposes to handlers must cover every bootstrapped service, not four (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the python runtime bundle's botocore client factory it loads). Related open issue 20261105T094343Z-cloudfront-events-the-lambda-python-runtime-provides-no.md describes an earlier form of the same gap (no boto3 at all, Runtime.ImportModuleError) and does NOT carry this check's replay key; this is the surviving, narrower form.

Traces: cloudfront__events.deploy-cfn-iam-role-trust-events-service-cloudfront-target.

=== Cause 2 of 2: a matched CloudFront CloudTrail-derived event never starts the correctly-trusted rule target's execution. Failing check: cloudfront__events.cfn-deploy-iam-role-mistrust-blocks-rule-target ===

AWS: CloudFront is a global service — CloudTrail records its management API calls in us-east-1 and the derived envelopes (source `aws.cloudfront`, detail-type `AWS API Call via CloudTrail`) are delivered to the us-east-1 default bus while a trail is logging there. A rule with two Step Functions targets behaves asymmetrically by role trust: the target whose RoleArn trust policy holds an Allow sts:AssumeRole for events.amazonaws.com is assumed and its state machine executes; the sibling target whose trust policy names a different principal (here ec2.amazonaws.com) is never assumed and its state machine never executes. recorded behavior only for the exact detail body.

Emulator: after StartLogging in us-east-1, the CFN deploy of both roles and both state machines, and a CloudFront API call to trigger the event, NEITHER state machine ever executes — the positive control is as silent as the negative — and the check exhausts the harness budget: `Check timed out after 60.0s`. Hypothesis: the timeout line carries no further diagnosis; the absence of the positive control is consistent with CloudFront calls producing no CloudTrail-derived EventBridge envelope (see open issues 20261025T002357Z-cloudfront-events-cloudfront-read-only-calls-emit-no-events-and.md and 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md, neither of which carries this check's replay key).

Why: a client cannot tell a correctly-refused mistrusted target role from a pipeline that delivers nothing to either target, so the whole deployment-order guarantee is unverifiable and a real trust misconfiguration looks identical to a working stack.

Fix: the CloudTrail-to-EventBridge derivation for cloudfront management calls on the us-east-1 default bus, plus the EventBridge target role-trust enforcement that must still distinguish the two targets once events flow (the events target dispatcher and its sts:AssumeRole check).

Traces: cloudfront__events.cfn-deploy-iam-role-mistrust-blocks-rule-target.

## Evidence
Cause 1 — cloudfront__events.deploy-cfn-iam-role-trust-events-service-cloudfront-target
Expected (AWS): the Step Functions execution driven by the role reaches `SUCCEEDED` with the integer CloudFront distribution count read by the handler via cloudfront:ListDistributions.
Actual (emulator): `AssertionError: expected SUCCEEDED, got 'FAILED': UnknownServiceError/{"errorMessage":"Unknown service: 'cloudfront'. Valid service names are: sqs, events, eventbridge, dynamodb","errorType":"UnknownServiceError",...,"stackTrace":["  File \"/var/task/index.py\", line 5, in handler\n    resp = client.list_distributions()\n", "  File \"/var/task/botocore/client.py\", line 168, in _raise_refusal\n    raise UnknownServiceError(...)"]}`.
Fixed text of the diverging message: `Unknown service: '` ... `'. Valid service names are: `. Variable parts: the service name from the handler's boto3.client() call; the trailing list is the emulator's own four-entry registry (it is not an AWS value).

Cause 2 — cloudfront__events.cfn-deploy-iam-role-mistrust-blocks-rule-target
Expected (AWS): the correctly-trusted target's state machine has >=1 execution for the triggering CloudFront CloudTrail event; the mistrusted target's state machine has none.
Actual (emulator): `Error: Check timed out after 60.0s` — neither state machine ever has an execution; no aws.cloudfront "AWS API Call via CloudTrail" envelope is observed on the us-east-1 default bus.

## Steps to Reproduce
Cause 1:
1. CreateFunction (python3.x) whose index.py does `import boto3`, `client = boto3.client('cloudfront')`, `client.list_distributions()`.
2. Invoke it (directly, or as a Step Functions Task / EventBridge target).
3. Observe the handler fault `UnknownServiceError: Unknown service: 'cloudfront'. Valid service names are: sqs, events, eventbridge, dynamodb`, and the driving execution end FAILED.

Cause 2:
1. In us-east-1: CreateTrail + StartLogging.
2. Deploy a CFN stack with two IAM roles (one trusting events.amazonaws.com, one trusting ec2.amazonaws.com), both granted states:StartExecution, and two Pass-state state machines.
3. PutRule on {"source":["aws.cloudfront"],"detail-type":["AWS API Call via CloudTrail"]} with both state machines as targets, each with its own RoleArn.
4. Make a CloudFront API call and poll ListExecutions on both machines. Neither ever has an execution.
5. Re-replay the two checks in trace_ids to confirm.
