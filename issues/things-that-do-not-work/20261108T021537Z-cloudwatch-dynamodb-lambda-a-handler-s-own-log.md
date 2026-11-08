Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.lambda-ddb-accessdenied-logged-to-cloudwatch-negative
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-accessdenied-logged-to-cloudwatch-negative|_poll_until#14|AssertionError|raise AssertionError(f"{description} not satisfied within {timeout_s}s, last value={last!r}")
Issue-Commit: 43c7d03c349635b4ef0fa0dfa9d9ce4b91e8a4c8

# cloudwatch__dynamodb__lambda: a handler's own log output never reaches /aws/lambda/<fn> — the AccessDenied boundary check sees only START/END/REPORT

> _Note: this issue was identified against the code base at commit `43c7d03c349635b4ef0fa0dfa9d9ce4b91e8a4c8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the Lambda invoke path does not capture handler stdout/stderr into the function's CloudWatch Logs stream, so nothing the handler logs is ever observable. Failing check: cloudwatch__dynamodb__lambda.lambda-ddb-accessdenied-logged-to-cloudwatch-negative.

AWS: recorded behavior only for the log content (the Lambda Smithy model describes only the Invoke envelope — LogType/LogResult — and says nothing about log-stream contents). AWS's Lambda runtime writes everything the handler emits on stdout/stderr (including Python `logging` output) as log events into the function's log group /aws/lambda/<function-name>, interleaved between the runtime's own platform lines `START RequestId: <id> Version: <version>`, `END RequestId: <id>` and `REPORT RequestId: <id>\tDuration: <ms> ms\tBilled Duration: <ms> ms\tMemory Size: <n> MB\tMax Memory Used: <n> MB`. In those lines the quoted words are the fixed text and the request id, version, durations and memory figures are per-invocation values. A handler that catches a DynamoDB AccessDeniedException and logs a diagnostic line therefore has that line returned by logs GetLogEvents/FilterLogEvents on the stream. The invocation itself is a 2xx with no FunctionError, which the emulator now gets right: the denied PutItem is correctly refused AccessDeniedException with the expected message and writes no item, so the divergence is purely the missing log delivery.

Emulator: the invocation runs and the platform lines are delivered, but the stream carries ONLY those lines — the handler's `PUT_DENIED code=AccessDeniedException pk=<pk>` line is absent, and the check polls FilterLogEvents for 90s before failing with `AssertionError: PUT_DENIED entry in <function-name> logs not satisfied within 90s, last value='START RequestId: … Version: $LATEST\nEND RequestId: …\nREPORT RequestId: …\tDuration: … ms\tBilled Duration: … ms\tMemory Size: 128 MB'`. Note the REPORT line also omits the `Max Memory Used:` field AWS appends.

Why: a client can never see what its own function logged. Application logging is the primary Lambda debugging channel, so every log-grounded expectation — permission-boundary diagnostics, caught-exception traces, structured logs, EMF metric lines parsed by metric filters — silently yields nothing even though the invocation succeeded and the refusal happened exactly as AWS would refuse it.

Fix: the Lambda in-process runtime's log capture and its delivery into CloudWatch Logs — handler stdout/stderr must be captured per invocation and emitted as log events into the function's /aws/lambda/<function-name> stream alongside the platform START/END/REPORT lines.

Related open issues that do NOT carry this check's replay key, so none of them covers it: 20261107T173512Z-cloudwatch-dynamodb-lambda-a-function-s-own-log.md (same root cause, grounded only in lambda-ddb-table-not-exist-error-logged-cloudwatch), 20261107T135310Z-cloudwatch-dynamodb-lambda-the-missing-logs-permission-boundary.md and 20261105T111951Z-cloudwatch-dynamodb-lambda-the-lambda-python-runtime-has.md (whose key for this check is the now-passing `lambda invocation failed`/boto3-import failure — the runtime now imports boto3 and the call is refused correctly; the check fails later, at the log poll).

Traces: cloudwatch__dynamodb__lambda.lambda-ddb-accessdenied-logged-to-cloudwatch-negative.

## Evidence
Expected: after invoking the denied function, its /aws/lambda/<function-name> stream contains the handler's `PUT_DENIED code=AccessDeniedException pk=<pk>` line between the START and END platform lines (the pk and request id are per-run values; the requirement is that handler output appears at all, with AWS's platform lines around it, REPORT including Max Memory Used).

Actual: `AssertionError: PUT_DENIED entry in sim2real-cwlambdadb-fn-<suffix> logs not satisfied within 90s, last value='START RequestId: <id> Version: $LATEST\nEND RequestId: <id>\nREPORT RequestId: <id>\tDuration: 2509.57 ms\tBilled Duration: 2510 ms\tMemory Size: 128 MB'` — runtime platform lines only, no handler output, and no Max Memory Used field. The preceding assertions passed: the denied invoke returned ok=false with code AccessDeniedException and the expected message, and GetItem confirmed no item was written.

## Steps to Reproduce
1. CreateTable; create two IAM roles, one with no dynamodb:PutItem grant and one with it; CreateFunction (python3.x) for each with a handler that calls ddb.put_item and logs `PUT_OK pk=%s` on success or `PUT_DENIED code=%s pk=%s` from the caught ClientError.
2. Invoke the denied function — it correctly answers ok=false / AccessDeniedException.
3. Poll logs FilterLogEvents (or GetLogEvents) on /aws/lambda/<function-name> for the PUT_DENIED line. Observe only START/END/REPORT for up to 90s.
4. Re-replay cloudwatch__dynamodb__lambda.lambda-ddb-accessdenied-logged-to-cloudwatch-negative to confirm a fix.
