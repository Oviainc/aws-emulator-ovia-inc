Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3.error-propagation-lambda-s3-get-nonexistent-key dynamodb__lambda__s3.lambda-sdk-get-and-update-dynamodb-item dynamodb__lambda__s3.lambda-sdk-put-object-to-s3 dynamodb__lambda__s3.lambda-without-dynamodb-permission-denied dynamodb__lambda__s3.lambda-without-s3-permission-denied
Issue-Key: dynamodb__lambda__s3.error-propagation-lambda-s3-get-nonexistent-key|run#6|KeyError|assert control["payload"]["ok"] is True, (
Issue-Key: dynamodb__lambda__s3.lambda-sdk-get-and-update-dynamodb-item|run#7|KeyError|assert result["payload"]["ok"] is True, f"lambda invocation did not report ok: {result}"
Issue-Key: dynamodb__lambda__s3.lambda-sdk-put-object-to-s3|run#5|KeyError|assert result["payload"]["ok"] is True, f"Lambda did not report success: {result!r}"
Issue-Key: dynamodb__lambda__s3.lambda-without-dynamodb-permission-denied|run#6|KeyError|assert denied_result["payload"]["ok"] is False, (
Issue-Key: dynamodb__lambda__s3.lambda-without-s3-permission-denied|invoke_ok#13|KeyError|return r["payload"]["ok"]
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# dynamodb__lambda__s3: the Lambda python runtime has no AWS SDK, so no handler can reach DynamoDB or S3

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 — the managed python runtime ships no boto3/botocore, so every handler in this group faults at init instead of calling DynamoDB or S3.
AWS: the python3.x managed runtimes bundle boto3/botocore; a handler may `import boto3` and call s3:GetObject/PutObject and dynamodb:GetItem/UpdateItem under its execution role. A GetObject for a key that does not exist surfaces in the handler as NoSuchKey, and a denied action surfaces as AccessDenied — both as the handler's own caught error, not as a runtime init failure.
Emulator: Invoke answers HTTP 200 with FunctionError='Unhandled' and payload {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError"}, so the handler never runs.
Why: the Lambda-side S3/DynamoDB flows and the error-propagation and permission-boundary controls in this group are unobservable; a client gets an init failure where AWS returns the handler's result or its caught service error. Open issue 20261029T180026Z-dynamodb-lambda-s3-s3-notifications-never-invoke-lambda-are-accepted-without.md owns the notification checks in this group, not these.
Fix: the python runtime's bundled site-packages / module search path in the lambda runtime component.
Traces: all five check ids in trace_ids.

## Evidence
The checks fail at the point where they read the handler's result; the invoke response is HTTP 200 with FunctionError='Unhandled' and payload {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError"}. Expected: the handler's result, or its caught NoSuchKey/AccessDenied code in the payload.

## Steps to Reproduce
CreateFunction (python3.12) whose handler does `import boto3` and calls s3.put_object / s3.get_object / dynamodb.update_item, then Invoke it; withhold the action on the execution role for the negative legs.
