Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.lambda-ddb-table-not-exist-error-logged-cloudwatch
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-table-not-exist-error-logged-cloudwatch|_poll_until#8|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# cloudwatch__dynamodb__lambda: a function's own log output never reaches its CloudWatch log stream — only START/END/REPORT lines are delivered

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: handler-produced log lines are dropped from the function's CloudWatch Logs stream.
AWS: the Lambda runtime writes everything the handler emits on stdout/stderr into the function's log stream in /aws/lambda/<function-name>, interleaved between the runtime's own `START RequestId: … Version: …`, `END RequestId: …` and `REPORT RequestId: … Duration: … Billed Duration: … Memory Size: … Max Memory Used: …` lines. A handler that catches a DynamoDB ResourceNotFoundException for a missing table and prints a diagnostic line therefore has that line visible via logs FilterLogEvents/GetLogEvents on the stream.
Emulator: the invocation runs (the REPORT line shows a 2182 ms duration) but the stream contains ONLY the three runtime lines — the handler's own output is absent, so the expected error line is never found: `GET_ERROR log line for missing table not met within 90s`.
Why: a client cannot observe anything its own function logged. Application logging is the primary debugging channel for Lambda, and every log-based assertion (error diagnostics, structured logs, EMF metric lines) silently yields nothing even though the invocation succeeded.
Fix: the lambda in-process runtime's log capture — handler stdout/stderr must be captured per invocation and emitted as log events into the function's /aws/lambda/<function-name> stream alongside the START/END/REPORT lines.
Traces: cloudwatch__dynamodb__lambda.lambda-ddb-table-not-exist-error-logged-cloudwatch. Related open issue that does not carry this check's replay key: 20261106T151148Z-cloudwatch-dynamodb-lambda-an-unpermitted-in-function-dynamodb.md.

## Evidence
Expected: the function's log stream contains the handler's diagnostic line for the missing table (the check looks for a GET_ERROR line) between START and END. Actual: `AssertionError: GET_ERROR log line for missing table not met within 90s. Last result: 'START RequestId: … Version: $LATEST\nEND RequestId: …\nREPORT RequestId: …\tDuration: 2182.21 ms\tBilled Duration: 2183 ms\tMemory Size: 128 MB'` — runtime lines only, no handler output. (The request id, duration and memory figures are per-run values; the requirement is that handler output appears at all.)

## Steps to Reproduce
Create a function whose handler performs a DynamoDB GetItem against a non-existent table, catches the error and prints a diagnostic line; Invoke it; then read /aws/lambda/<function-name> with logs FilterLogEvents. Observe only START/END/REPORT. Re-replay cloudwatch__dynamodb__lambda.lambda-ddb-table-not-exist-error-logged-cloudwatch.
