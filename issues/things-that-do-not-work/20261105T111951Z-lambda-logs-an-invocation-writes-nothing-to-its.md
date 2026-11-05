Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs.async-invocation-withholds-handler-error-from-the-response lambda__logs.handler-traceback-routes-to-declared-group-leaving-default-absent lambda__logs.log-cwl-custom-log-group-shared-by-two-functions-keeps-streams-apart lambda__logs.log-cwl-log-group-update-redirects-the-next-invocation lambda__logs.log-cwl-missing-custom-log-group-is-created-on-first-invoke lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group lambda__logs.log-cwl-self-structured-json-lines-are-filtered-by-their-level-key lambda__logs.log-cwl-system-debug-level-delivers-runtime-done-and-init-records
Issue-Key: lambda__logs.async-invocation-withholds-handler-error-from-the-response|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: lambda__logs.handler-traceback-routes-to-declared-group-leaving-default-absent|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: lambda__logs.log-cwl-custom-log-group-shared-by-two-functions-keeps-streams-apart|_invoke_tail#9|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda__logs.log-cwl-log-group-update-redirects-the-next-invocation|_invoke_tail#7|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda__logs.log-cwl-missing-custom-log-group-is-created-on-first-invoke|_invoke_tail#9|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group|_invoke_tail#9|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda__logs.log-cwl-self-structured-json-lines-are-filtered-by-their-level-key|_invoke_tail#7|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda__logs.log-cwl-system-debug-level-delivers-runtime-done-and-init-records|_invoke_tail#6|KeyError|rid = resp["ResponseMetadata"]["RequestId"]
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# lambda__logs: an invocation writes nothing to its CloudWatch log group, and a custom LogGroup is never created or honoured

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 2 — an invocation creates no log group and writes no log events, so every log-backed check either never settles or fails at its first read.
AWS: the first invocation of a function creates its log group (the default /aws/lambda/<function-name>, or the one named by LoggingConfig.LogGroup), opens a stream named <YYYY/MM/DD>/[<version>]<hex>, and writes START/END/REPORT framing plus the handler's stdout/stderr and tracebacks; FilterLogEvents/GetLogEvents expose those lines within seconds, each function in a shared custom group gets its own stream, a LoggingConfig update redirects the next invocation, and when the execution role lacks logs:CreateLogGroup/PutLogEvents no group is created at all while LogType=Tail still returns the tail.
Emulator: recorded behavior only — nothing is delivered: the declared group stays absent, no stream or event appears, so the checks report "Check timed out after 60.0s" or fail at the point where they read the group.
Why: a client cannot observe an invocation's output, its handler traceback, its system-level records or its structured JSON levels, and log-based alerting and cost controls (retention, custom groups) are unverifiable.
Fix: the Lambda->CloudWatch Logs delivery path in the invoke path: implicit group creation (default and LoggingConfig.LogGroup), per-function stream naming, and START/END/REPORT plus application-record emission with the configured log format and levels.
Traces: lambda__logs.async-invocation-withholds-handler-error-from-the-response, .handler-traceback-routes-to-declared-group-leaving-default-absent, .log-cwl-custom-log-group-shared-by-two-functions-keeps-streams-apart, .log-cwl-log-group-update-redirects-the-next-invocation, .log-cwl-missing-custom-log-group-is-created-on-first-invoke, .log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group, .log-cwl-self-structured-json-lines-are-filtered-by-their-level-key, .log-cwl-system-debug-level-delivers-runtime-done-and-init-records.

Cause 2 of 2 — an asynchronous invocation's handler error is not withheld from the response as AWS does, because the invocation's outcome is unobservable at all.
AWS: Invoke with InvocationType=Event returns 202 with an empty payload and no FunctionError even when the handler raises; the error is only visible in the function's logs and metrics.
Emulator: recorded behavior only — the check never reaches that comparison because the logs that carry the error never arrive, so the harness reports "Check timed out after 60.0s".
Why: a client cannot confirm that async failures are reported out-of-band, so a failing async workload looks identical to a succeeding one.
Fix: the same log-delivery path, plus the Event-invocation response contract in the Invoke handler.
Traces: lambda__logs.async-invocation-withholds-handler-error-from-the-response.

## Evidence
Error: Check timed out after 60.0s for lambda__logs.async-invocation-withholds-handler-error-from-the-response and .handler-traceback-routes-to-declared-group-leaving-default-absent; the log-cwl-* checks fail at the point where they read the group/stream. Expected: the group exists after the first invoke, holds a dated stream per function, and carries START/END/REPORT plus the handler's lines; with no logs permission, no group exists but LogType=Tail still returns the tail.

## Steps to Reproduce
1. CreateFunction with LoggingConfig.LogGroup naming a group that does not exist; Invoke; DescribeLogGroups/DescribeLogStreams/FilterLogEvents for it.
2. Point two functions at one custom group and compare their streams; update LoggingConfig and invoke again.
3. Strip logs:CreateLogGroup/PutLogEvents from the role, Invoke with LogType=Tail, and check that no group exists but the tail is returned.
4. Invoke with InvocationType=Event a handler that raises; expect 202, empty payload, no FunctionError, and the error only in the logs.
