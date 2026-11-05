Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role
Issue-Key: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role|_invoke_payload#13|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Commit: c1082b91874a267e16e5e882b92de2a31503a88d

# kms__lambda__s3: the Lambda python runtime provides no AWS SDK, so the SSE-KMS read handler faults Runtime.ImportModuleError and a key-policy denial is never observed

> _Note: this issue was identified against the code base at commit `c1082b91874a267e16e5e882b92de2a31503a88d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the emulator's python3.x Lambda invoke runtime has no boto3/botocore on the handler's import path, so the function that reads an SSE-KMS object to prove a key-policy denial fails at module import and never issues the GetObject. Failing check: kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role.

AWS: AWS Lambda's managed python3.x runtimes ship the AWS SDK for Python (boto3/botocore) pre-installed on the function's sys.path, so a module-scope `import boto3` succeeds and the handler calls S3 with its execution-role credentials. Smithy (lambda, Invoke): output StatusCode is bound to httpResponseCode, FunctionError to the response header `X-Amz-Function-Error`, Payload to the HTTP payload; a successful synchronous Invoke answers HTTP 200 with NO `X-Amz-Function-Error` header and Payload equal to the handler's return value. The handler is expected to run, have its GetObject refused because the CMK's key policy denies its role, and report that refusal (an `AccessDenied`/`KMSAccessDenied` class result) in its own payload.

Emulator: Invoke answers HTTP 200 with `x-amz-function-error: Unhandled` and the Payload is the runtime's own import failure — errorType `Runtime.ImportModuleError`, errorMessage `Unable to import module '<module>': No module named 'boto3'`. Fixed text: `Unable to import module '` + `': No module named 'boto3'`; the variable parts are `<module>` (from the function's Handler setting) and the per-invocation requestId.

Why: the whole point of the check — that an SSE-KMS object becomes unreadable to a role the key policy denies, and readable again when it does not — is unobservable, because the function cannot start. A client gets a fault attributed to its own code instead of either the KMS denial or the absent SDK.

Fix: the Lambda invoke runtime/sandbox that backs the python3.x runtimes must expose the AWS SDK on the handler's import path (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the python runtime bundle it loads).

Related open issues describe the same runtime gap under other service groupings and do not carry this check's replay key: 20261105T094343Z-events-lambda-s3-the-lambda-python-runtime-provides.md, 20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md. The open kms__lambda__s3 issue 20261029T175759Z-kms-lambda-s3-headobject-reports-ssekmskeyid-as-a.md owns the other check of this group, not this one.

Traces: see trace_ids.

## Evidence
Expected (AWS): synchronous Invoke -> HTTP 200, no `X-Amz-Function-Error` header, Payload = the handler's own report of the GetObject outcome (success before the deny, an AccessDenied-class refusal after it).

Actual (emulator): `AssertionError: handler faulted instead of returning a result: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<per-invocation id>', 'stackTrace': []}}` at check_kms__lambda__s3_sse_kms_object_unreadable_when_key_policy_denies_role.py line 268.

## Steps to Reproduce
1. Create a CMK, a bucket with SSE-KMS default encryption against it, and put an object.
2. CreateFunction (runtime python3.x) from a zip whose handler module does `import boto3` and calls s3.get_object on that object.
3. Invoke it synchronously -> observe HTTP 200 with `x-amz-function-error: Unhandled` and a Runtime.ImportModuleError payload naming boto3; no GetObject and no KMS Decrypt is attempted.
4. Re-replay kms__lambda__s3.sse-kms-object-unreadable-when-key-policy-denies-role to confirm.
