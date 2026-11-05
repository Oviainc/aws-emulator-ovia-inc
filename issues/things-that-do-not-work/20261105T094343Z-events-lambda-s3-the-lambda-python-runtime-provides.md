Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3.lambda-role-missing-s3-permission-denied events__lambda__s3.lambda-putevents-triggers-downstream-rule
Issue-Key: events__lambda__s3.lambda-role-missing-s3-permission-denied|run#8|KeyError|assert allowed["denied"] is False and allowed["ok"] is True, (
Issue-Key: events__lambda__s3.lambda-putevents-triggers-downstream-rule|invoke_and_receive#14|Timeout|resp = sqs.receive_message(
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# events__lambda__s3: the Lambda python runtime provides no AWS SDK, so a handler that imports boto3 returns an import error instead of its S3/PutEvents result

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the python3.x Lambda invoke runtime has no boto3/botocore on the handler's import path, so a function whose module imports the AWS SDK to call S3 or EventBridge fails at module import and its body never executes. Failing checks: events__lambda__s3.lambda-role-missing-s3-permission-denied, events__lambda__s3.lambda-putevents-triggers-downstream-rule.

AWS: AWS Lambda's managed python3.x runtimes ship the AWS SDK for Python (boto3/botocore) pre-installed on the function's sys.path, so `import boto3` at module scope succeeds and the handler can call S3 and EventBridge with its execution-role credentials. Smithy (lambda, Invoke): FunctionError is bound to the response header `X-Amz-Function-Error`, StatusCode to the HTTP response code; a successful synchronous Invoke answers HTTP 200 with NO `X-Amz-Function-Error` header and Payload equal to the handler's return value — here a JSON report carrying the handler's own keys (`ok`, `denied`). A handler whose S3 call is refused by IAM is expected to run and report that refusal (`denied: true`), not to fail at import.

Emulator: the Invoke answers HTTP 200 with `x-amz-function-error: Unhandled` and the payload is the runtime's own import failure: errorType `Runtime.ImportModuleError`, errorMessage `Unable to import module '<module>': No module named 'boto3'` — fixed text `Unable to import module '` + `': No module named 'boto3'`, with `<module>` taken from the function's Handler setting and requestId the per-invocation id. The handler's keys are absent, so events__lambda__s3.lambda-role-missing-s3-permission-denied dies with `KeyError: 'denied'`; events__lambda__s3.lambda-putevents-triggers-downstream-rule polls for the downstream rule's effect of a PutEvents the handler never made and ends with `Check timed out after 60.0s` (Hypothesis: same cause — it deploys the same shape of boto3-importing handler, and the timeout line carries no further diagnosis).

Why: a client whose function reads or writes S3, or re-publishes to EventBridge, gets a function that cannot run at all; the permission-boundary distinction (granted twin writes, denied twin is refused) becomes unobservable, and the emulator attributes the fault to the customer's code rather than to the absent SDK.

Fix: the Lambda invoke runtime/sandbox that backs the python3.x runtimes must expose the AWS SDK on the handler's import path (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the python runtime bundle it loads).

Related open issues describe the same runtime gap under other service groupings (20261105T081644Z-events-lambda-s3-stepfunctions-the-lambda-python-runtime.md, 20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md); the open events__lambda__s3 issues (e.g. 20261029T182228Z-events-lambda-s3-eventbridge-and-s3-notifications-never.md) do not carry the replay keys of these two checks.

Traces: see trace_ids.

## Evidence
Expected (AWS): Invoke -> HTTP 200, no `X-Amz-Function-Error` header, Payload = the handler's JSON report containing `ok` and `denied`; the granted twin's PutObject succeeds and the denied twin reports AccessDenied.

Actual (emulator):
- events__lambda__s3.lambda-role-missing-s3-permission-denied: `KeyError: 'denied'` at check_events__lambda__s3_lambda_role_missing_s3_permission_denied.py line 58 (`assert allowed["denied"] is False and allowed["ok"] is True`) — the returned payload is the runtime import error, not the handler report.
- events__lambda__s3.lambda-putevents-triggers-downstream-rule: `Check timed out after 60.0s`; no event from the handler ever reaches the downstream rule.
- The import failure as observed for identically packaged handlers in this same suite run: {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}.

## Steps to Reproduce
1. CreateFunction (runtime python3.x) from a zip whose handler module does `import boto3` and calls s3.put_object / events.put_events.
2. Invoke it synchronously.
3. Observe HTTP 200 with `x-amz-function-error: Unhandled` and a Runtime.ImportModuleError payload naming boto3; nothing is written to S3 and no event is published.
4. Re-replay the two checks in trace_ids to confirm.
