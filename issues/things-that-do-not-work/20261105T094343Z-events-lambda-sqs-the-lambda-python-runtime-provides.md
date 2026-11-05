Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.cfn-iam-role-trusts-lambda-service-principal events__lambda__sqs.lambda-putevents-missing-permission-denied events__lambda__sqs.lambda-putevents-triggers-downstream-rule-chain events__lambda__sqs.lambda-role-without-events-putevents-permission-denied events__lambda__sqs.lambda-role-without-sqs-sendmessage-permission-denied events__lambda__sqs.lambda-sdk-call-sqs-missing-permission-denied events__lambda__sqs.lambda-sdk-call-sqs-send-message-success events__lambda__sqs.lambda-sdk-events-describerule-missing-permission-denied events__lambda__sqs.lambda-sdk-putevents-custom-bus-triggers-downstream-rule events__lambda__sqs.lambda-sdk-putevents-missing-permission-denied events__lambda__sqs.lambda-sdk-sqs-batch-sendmessage-delivers-all events__lambda__sqs.lambda-sdk-sqs-changemessagevisibility-extends-lock events__lambda__sqs.lambda-sdk-sqs-deletemessage-after-processing events__lambda__sqs.lambda-sdk-sqs-receivemessage-missing-permission-denied events__lambda__sqs.lambda-to-eventbridge-to-sqs-chain
Issue-Key: events__lambda__sqs.cfn-iam-role-trusts-lambda-service-principal|run#9|AssertionError|assert invoke.get("FunctionError") is None, f"invoke failed: {invoke.get('FunctionError')}"
Issue-Key: events__lambda__sqs.lambda-putevents-missing-permission-denied|run#18|KeyError|assert payload["ok"] is False, f"expected put_events to be denied, got: {payload}"
Issue-Key: events__lambda__sqs.lambda-putevents-triggers-downstream-rule-chain|run#17|AssertionError|assert resp.get("FunctionError") is None, f"lambda invoke reported an error: {resp}"
Issue-Key: events__lambda__sqs.lambda-role-without-events-putevents-permission-denied|_invoke_once#21|AssertionError|assert result.get("error") is None, f"handler should not fault: {result}"
Issue-Key: events__lambda__sqs.lambda-role-without-sqs-sendmessage-permission-denied|run#6|AssertionError|assert denied.get("error") is None, f"handler should not fault: {denied}"
Issue-Key: events__lambda__sqs.lambda-sdk-call-sqs-missing-permission-denied|run#9|KeyError|assert payload["ok"] is False, f"expected the SDK call to fail without permission, got: {payload}"
Issue-Key: events__lambda__sqs.lambda-sdk-call-sqs-send-message-success|run#7|AssertionError|assert resp.get("FunctionError") is None
Issue-Key: events__lambda__sqs.lambda-sdk-events-describerule-missing-permission-denied|run#11|KeyError|assert denied_body["ok"] is False, (
Issue-Key: events__lambda__sqs.lambda-sdk-putevents-custom-bus-triggers-downstream-rule|_poll_until#20|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: events__lambda__sqs.lambda-sdk-putevents-missing-permission-denied|run#22|KeyError|assert denied_body["ok"] is False, f"expected the denied call to fail, got: {denied_body}"
Issue-Key: events__lambda__sqs.lambda-sdk-sqs-batch-sendmessage-delivers-all|_poll_until#7|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: events__lambda__sqs.lambda-sdk-sqs-changemessagevisibility-extends-lock|_poll_until#9|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: events__lambda__sqs.lambda-sdk-sqs-deletemessage-after-processing|_poll_until#8|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: events__lambda__sqs.lambda-sdk-sqs-receivemessage-missing-permission-denied|run#9|KeyError|assert denied_body["ok"] is False, (
Issue-Key: events__lambda__sqs.lambda-to-eventbridge-to-sqs-chain|_wait_for_messages#15|Timeout|resp = sqs.receive_message(
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# events__lambda__sqs: the Lambda python runtime provides no AWS SDK, so no handler can call SQS or EventBridge — 15 checks get Runtime.ImportModuleError instead of a result

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the python3.x Lambda invoke runtime has no boto3/botocore on the handler's import path, so every function these checks deploy — each of which calls SQS (SendMessage/SendMessageBatch/ReceiveMessage/DeleteMessage/ChangeMessageVisibility) or EventBridge (PutEvents/DescribeRule) through the AWS SDK — fails at module import and its body never executes. Failing checks: events__lambda__sqs.cfn-iam-role-trusts-lambda-service-principal, events__lambda__sqs.lambda-putevents-missing-permission-denied, events__lambda__sqs.lambda-putevents-triggers-downstream-rule-chain, events__lambda__sqs.lambda-role-without-events-putevents-permission-denied, events__lambda__sqs.lambda-role-without-sqs-sendmessage-permission-denied, events__lambda__sqs.lambda-sdk-call-sqs-missing-permission-denied, events__lambda__sqs.lambda-sdk-call-sqs-send-message-success, events__lambda__sqs.lambda-sdk-events-describerule-missing-permission-denied, events__lambda__sqs.lambda-sdk-putevents-custom-bus-triggers-downstream-rule, events__lambda__sqs.lambda-sdk-putevents-missing-permission-denied, events__lambda__sqs.lambda-sdk-sqs-batch-sendmessage-delivers-all, events__lambda__sqs.lambda-sdk-sqs-changemessagevisibility-extends-lock, events__lambda__sqs.lambda-sdk-sqs-deletemessage-after-processing, events__lambda__sqs.lambda-sdk-sqs-receivemessage-missing-permission-denied, events__lambda__sqs.lambda-to-eventbridge-to-sqs-chain.

AWS: AWS Lambda's managed python3.x runtimes ship the AWS SDK for Python (boto3/botocore) pre-installed on the function's sys.path, so `import boto3` at module scope succeeds and the handler can call SQS and EventBridge with its execution-role credentials. Smithy (lambda, Invoke): FunctionError is bound to the response header `X-Amz-Function-Error`, StatusCode to the HTTP response code; a successful synchronous Invoke answers HTTP 200 with NO `X-Amz-Function-Error` header and Payload equal to the handler's return value — here a JSON report carrying the handler's own keys (`ok`, `error`, the MessageId or EventId it obtained). For the permission-boundary checks the handler is expected to RUN and report the service's AccessDeniedException in its result (`ok: false`), which is why those checks assert on `payload["ok"] is False` rather than on a function error.

Emulator: every such Invoke answers HTTP 200 with `x-amz-function-error: Unhandled` (observed headers: `x-amz-function-error: Unhandled`, `x-amz-executed-version: $LATEST`) and the payload is the runtime's own import failure, not the handler's result: errorType `Runtime.ImportModuleError`, errorMessage `Unable to import module '<module>': No module named 'boto3'`. The fixed text is `Unable to import module '` + `': No module named 'boto3'`; `<module>` is the handler module named by the function's Handler setting (observed as both `index` and `lambda_function`) and requestId is the per-invocation id. Because the body never runs the handler's keys are missing (`KeyError: 'ok'`), the polling checks never see the SQS message or downstream event the handler was to produce (`condition not met within 30.0s/40.0s; last={... Runtime.ImportModuleError ...}`), and events__lambda__sqs.lambda-to-eventbridge-to-sqs-chain simply exhausts the harness budget with `Check timed out after 60.0s` (Hypothesis for that one: same cause — it deploys the same shape of boto3-importing handler, and the timeout line carries no further diagnosis).

Why: a client's function cannot talk to SQS or EventBridge at all, so the entire in-function interop surface is unobservable: positive paths send/receive/delete nothing, chained Lambda->EventBridge->SQS pipelines never advance, and every negative permission check cannot be distinguished from a broken runtime because the emulator reports the fault as the customer's code faulting rather than as a missing SDK.

Fix: the Lambda invoke runtime/sandbox that backs the python3.x runtimes must expose the AWS SDK on the handler's import path (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the python runtime bundle it loads).

Related open issues describe the same runtime gap under other service groupings (20261105T081727Z-events-lambda-sns-sqs-a-lambda-handler-cannot.md, 20261105T081644Z-events-lambda-sqs-stepfunctions-the-lambda-python-runtime.md); the open events__lambda__sqs issues (e.g. 20261029T182228Z-events-lambda-sqs-6-of-the-7-causes-remain-open.md, 20261101T210635Z-events-lambda-sqs-6-net-new-causes-eventbridge.md) do not carry the replay keys of these fifteen checks.

Traces: see trace_ids.

## Evidence
Expected (AWS): Invoke -> HTTP 200, no `X-Amz-Function-Error` header, Payload = the handler's JSON report (`ok: true` with the MessageId/EventId it obtained, or `ok: false` carrying the AccessDeniedException for the denied-role variants).

Actual (emulator): HTTP 200 with `x-amz-function-error: Unhandled`, `FunctionError: 'Unhandled'`, Payload = {"errorMessage":"Unable to import module '<module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}. Representative assertions:
- lambda-putevents-missing-permission-denied line 79, lambda-sdk-call-sqs-missing-permission-denied line 49, lambda-sdk-events-describerule-missing-permission-denied line 55, lambda-sdk-putevents-missing-permission-denied line 102, lambda-sdk-sqs-receivemessage-missing-permission-denied line 51: `KeyError: 'ok'`
- lambda-role-without-events-putevents-permission-denied line 108 and lambda-role-without-sqs-sendmessage-permission-denied line 45: `AssertionError: handler should not fault: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}}`
- lambda-sdk-putevents-custom-bus-triggers-downstream-rule line 163, lambda-sdk-sqs-batch-sendmessage-delivers-all line 99, lambda-sdk-sqs-changemessagevisibility-extends-lock line 137, lambda-sdk-sqs-deletemessage-after-processing line 130: `AssertionError: condition not met within 40.0s/30.0s; last={'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}`
- lambda-putevents-triggers-downstream-rule-chain line 87, cfn-iam-role-trusts-lambda-service-principal line 94, lambda-sdk-call-sqs-send-message-success line 58: FunctionError 'Unhandled' present on a 200 response (`invoke failed: Unhandled`)
- lambda-to-eventbridge-to-sqs-chain: `Check timed out after 60.0s`.

## Steps to Reproduce
1. CreateFunction (runtime python3.x) from a zip whose handler module does `import boto3` and calls sqs.send_message or events.put_events.
2. Invoke it synchronously.
3. Observe HTTP 200 with `x-amz-function-error: Unhandled` and a Runtime.ImportModuleError payload naming boto3; no message reaches the queue and no event reaches the bus.
4. Re-replay the fifteen checks in trace_ids to confirm.
