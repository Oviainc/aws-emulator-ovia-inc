Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__events__lambda.xsvc-cloudformation-3-rollback-complete-status-event-drives-lambda-dynamodb-write cloudformation__dynamodb__events__lambda.xsvc-cloudformation-3-update-rollback-complete-rejected-by-exact-rollback-complete-rule
Issue-Key: cloudformation__dynamodb__events__lambda.xsvc-cloudformation-3-rollback-complete-status-event-drives-lambda-dynamodb-write|_build_chain#8|AssertionError|assert "AccessDenied" in body and time.monotonic() < deadline, f"warm-up invoke failed: {body[:400]!r}"
Issue-Key: cloudformation__dynamodb__events__lambda.xsvc-cloudformation-3-update-rollback-complete-rejected-by-exact-rollback-complete-rule|_build_chain#8|AssertionError|assert "AccessDenied" in body and time.monotonic() < deadline, f"warm-up invoke failed: {body[:400]!r}"
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# cloudformation__dynamodb__events__lambda: the Lambda python runtime provides no AWS SDK, so a handler that imports boto3 faults Runtime.ImportModuleError and the CFN-status-event chain never runs

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the python3.x Lambda invoke runtime has no boto3/botocore on the handler's import path, so every function whose module imports the AWS SDK fails at module import and its body never executes. Failing checks: cloudformation__dynamodb__events__lambda.xsvc-cloudformation-3-rollback-complete-status-event-drives-lambda-dynamodb-write, cloudformation__dynamodb__events__lambda.xsvc-cloudformation-3-update-rollback-complete-rejected-by-exact-rollback-complete-rule.

AWS: AWS Lambda's managed python3.x runtimes ship the AWS SDK for Python (boto3/botocore) pre-installed on the function's sys.path, so `import boto3` at module scope succeeds and the handler can call other AWS services with its execution-role credentials. Smithy (lambda, Invoke): output member FunctionError is bound to the response header `X-Amz-Function-Error` and StatusCode to the HTTP response code; a successful synchronous Invoke answers HTTP 200 with NO `X-Amz-Function-Error` header and Payload equal to the handler's own return value. `FunctionError: Unhandled` plus a payload of the shape {"errorMessage":...,"errorType":...,"requestId":...,"stackTrace":[...]} is reserved for a genuine fault in customer code. These two checks deploy a handler that calls EventBridge/DynamoDB through boto3 and first perform a warm-up invoke that must come back refused by IAM (the body must contain `AccessDenied`).

Emulator: the Invoke answers HTTP 200 but sets `x-amz-function-error: Unhandled` and the payload is the runtime's own import failure: errorType `Runtime.ImportModuleError`, errorMessage `Unable to import module '<module>': No module named 'boto3'`. The fixed text is `Unable to import module '` + `': No module named 'boto3'`; `<module>` is the handler module named by the function's Handler setting (here `index`), and requestId is the per-invocation id (not part of the requirement). stackTrace is empty. The handler body never runs, so no IAM decision, no PutEvents and no DynamoDB write is ever produced, and the checks' warm-up assertion (`AccessDenied` in body) can never be satisfied.

Why: a client that packages an ordinary python handler using boto3 — the overwhelmingly common Lambda shape, and the shape CloudFormation-deployed stacks produce — gets a function that cannot run at all. Every in-function AWS interop behavior built on top of it (IAM denial of a cross-service call, CFN stack-status events driving a DynamoDB write, downstream rule chains) is unobservable, and the emulator reports the failure as the customer's code faulting rather than as a missing runtime.

Fix: the Lambda invoke runtime/sandbox that backs the python3.x runtimes must expose the AWS SDK on the handler's import path (the Invoke handler and the runtime image it executes in: services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the python runtime bundle it loads).

Related open issues describe this same runtime gap under other service groupings (20261105T081225Z-events-lambda-ssm-the-lambda-python-runtime-has.md, 20261105T081644Z-events-lambda-s3-stepfunctions-the-lambda-python-runtime.md, 20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md); none of them carries the replay keys of these two checks.

Traces: see trace_ids.

## Evidence
Expected (AWS): Invoke -> HTTP 200, no `X-Amz-Function-Error` header, Payload = handler return value; the warm-up invoke in both checks returns a body containing `AccessDenied` because the execution role lacks the grant.

Actual (emulator): Invoke -> HTTP 200 with `x-amz-function-error: Unhandled`, Payload = {"errorMessage":"Unable to import module 'index': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}. Both checks abort at `assert "AccessDenied" in body ...` -> `AssertionError: warm-up invoke failed: '{"errorMessage":"Unable to import module \'index\': No module named \'boto3\'","errorType":"Runtime.ImportModuleError",...}'` (check_...rollback_complete_status_event_drives_lambda_dynamodb_write.py line 222; check_...update_rollback_complete_rejected_by_exact_rollback_complete_rule.py line 244).

## Steps to Reproduce
1. CreateFunction with runtime python3.x and a zip whose index.py starts with `import boto3`.
2. Invoke it synchronously.
3. Observe HTTP 200 with `x-amz-function-error: Unhandled` and a Runtime.ImportModuleError payload naming boto3, instead of the handler's own result.
4. Re-replay the two checks in trace_ids to confirm.
