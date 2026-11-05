Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3__sqs.declared-log-group-receives-while-default-group-is-never-created
Issue-Key: ec2__lambda__s3__sqs.declared-log-group-receives-while-default-group-is-never-created|_poll#14|Timeout|time.sleep(interval_s)
Issue-Commit: 506f51337b02a9b5673b5526bcfdc40d4e5c4821

# The declared-LoggingConfig.LogGroup routing scenario never reaches a verdict: the invoke/log-routing path stalls past the harness budget

> _Note: this issue was identified against the code base at commit `506f51337b02a9b5673b5526bcfdc40d4e5c4821`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause.

Cause 1 of 1: the LoggingConfig log-routing lifecycle stalls — the check is cut off at the harness budget (60s) with no response observed, so neither the declared-group routing nor the never-created default group can be verified.

AWS: a function created with LoggingConfig={LogFormat: 'Text', LogGroup: <pre-created group>} returns that LoggingConfig from GetFunctionConfiguration, Invoke answers promptly with StatusCode 200 and no FunctionError, its output lands in the DECLARED group (whose pre-set retentionInDays is honoured), and /aws/lambda/<name> is never created; a sibling function with no declaration creates and fills its default group, which carries no retention policy of its own. Recorded behavior only for timing: the recording completes this lifecycle inside the check's budget. Smithy (services/service-com-amazonaws-lambda-awsgirapiservice): CreateFunction/GetFunctionConfiguration model LoggingConfig.LogGroup and Invoke is synchronous — no asynchronous settle contract is modelled.

Emulator: no status, error code or payload is observed for the scenario; some call on the create→invoke→log-read path does not return, or its polled condition never becomes true, inside the budget that sufficed for the recording.

Why: a client that routes function logs to its own group gets no answer at all, and cannot tell whether routing, retention or default-group suppression works.

Fix: the lambda Invoke path and the log delivery it drives into the cloudwatchlogs service (including the LoggingConfig.LogGroup routing decision) must settle to an observable result; a call that does not return must answer, and an unreachable condition must be reported rather than polled forever.

Traces: ec2__lambda__s3__sqs.declared-log-group-receives-while-default-group-is-never-created. Open issues 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md and 20261105T022225Z-lambda-sesv2-lambda-invoke-refuses-every-real-invocation.md describe Invoke failing fast for other reasons; neither carries this check's replay key, and this failure is a stall, not a refusal.

## Evidence
Expected: lam.invoke returns StatusCode 200 with no FunctionError, GetFunctionConfiguration echoes LoggingConfig.LogGroup = '/sim2real/scn/<name>', that group receives the marker and keeps retentionInDays=1, and /aws/lambda/<function> is never created.
Actual (emulator): "Error: Check timed out after 60.0s" — no HTTP status, no error code and no payload is reported; the check never reaches its first assertion.

## Steps to Reproduce
1. logs CreateLogGroup '/sim2real/scn/<name>' and PutRetentionPolicy(retentionInDays=1).
2. Create a python3.x Lambda function with LoggingConfig={LogFormat: 'Text', LogGroup: '<that group>'} and a role allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents.
3. Invoke it with a marker payload, read the declared group's events, and check that /aws/lambda/<function> does not exist; then create a sibling with no LoggingConfig and repeat.
4. Expected: every call answers; the declared group receives the marker and keeps retention 1, the default group is never created. Observed: the scenario is cut off at the 60s harness budget with no verdict.
