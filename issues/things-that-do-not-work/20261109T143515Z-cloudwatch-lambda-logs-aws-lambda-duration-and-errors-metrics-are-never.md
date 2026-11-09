Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.lambda-duration-metric-positive cloudwatch__lambda__logs.lambda-error-count-metric-positive cloudwatch__lambda__logs.cloudwatch-alarm-missing-lambda-permission-negative
Issue-Key: cloudwatch__lambda__logs.lambda-duration-metric-positive|run#7|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__lambda__logs.lambda-error-count-metric-positive|run#11|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__lambda__logs.cloudwatch-alarm-missing-lambda-permission-negative|_wait_for_dead_letter#21|Timeout|resp = sqs.receive_message(
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# AWS/Lambda Duration and Errors metrics are never published, and the alarm-without-invoke-permission check never settles (2 causes)

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 2: Lambda publishes no AWS/Lambda Duration or Errors datapoints for invocations that happened.
AWS: every invocation publishes AWS/Lambda Invocations, Duration (reflecting the handler's wall time) and, for a failing invocation, Errors, dimensioned by FunctionName.
Emulator: neither series ever appears — 'AWS/Lambda Duration metric did not appear (or never reflected the induced 1s sleep) within 300s poll window' and 'AWS/Lambda Errors metric did not appear within 300s poll window'.
Why: a client cannot alarm on function errors or latency at all; every Lambda alarm stays INSUFFICIENT_DATA while the function is plainly erroring.
Fix: the AWS/Lambda metric publisher in the Lambda invoke path (Invocations/Duration/Errors into the CloudWatch metric store).
Traces: cloudwatch__lambda__logs.lambda-duration-metric-positive, cloudwatch__lambda__logs.lambda-error-count-metric-positive.

Cause 2 of 2: the alarm-action-without-invoke-permission negative never reaches a verdict.
AWS: an alarm whose action names a function that has not granted lambda:InvokeFunction to cloudwatch/events makes no invocation, which the check observes well inside its budget.
Emulator: 'Check timed out after 180.0s' — nothing the check waits on (the alarm's state transition or the absence/presence of an invocation) ever happens.
Why: a client cannot tell a denied alarm action from an alarm that never evaluated.
Fix: alarm evaluation and alarm-action dispatch in the cloudwatch crate.
Traces: cloudwatch__lambda__logs.cloudwatch-alarm-missing-lambda-permission-negative.

## Evidence
Cause 1 — 'AssertionError: AWS/Lambda Duration metric did not appear (or never reflected the induced 1s sleep) within 300s poll window' and 'AssertionError: AWS/Lambda Errors metric did not appear within 300s poll window'.
Cause 2 — 'Error: Check timed out after 180.0s'.

## Steps to Reproduce
Replay cloudwatch__lambda__logs.lambda-duration-metric-positive, .lambda-error-count-metric-positive (invoke a function, then poll GetMetricStatistics for AWS/Lambda Duration/Errors) and .cloudwatch-alarm-missing-lambda-permission-negative.
