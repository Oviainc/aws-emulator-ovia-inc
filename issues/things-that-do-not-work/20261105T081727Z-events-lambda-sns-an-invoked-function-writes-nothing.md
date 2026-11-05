Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.iam-role-trusts-lambda-service-assumed events__lambda__sns.lambda-role-missing-events-permission-putevents-denied
Issue-Key: events__lambda__sns.iam-role-trusts-lambda-service-assumed|_poll_for_report_line#25|Timeout|time.sleep(4)
Issue-Key: events__lambda__sns.lambda-role-missing-events-permission-putevents-denied|_invoke_with_retry#17|Timeout|time.sleep(5)
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# events__lambda__sns: an invoked function writes nothing to its own /aws/lambda log group (no START/END/REPORT line), and a handler that imports boto3 cannot call EventBridge at all

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue bundles 2 net-new root causes. Both must be closed before submitting.

Cause 1 of 2: a successful RequestResponse Invoke leaves the function's own CloudWatch Logs group with no invocation record, so nothing can ever be read back from it.

AWS: the Lambda runtime writes every invocation's log to the function's log group `/aws/lambda/<function-name>` — the group is created on first invocation when the execution role allows logs:CreateLogGroup/CreateLogStream/PutLogEvents, a log stream is opened per execution environment, and each invocation contributes a `START RequestId: <id> Version: <version>` line, anything the handler wrote to stdout, an `END RequestId: <id>` line and a `REPORT RequestId: <id>\tDuration: … ms\tBilled Duration: … ms\tMemory Size: … MB\tMax Memory Used: … MB` line. The fixed text here is the `START`/`END`/`REPORT RequestId:` prefixes and the `Duration`/`Billed Duration`/`Memory Size`/`Max Memory Used` field names; the ids, durations and sizes are per-invocation values. A FilterLogEvents on that group with filterPattern `REPORT` therefore returns at least one event shortly after the invocation. Recorded behavior: the check's Invoke returns 200 with the expected payload, then the REPORT line becomes readable.

Emulator: Invoke runs the handler and answers correctly, but polling FilterLogEvents on `/aws/lambda/<function-name>` for `REPORT` returns no events for the whole 90s poll (the check's 60s budget expires first) — either the group is never created (ResourceNotFoundException) or it stays empty. Nothing the runtime emits about an invocation is observable through CloudWatch Logs.

Why: a client relying on AWS's behavior cannot observe or debug an invocation: `aws logs tail`, log-driven alarms, metric filters over Lambda output and any "invoke then read the log" workflow see an empty or absent group, and a role whose logs permissions are wrong is indistinguishable from one that is right because neither produces a line.

Fix: the Lambda Invoke execution path's log emission — services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs together with src/state/execution (the run's captured log) must write the run's log through to the CloudWatch Logs state store under `/aws/lambda/<function-name>`, creating the group and stream and emitting the START/END/REPORT framing around the handler's own output.

Traces: events__lambda__sns.iam-role-trusts-lambda-service-assumed.

Cause 2 of 2: a python3.x handler that imports the AWS SDK never runs, so a function cannot call EventBridge (or any AWS service) and the permission-denied path it should take is replaced by an import crash.

AWS: the managed Python runtimes bundle boto3/botocore in the runtime image, so `import boto3` resolves for a package containing only the handler module, and the runtime injects the execution role's credentials. With a role that lacks events:PutEvents, the in-handler PutEvents raises a botocore ClientError carrying an AccessDenied-class code and message, which the handler catches and reports in its own payload; the Invoke itself still answers 200 with no `x-amz-function-error`.

Emulator: Invoke answers 200 with `FunctionError: Unhandled` and the payload `{"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<uuid>", "stackTrace": []}` (fixed text `Unable to import module '`…`': No module named 'boto3'` and `Runtime.ImportModuleError`; variable parts the handler module name from the request's `Handler` and the per-invocation requestId), so the check never observes a denial and exhausts its budget. The guest interpreter registers an allow-list of modules with no socket layer and no boto3/botocore on the import path.

Why: no emulated function can call an AWS service, and a negative IAM test cannot tell "the call was refused" from "the call was never made".

Fix: the Lambda execution engine's guest environment — services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/python.rs (guest module allow-list and import path) and src/state/execution/bootstrap.rs — must expose an AWS SDK surface that dispatches to the emulator's own front doors under the function's execution-role identity. The same cause is filed under the lambda__sns__sqs and events__lambda__sns__sqs headings from this sweep.

Traces: events__lambda__sns.lambda-role-missing-events-permission-putevents-denied.

## Evidence
Cause 1 — events__lambda__sns.iam-role-trusts-lambda-service-assumed: `Error: Check timed out after 60.0s`. The check's Invoke of a trivial `def handler(event, context): return {'ok': True, 'marker': ...}` succeeds, then it polls `logs.filter_log_events(logGroupName='/aws/lambda/<fn>', filterPattern='REPORT')` for up to 90s and never sees an event; its own assertion text is "no REPORT line ever appeared in the function's own log group". The same absence is visible under the cloudwatch__lambda__logs__sns__sqs heading, where the harness reports: TimeoutError: a printed line reaching log group /aws/lambda/<fn>: the run budget ended first; not graded.
Expected (AWS): at least one event matching REPORT in /aws/lambda/<fn> within seconds of the invocation.

Cause 2 — events__lambda__sns.lambda-role-missing-events-permission-putevents-denied: `Error: Check timed out after 60.0s`. Its handler begins `import json` / `import boto3` and then calls events.put_events; on this emulator such a handler answers 200 with FunctionError=Unhandled and payload {'errorMessage': "Unable to import module '<module>': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'stackTrace': []} (identical text captured verbatim on the lambda__sns__sqs checks), so the expected AccessDenied-bearing payload never appears.

## Steps to Reproduce
Cause 1: CreateRole trusting lambda.amazonaws.com with an inline policy allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents; CreateFunction (Runtime=python3.12, Handler=index.handler) with a handler that returns a dict; Invoke it (RequestResponse); then call FilterLogEvents on /aws/lambda/<function-name> with filterPattern='REPORT'. AWS returns the REPORT event; the emulator returns none (or ResourceNotFoundException for the group).
Cause 2: same function but with a handler whose first lines are `import boto3` and which calls events.put_events, under a role WITHOUT events:PutEvents; Invoke and read the payload. AWS returns 200/no FunctionError with the handler-reported AccessDenied; the emulator returns 200 with FunctionError=Unhandled and Runtime.ImportModuleError.
Replay both checks in trace_ids.
