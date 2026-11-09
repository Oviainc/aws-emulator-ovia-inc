Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__stepfunctions.deploy-cdk-state-machine-lambda-invoke-denied-fails
Issue-Key: cloudformation__lambda__stepfunctions.deploy-cdk-state-machine-lambda-invoke-denied-fails|run#48|AssertionError|assert want_cause.fullmatch(negative.get("cause") or ""), (
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# A Step Functions Lambda task denied lambda:InvokeFunction reports a cause with the wrong service name and status code

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: when a state machine's Lambda task is denied lambda:InvokeFunction, the execution's failure cause does not carry Lambda's AccessDeniedException as AWS renders it.

AWS: a Task state whose role cannot invoke the function fails with error `Lambda.AccessDeniedException`, and the cause is Lambda's own AccessDeniedException text followed by the AWS SDK suffix: `User: <role ARN> is not authorized to perform: lambda:InvokeFunction on resource: <function ARN> because no identity-based policy allows the lambda:InvokeFunction action` (or, for an explicit deny, `... with an explicit deny in an identity-based policy`) followed by `(Service: Lambda, Status Code: 403, Request ID: <id>) (SDK Attempt Count: <n>)`. The fixed parts are the IAM sentence template and the parenthesised SDK suffix shape; the variable parts are the role ARN and function ARN from the state/request, the request id and the attempt count.

Emulator: emits the IAM sentence but closes it with `(Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)` — semicolon-separated, service name `AWSLambda` instead of `Lambda`, HTTP status 400 instead of 403, and an `Error Code`/`Proxy` pair AWS does not emit here, with no request id. Hypothesis: the cause is formatted by a legacy SDK-v1 style renderer rather than from the Lambda refusal's real status and request id.

Why: a client that parses the cause (to report the denied action, or to branch on status 403 versus a 400 validation failure) reads a 400 for an authorization denial and a service name that never appears in AWS causes, so retry and alarm logic keyed on the cause misclassifies the failure.

Fix: the Step Functions Lambda-integration error serializer that wraps a Lambda refusal into the execution's cause — take the status code, error code and request id from the Lambda response and render AWS's comma-separated `(Service: Lambda, Status Code: ..., Request ID: ...)` suffix.

Traces: cloudformation__lambda__stepfunctions.deploy-cdk-state-machine-lambda-invoke-denied-fails.

## Evidence
Check cloudformation__lambda__stepfunctions.deploy-cdk-state-machine-lambda-invoke-denied-fails:
AssertionError: the denial's cause is 'User: arn:aws:iam::<account>:role/<MachineRole> is not authorized to perform: lambda:InvokeFunction on resource: arn:aws:lambda:<region>:<account>:function:<fn> with an explicit deny in an identity-based policy (Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)', expected Lambda's AccessDeniedException for '<MachineRole>' on '<function ARN>' with an explicit deny
(role and function ARNs are per-run names; the divergence is the suffix format, the service name 'AWSLambda' and the 400 status.)

## Steps to Reproduce
1. Deploy a state machine whose role has an explicit deny on lambda:InvokeFunction for the target function.
2. StartExecution and wait for it to fail.
3. Read the execution's error/cause via DescribeExecution or GetExecutionHistory and compare the SDK suffix, service name and status code with AWS.
