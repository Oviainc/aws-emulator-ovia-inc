Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3.error-propagation-lambda-s3-get-nonexistent-key dynamodb__lambda__s3.lambda-sdk-get-and-update-dynamodb-item dynamodb__lambda__s3.lambda-sdk-put-object-to-s3 dynamodb__lambda__s3.lambda-without-dynamodb-permission-denied dynamodb__lambda__s3.lambda-without-s3-permission-denied
Issue-Key: dynamodb__lambda__s3.error-propagation-lambda-s3-get-nonexistent-key|run#6|KeyError|assert control["payload"]["ok"] is True, (
Issue-Key: dynamodb__lambda__s3.lambda-sdk-get-and-update-dynamodb-item|run#7|KeyError|assert result["payload"]["ok"] is True, f"lambda invocation did not report ok: {result}"
Issue-Key: dynamodb__lambda__s3.lambda-sdk-put-object-to-s3|run#5|KeyError|assert result["payload"]["ok"] is True, f"Lambda did not report success: {result!r}"
Issue-Key: dynamodb__lambda__s3.lambda-without-dynamodb-permission-denied|run#6|KeyError|assert denied_result["payload"]["ok"] is False, (
Issue-Key: dynamodb__lambda__s3.lambda-without-s3-permission-denied|invoke_ok#13|KeyError|return r["payload"]["ok"]
Issue-Commit: c1082b91874a267e16e5e882b92de2a31503a88d

# dynamodb__lambda__s3: the Lambda python runtime provides no AWS SDK, so every boto3-importing handler faults Runtime.ImportModuleError and the S3/DynamoDB permission pairs are unobservable

> _Note: this issue was identified against the code base at commit `c1082b91874a267e16e5e882b92de2a31503a88d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the emulator's python3.x Lambda invoke runtime has no boto3/botocore on the handler's import path, so a function whose module does `import boto3` (and `from botocore.exceptions import ClientError`) to call S3 or DynamoDB fails at module import and the handler body never executes. Failing checks: dynamodb__lambda__s3.error-propagation-lambda-s3-get-nonexistent-key, dynamodb__lambda__s3.lambda-sdk-get-and-update-dynamodb-item, dynamodb__lambda__s3.lambda-sdk-put-object-to-s3, dynamodb__lambda__s3.lambda-without-dynamodb-permission-denied, dynamodb__lambda__s3.lambda-without-s3-permission-denied.

AWS: AWS Lambda's managed python3.x runtimes ship the AWS SDK for Python (boto3/botocore) pre-installed on the function's sys.path, so a module-scope `import boto3` succeeds and the handler calls S3/DynamoDB with its execution-role credentials. Smithy (lambda, Invoke): output StatusCode is bound to httpResponseCode, FunctionError to the response header `X-Amz-Function-Error`, Payload to the HTTP payload; a successful synchronous Invoke answers HTTP 200 with NO `X-Amz-Function-Error` header and Payload equal to the handler's return value — here the handler's own report `{"ok": true}` on success and `{"ok": false, "error": <code>, "message": <message>}` when it catches a ClientError. A handler whose call is refused by IAM is expected to run and report that refusal, not to fail at import.

Emulator: Invoke answers HTTP 200 and the Payload is the runtime's own import failure instead of the handler's report — errorType `Runtime.ImportModuleError`, errorMessage `Unable to import module '<module>': No module named 'boto3'`. Fixed text: `Unable to import module '` + `': No module named 'boto3'`; variable parts are `<module>` (taken from the function's Handler setting) and the per-invocation requestId. The handler's keys are absent, so each of the five checks dies with `KeyError: 'ok'`.

Why: a client whose function reads S3 or reads/updates a DynamoDB item gets a function that cannot run at all. Both halves of every permission pair collapse to the same import error, so the caller cannot distinguish "the SDK call was refused by IAM" from "the runtime could not start", and error propagation from S3/DynamoDB into the handler's result is entirely unreachable.

Fix: the Lambda invoke runtime/sandbox that backs the python3.x runtimes must expose the AWS SDK on the handler's import path (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the python runtime bundle it loads).

Related open issues describe the same runtime gap under other service groupings and do not carry these five checks' replay keys: 20261105T094343Z-events-lambda-s3-the-lambda-python-runtime-provides.md, 20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md, 20261105T081644Z-events-lambda-s3-stepfunctions-the-lambda-python-runtime.md. The open dynamodb__lambda__s3 issues (e.g. 20261029T180026Z-dynamodb-lambda-s3-s3-notifications-never-invoke-lambda-are-accepted-without.md, 20261025T022656Z-dynamodb-lambda-s3-an-s3-putobject-never-invokes.md) own other checks of this group, not these.

Traces: see trace_ids.

## Evidence
Expected (AWS): synchronous Invoke -> HTTP 200, no `X-Amz-Function-Error` header, Payload = `{"ok": true}` for the control call and `{"ok": false, "error": "NoSuchKey"|"AccessDenied", "message": ...}` for the refused/missing-key call.

Actual (emulator): HTTP 200 with `x-amz-function-error: Unhandled` and Payload
{"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}
so the harness assertion `assert control["payload"]["ok"] is True` raises:
- dynamodb__lambda__s3.error-propagation-lambda-s3-get-nonexistent-key: `KeyError: 'ok'` (check line 65)
- dynamodb__lambda__s3.lambda-sdk-get-and-update-dynamodb-item: `KeyError: 'ok'`
- dynamodb__lambda__s3.lambda-sdk-put-object-to-s3: `KeyError: 'ok'`
- dynamodb__lambda__s3.lambda-without-dynamodb-permission-denied: `KeyError: 'ok'`
- dynamodb__lambda__s3.lambda-without-s3-permission-denied: `KeyError: 'ok'`
The deployed handler source in these checks is literally `import boto3` / `from botocore.exceptions import ClientError` / `s3 = boto3.client("s3")` at module scope, which is exactly what fails.

## Steps to Reproduce
1. CreateFunction (runtime python3.x) from a zip whose handler module starts with `import boto3` and `from botocore.exceptions import ClientError`, then calls s3.get_object / dynamodb.update_item and returns `{"ok": True}`.
2. Invoke it synchronously.
3. Observe HTTP 200 with `x-amz-function-error: Unhandled` and a Runtime.ImportModuleError payload naming boto3; nothing is read from S3 or written to DynamoDB.
4. Re-replay the five checks in trace_ids to confirm.
