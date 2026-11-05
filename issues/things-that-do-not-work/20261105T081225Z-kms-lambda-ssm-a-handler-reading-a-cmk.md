Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda__ssm.lambda-reads-securestring-parameter-encrypted-with-customer-key
Issue-Key: kms__lambda__ssm.lambda-reads-securestring-parameter-encrypted-with-customer-key|_invoke_payload#12|AssertionError|assert "error" not in result, f"handler faulted instead of returning a result: {result!r}"
Issue-Commit: bd1d039abc40ef8a9373a2b76b9b7f1b159abc82

# kms__lambda__ssm: a handler reading a CMK-encrypted SecureString parameter faults Runtime.ImportModuleError — boto3 is absent from the Lambda python runtime

> _Note: this issue was identified against the code base at commit `bd1d039abc40ef8a9373a2b76b9b7f1b159abc82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a python3.x function whose handler imports boto3 to call `ssm:GetParameter` with `WithDecryption` faults on import instead of running.

AWS: the managed `python3.x` runtimes bundle boto3/botocore in the execution environment, so the handler imports the SDK with no layer or vendored dependency and `Invoke` (RequestResponse) answers 200 with the handler's payload — here the decrypted value of a SecureString parameter encrypted under a customer-managed KMS key, which is what the check reads back. Recorded behavior: the check expects a result document, and asserts `"error" not in result`.

Emulator: `Invoke` answers 200 with `X-Amz-Function-Error: Unhandled` and `{"errorMessage": "Unable to import module 'lambda_function': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": <per-run id>, "stackTrace": []}`. That wording and errorType are what AWS emits for a genuinely missing module (`Unable to import module '<handler module>': No module named '<module>'`, the names taken from the request's handler and the failing import); the defect is that boto3 is missing from a runtime where AWS provides it.

Why: the KMS/SSM interop this check exists to prove — that a function with `kms:Decrypt` on the CMK can read the SecureString and one without it cannot — is unobservable, because the handler never reaches its first API call and a real denial cannot be distinguished from this import fault.

Fix: the Lambda execution environment the in-process interpreter builds — `services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/python` and the runtime bootstrap beside it — must expose boto3/botocore to the handler, wired to this emulator's endpoints and the function role's credentials. Same root cause as the events__lambda__ssm issue filed in this sweep; filed separately because it is a different service heading and takes its own gate verdict.

Traces: kms__lambda__ssm.lambda-reads-securestring-parameter-encrypted-with-customer-key.

## Evidence
`AssertionError: handler faulted instead of returning a result: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': <id>, 'stackTrace': []}}`
Expected: 200 with no FunctionError and the handler's JSON result carrying the decrypted SecureString value.

## Steps to Reproduce
Replay the behavioral suite, group kms__lambda__ssm, check lambda-reads-securestring-parameter-encrypted-with-customer-key: create a CMK, put a SecureString parameter encrypted with it, create a python3.x function whose inline handler imports boto3 and calls `ssm:GetParameter(WithDecryption=True)`, then `lambda:Invoke` it — the response is 200 with `FunctionError: Unhandled` and `Runtime.ImportModuleError` / `No module named 'boto3'`.
