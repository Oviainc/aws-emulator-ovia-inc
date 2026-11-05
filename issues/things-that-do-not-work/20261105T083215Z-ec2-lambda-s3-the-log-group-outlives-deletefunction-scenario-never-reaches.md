Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3.lambda-log-group-outlives-delete-function
Issue-Key: ec2__lambda__s3.lambda-log-group-outlives-delete-function|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: 506f51337b02a9b5673b5526bcfdc40d4e5c4821

# The log-group-outlives-DeleteFunction scenario never reaches a verdict: the Lambda log-routing path stalls past the harness budget

> _Note: this issue was identified against the code base at commit `506f51337b02a9b5673b5526bcfdc40d4e5c4821`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause.

Cause 1 of 1: the lambda log-routing lifecycle stalls — the check is cut off at the harness budget (60s) with no response observed, so the claim that a function's CloudWatch log group survives DeleteFunction is unverifiable.

AWS: the function writes to its log group, Invoke returns promptly with StatusCode 200, the group's streams become readable through logs, and DeleteFunction removes only the function — /aws/lambda/<name> and its retained events outlive it, so a later DescribeLogGroups/GetLogEvents still finds them. Recorded behavior only for timing: the recording completes this whole lifecycle inside the check's budget. Smithy (services/service-com-amazonaws-lambda-awsgirapiservice, services/service-com-amazonaws-cloudwatchlogs-logs-20140328): Invoke, DeleteFunction, DescribeLogGroups and GetLogEvents are synchronous modelled operations with no asynchronous settle contract.

Emulator: the scenario produces no verdict at all — no status, error code or payload is observed. Some call on the invoke→log-delivery→delete path does not return, or its polled condition never becomes true, within the budget that sufficed for the recording.

Why: a client doing the ordinary thing — invoke a function, then read its logs — hangs instead of getting an answer, and log retention across DeleteFunction cannot be relied on in either direction.

Fix: the lambda Invoke path and the log delivery it drives into the cloudwatchlogs service (plus the state the lambda DeleteFunction handler leaves behind) must settle to an observable result; a call that does not return must answer, and a condition that cannot be reached must be reported rather than polled forever.

Traces: ec2__lambda__s3.lambda-log-group-outlives-delete-function. Open issues 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md and 20261105T022225Z-lambda-sesv2-lambda-invoke-refuses-every-real-invocation.md describe Invoke failing fast for other causes; neither carries this check's replay key, and this failure is a stall, not a refusal.

## Evidence
Expected: every call in the invoke → read-logs → DeleteFunction → re-read-logs sequence answers, and the log group with its marker event outlives the deleted function.
Actual (emulator): "Error: Check timed out after 60.0s" — the check reports no HTTP status, no error code and no payload; it never reaches an assertion, so the stall is the only observable.

## Steps to Reproduce
1. Create an IAM role allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents and a python3.x Lambda function that writes one marker line.
2. Invoke the function; poll CloudWatch Logs for /aws/lambda/<name> and the marker event.
3. DeleteFunction, then re-read the log group and its events.
4. Expected: each call answers and the log group (with the marker) outlives the function. Observed: the scenario is cut off at the 60s harness budget with no verdict.
