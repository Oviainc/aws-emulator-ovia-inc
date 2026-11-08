Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.lambda-role-missing-events-permission-putevents-denied
Issue-Key: events__lambda__sns.lambda-role-missing-events-permission-putevents-denied|_invoke_once#17|Timeout|return json.loads(resp["Payload"].read())
Issue-Commit: f57c1e6183126cdb225aa0da3b893ecbc2169fde

# events__lambda__sns: a python Lambda handler that imports the AWS SDK and calls EventBridge PutEvents never runs — Invoke answers FunctionError=Unhandled/Runtime.ImportModuleError, so the role-denied PutEvents path is never reached

> _Note: this issue was identified against the code base at commit `f57c1e6183126cdb225aa0da3b893ecbc2169fde`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue carries 1 net-new root cause. It must be closed before submitting.

NOTE: the open issue 20261105T081727Z-events-lambda-sns-an-invoked-function-writes-nothing.md describes a similar in-guest-SDK cause and even lists this check in its trace ids, but the replay suite did NOT report this failure as OWNED by it (its recorded replay key no longer matches what the check now fails on), so the failure is live and unowned and is filed here. A fixer closing this one should check that issue for overlap rather than assume it is covered.

Cause 1 of 1: a python3.x function whose deployment package contains a single handler module beginning `import json` / `import boto3` and which then calls EventBridge PutEvents never executes that call. The handler fails at import time, so neither the permitted PutEvents (which should land an event on the bus and, via the rule's SQS target, on the queue) nor the denied PutEvents (which should surface an AccessDenied-class error code inside the handler's own returned payload) ever happens, and the check exhausts its budget.
Failing check id: events__lambda__sns.lambda-role-missing-events-permission-putevents-denied

AWS: the managed Python runtimes ship the AWS SDK for Python (boto3/botocore) inside the runtime image, so `import boto3` resolves for a package that contains only the handler module, and the runtime injects the execution role's credentials so a client constructed inside the handler is signed as that role. Smithy (lambda Invoke, POST /2015-03-31/functions/{FunctionName}/invocations, success code 200): the response carries `StatusCode` as the HTTP response code and `FunctionError` in the `X-Amz-Function-Error` header; `FunctionError` is populated only when the function itself raised, and `Unhandled` specifically means an error escaped the handler. A handler that catches its own botocore ClientError and returns a dict is therefore a 200 with NO `X-Amz-Function-Error` header. Recorded behavior: with a role granting events:PutEvents the handler returns `{"ok": true, "failed_count": 0}` and the event matches the rule and reaches the rule's SQS target; with a role lacking events:PutEvents the in-handler PutEvents raises a ClientError whose `Error.Code` is the AccessDenied-class code `AccessDeniedException`, the handler returns `{"ok": false, "error_code": "AccessDeniedException"}`, the Invoke is still 200 with no FunctionError, and no event reaches the bus or the denied rule's queue.

Emulator: the handler module is never imported. Invoke answers 200 with `X-Amz-Function-Error: Unhandled` and the payload `{"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<uuid>", "stackTrace": []}` — the fixed text is `Unable to import module '` … `': No module named 'boto3'` and the errorType `Runtime.ImportModuleError`; the variable parts are the handler module name taken from the request's `Handler` and the per-invocation requestId. Because `ok` is never true, the check's positive-control retry loop re-invokes until its own 60s deadline and the harness reports `Error: Check timed out after 60.0s`; the assertions on `error_code == "AccessDeniedException"` and on the event reaching the allowed rule's queue are never reached.

Why: a client relying on AWS's behavior gets nothing from Lambda-as-glue — a function cannot publish to EventBridge (or any AWS service) from its handler, so fan-out, enrichment and "invoke then observe downstream" workflows silently produce no effect. Worse for negative testing, the IAM-refusal path is replaced by an import crash, so a caller cannot distinguish "the role was denied events:PutEvents" (200, no FunctionError, handler-reported AccessDeniedException) from "the call was never made" (200, FunctionError=Unhandled, Runtime.ImportModuleError).

Fix: the Lambda execution engine's guest environment — the guest module allow-list / import path and bootstrap under services/service-com-amazonaws-lambda-awsgirapiservice/src/state/execution/ (python guest + bootstrap), together with the Invoke handler that classifies the run's outcome into StatusCode/FunctionError — must expose an AWS SDK surface to the guest that dispatches to the emulator's own EventBridge/SNS/SQS front doors under the function's execution-role identity, and must let an IAM refusal come back to the guest as a ClientError the handler can catch rather than failing the import.

Traces: events__lambda__sns.lambda-role-missing-events-permission-putevents-denied.

## Evidence
Failing check: events__lambda__sns.lambda-role-missing-events-permission-putevents-denied — `Error: Check timed out after 60.0s`.

Expected (AWS):
  - Invoke of the allowed function: HTTP 200, no `X-Amz-Function-Error` header, payload `{"ok": true, "failed_count": 0}`; the put event matches the rule `{"source": [<source>], "detail-type": ["sim2real-putevents-check"]}` and the rule's SQS target receives a message containing the marker.
  - Invoke of the denied function (role without events:PutEvents): HTTP 200, no `X-Amz-Function-Error` header, payload `{"ok": false, "error_code": "AccessDeniedException"}`, and no message ever reaches the denied rule's queue.

Actual (emulator): Invoke returns HTTP 200 with `X-Amz-Function-Error: Unhandled` and payload `{"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<uuid>", "stackTrace": []}` for BOTH functions (fixed text `Unable to import module '`…`': No module named 'boto3'` + `Runtime.ImportModuleError`; variable parts = handler module name from the request's Handler and the per-invocation requestId). `ok` is therefore never true, the check's positive control re-invokes for its full 60s budget, and the harness kills the check at 60.0s before it can assert on the denied payload or on the event reaching either queue.

## Steps to Reproduce
1. IAM: CreateRole trusting lambda.amazonaws.com for sts:AssumeRole; attach arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole. Create two such roles: one with an additional inline policy `{"Effect":"Allow","Action":"events:PutEvents","Resource":"*"}`, one with no extra policy.
2. Lambda: CreateFunction for each role with Runtime=python3.12, Handler=index.handler and a zip whose only module is
     import json, boto3
     def handler(event, context):
         try:
             resp = boto3.client("events").put_events(Entries=[{"Source": event["source"], "DetailType": event["detail_type"], "Detail": json.dumps(event["detail"])}])
             return {"ok": True, "failed_count": resp.get("FailedEntryCount", 0)}
         except Exception as exc:
             return {"ok": False, "error_code": getattr(exc, "response", {}).get("Error", {}).get("Code", "Unknown")}
   Wait for State=Active via GetFunctionConfiguration.
3. EventBridge/SQS: CreateQueue, PutRule with EventPattern {"source":[<source>],"detail-type":["sim2real-putevents-check"]}, PutTargets with the queue ARN, and SetQueueAttributes with a policy allowing events.amazonaws.com sqs:SendMessage from the rule ARN — once per function (allowed/denied) with distinct sources.
4. Invoke the allowed function (RequestResponse) with {"source": <allowed source>, "detail_type": "sim2real-putevents-check", "detail": {"marker": <marker>}} and read the payload. AWS: 200, no FunctionError, {"ok": true, "failed_count": 0}. Emulator: 200 with FunctionError=Unhandled and Runtime.ImportModuleError for 'boto3'.
5. Invoke the denied function the same way. AWS: 200, no FunctionError, {"ok": false, "error_code": "AccessDeniedException"}. Emulator: the same Runtime.ImportModuleError payload.
6. ReceiveMessage on the allowed queue (AWS: the marker arrives) and on the denied queue (AWS: nothing arrives). On the emulator neither queue ever receives anything because no PutEvents was made.
Replay: events__lambda__sns.lambda-role-missing-events-permission-putevents-denied.
