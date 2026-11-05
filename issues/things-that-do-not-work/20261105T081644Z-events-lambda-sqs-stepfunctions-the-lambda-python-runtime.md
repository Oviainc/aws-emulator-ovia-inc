Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions
Issue-Key: events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions|run#8|KeyError|assert allowed_payload["events"] == "ALLOWED", (
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# events__lambda__sqs__stepfunctions: the Lambda Python runtime ships no AWS SDK, so an SDK-calling handler returns an import error instead of its result document (1 cause)

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the Lambda managed Python runtime provides no AWS SDK, so every handler that uses boto3 dies at module import and returns an error payload in place of its result document.
AWS: the managed Python runtimes bundle the AWS SDK for Python — `import boto3` succeeds in a function deployed as a bare .zip with no layer, and the SDK resolves the region and the function role's temporary credentials from the runtime environment (AWS_REGION, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_SESSION_TOKEN). Invoke then answers HTTP 200 with no X-Amz-Function-Error header and the handler's returned document as the payload (Smithy: lambda#Invoke output FunctionError -> httpHeader X-Amz-Function-Error, Payload -> httpPayload). A handler written to probe its own permissions therefore returns its own verdict document — e.g. a per-service key whose value is "ALLOWED" or the service's AccessDenied code.
Emulator: Invoke answers HTTP 200 with `X-Amz-Function-Error: Unhandled` and the payload `{"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-run id>", "stackTrace": []}`. The fixed text is "Unable to import module '<module>': No module named 'boto3'" with errorType `Runtime.ImportModuleError`; the module name comes from the function's Handler setting and the requestId is per-invocation. Because the handler body never runs, its result document has none of its own keys (the check read `allowed_payload["events"]` and got KeyError: 'events').
Why: an allow-vs-deny probe run inside Lambda cannot distinguish "permitted" from "denied" — both arms come back as the same import error — and no SDK side effect the handler would make (PutEvents, SendMessage, StartExecution) ever happens.
Fix: the Lambda Invoke data-plane runtime/sandbox for the Python runtimes must make the AWS SDK importable on the handler's sys.path and export the region plus the function role's credentials into the handler environment.
Traces: events__lambda__sqs__stepfunctions.lambda-sdk-denied-without-role-permissions.

## Evidence
Expected: the permitted arm's invoke payload carries the handler's own verdict, `payload["events"] == "ALLOWED"`. Actual: `KeyError: 'events'` at `assert allowed_payload["events"] == "ALLOWED"` — the payload is instead `{"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", ...}` with X-Amz-Function-Error: Unhandled.

## Steps to Reproduce
1. CreateFunction with a Python runtime and a one-file .zip whose handler does `import boto3`, calls events.put_events / sqs.send_message under the function role, and returns a dict of per-service verdicts.
2. Invoke it.
3. Observe HTTP 200 with X-Amz-Function-Error: Unhandled and the payload {"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", ...} instead of the handler's verdict document.
