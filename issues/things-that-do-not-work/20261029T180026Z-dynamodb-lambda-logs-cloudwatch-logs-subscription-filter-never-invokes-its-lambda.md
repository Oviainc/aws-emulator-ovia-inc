Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__logs.logs-subscription-filter-invokes-lambda-across-boundary
Issue-Key: dynamodb__lambda__logs.logs-subscription-filter-invokes-lambda-across-boundary|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# CloudWatch Logs subscription filter never invokes its Lambda destination

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
One net-new root cause in the dynamodb__lambda__logs group.

Cause 1 of 1: a log-group subscription filter whose destination is a Lambda function never delivers.
AWS: events written to a log group that has a subscription filter are delivered (gzipped, base64 `awslogs.data`) to the destination function, including across a log-group/function boundary, and the function runs.
Emulator: nothing is delivered; the check polls for the function's effect and ends with `Check timed out after 60.0s`.
Why: log-driven processing (alerting, forwarding, redaction) is silently inert — PutLogEvents is accepted and the subscriber never runs, so a caller cannot tell a filter that does not match from one that is never evaluated.
Fix: the cloudwatchlogs subscription-filter delivery path (and the lambda invoke it performs).
Traces: dynamodb__lambda__logs.logs-subscription-filter-invokes-lambda-across-boundary

## Evidence
[mismatch] dynamodb__lambda__logs.logs-subscription-filter-invokes-lambda-across-boundary — Error: Check timed out after 60.0s.

## Steps to Reproduce
Boot the emulator and replay dynamodb__lambda__logs.logs-subscription-filter-invokes-lambda-across-boundary: it creates a log group with a Lambda subscription filter, writes log events, and polls for the function's effect.
