Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.lambda-role-missing-cloudfront-permission-denied cloudfront__events.deploy-cfn-eventbridge-rule-cloudfront-invalidation-lambda cloudfront__events.eventbridge-cloudfront-distribution-arn-resolution-in-target-input cloudfront__events.eventbridge-cloudfront-event-missing-detail-type-not-matched cloudfront__events.eventbridge-lambda-invalidation-failure-routes-to-dlq cloudfront__events.eventbridge-lambda-target-missing-permission-blocks-delivery cloudfront__events.eventbridge-rule-input-path-extracts-distribution-id cloudfront__events.eventbridge-target-arn-resolves-to-specific-cloudfront-distribution
Issue-Key: cloudfront__events.lambda-role-missing-cloudfront-permission-denied|run#25|KeyError|assert direct_payload["ok"] is False, f"expected the denied role's direct invoke to fail, got {direct_payload}"
Issue-Key: cloudfront__events.deploy-cfn-eventbridge-rule-cloudfront-invalidation-lambda|_poll_until#9|Timeout|last = probe()
Issue-Key: cloudfront__events.eventbridge-cloudfront-distribution-arn-resolution-in-target-input|_warm_up_role#7|Timeout|time.sleep(3)
Issue-Key: cloudfront__events.eventbridge-cloudfront-event-missing-detail-type-not-matched|_warm_up_role#10|Timeout|time.sleep(3)
Issue-Key: cloudfront__events.eventbridge-lambda-invalidation-failure-routes-to-dlq|_warm_up_role#18|Timeout|time.sleep(3)
Issue-Key: cloudfront__events.eventbridge-lambda-target-missing-permission-blocks-delivery|_warm_up_role#14|Timeout|time.sleep(3)
Issue-Key: cloudfront__events.eventbridge-rule-input-path-extracts-distribution-id|_warm_up_role#10|Timeout|time.sleep(3)
Issue-Key: cloudfront__events.eventbridge-target-arn-resolves-to-specific-cloudfront-distribution|_warm_up_role#12|Timeout|time.sleep(3)
Issue-Commit: 6da489cbc4c2969d809761cd42ffd786edefa992

# cloudfront__events: the Lambda python runtime provides no AWS SDK, so every invalidation/eventing handler faults Runtime.ImportModuleError and 8 CloudFront-EventBridge checks never observe their effect

> _Note: this issue was identified against the code base at commit `6da489cbc4c2969d809761cd42ffd786edefa992`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the python3.x Lambda invoke runtime has no boto3/botocore on the handler's import path, so every function these checks deploy — each of which calls CloudFront (CreateInvalidation/GetDistribution), SQS or EventBridge through the AWS SDK — fails at module import and its body never executes. Failing checks: cloudfront__events.lambda-role-missing-cloudfront-permission-denied, cloudfront__events.deploy-cfn-eventbridge-rule-cloudfront-invalidation-lambda, cloudfront__events.eventbridge-cloudfront-distribution-arn-resolution-in-target-input, cloudfront__events.eventbridge-cloudfront-event-missing-detail-type-not-matched, cloudfront__events.eventbridge-lambda-invalidation-failure-routes-to-dlq, cloudfront__events.eventbridge-lambda-target-missing-permission-blocks-delivery, cloudfront__events.eventbridge-rule-input-path-extracts-distribution-id, cloudfront__events.eventbridge-target-arn-resolves-to-specific-cloudfront-distribution.

AWS: AWS Lambda's managed python3.x runtimes ship the AWS SDK for Python (boto3/botocore) pre-installed on the function's sys.path, so `import boto3` at module scope succeeds and the handler can call CloudFront with its execution-role credentials. Smithy (lambda, Invoke): FunctionError is bound to the response header `X-Amz-Function-Error`, StatusCode to the HTTP response code; a successful synchronous Invoke answers HTTP 200 with NO `X-Amz-Function-Error` header and Payload equal to the handler's return value — here a JSON object carrying the handler's own report fields (`ok`, and for the permission-boundary check also whether the CloudFront call was denied). `FunctionError: Unhandled` with an {errorMessage,errorType,requestId,stackTrace} payload is reserved for a fault in customer code.

Emulator: the Invoke answers HTTP 200 with `x-amz-function-error: Unhandled` and a payload produced by the runtime, not the handler: errorType `Runtime.ImportModuleError`, errorMessage `Unable to import module '<module>': No module named 'boto3'` — fixed text `Unable to import module '` + `': No module named 'boto3'`, with `<module>` taken from the function's Handler setting and requestId the per-invocation id. The handler's own keys are therefore absent: cloudfront__events.lambda-role-missing-cloudfront-permission-denied dies with `KeyError: 'ok'` when it reads the returned payload. The other seven checks poll for the effect that handler was supposed to produce (an invalidation on the named distribution, a DLQ or SQS record, a transformed input echoed back) and, because the handler never runs, observe nothing and exhaust the harness's 60 s budget: `Check timed out after 60.0s`. Hypothesis: the seven timeouts share this one cause — each deploys a boto3-importing python handler in the same way as the check that does surface the import failure directly; the timeout line itself carries no further diagnosis.

Why: a client wiring the standard "EventBridge rule -> Lambda -> CloudFront CreateInvalidation" pattern gets a function that cannot run, so no invalidation is ever created, no DLQ record is ever written, and a negative check for a missing IAM grant cannot be distinguished from a broken runtime. Callers also see a function error attributed to their code rather than to the absent SDK.

Fix: the Lambda invoke runtime/sandbox that backs the python3.x runtimes must expose the AWS SDK on the handler's import path (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the python runtime bundle it loads).

Related open issues describe the same runtime gap under other service groupings (20261105T081225Z-events-lambda-ssm-the-lambda-python-runtime-has.md, 20261105T081727Z-events-lambda-sns-sqs-a-lambda-handler-cannot.md), and the open cloudfront__events issues (e.g. 20261023T140640Z-cloudfront-events-3-of-the-4-causes-remain-open.md, 20261029T175823Z-cloudfront-events-13-more-cloudfront-eventbridge-checks-fail.md) do not carry the replay keys of these eight checks.

Traces: see trace_ids.

## Evidence
Expected (AWS): Invoke -> HTTP 200, no `X-Amz-Function-Error`, Payload = the handler's JSON report (containing `ok`); the invalidation / SQS / DLQ effect the handler performs becomes observable well inside the checks' poll windows.

Actual (emulator):
- cloudfront__events.lambda-role-missing-cloudfront-permission-denied: HTTP 200 with `x-amz-function-error: Unhandled`; payload has no `ok` key -> `KeyError: 'ok'` at check_cloudfront__events_lambda_role_missing_cloudfront_permission_denied.py line 104 (`assert direct_payload["ok"] is False`).
- The other seven checks: `Check timed out after 60.0s` with no effect of the handler ever appearing (deploy-cfn-eventbridge-rule-cloudfront-invalidation-lambda, eventbridge-cloudfront-distribution-arn-resolution-in-target-input, eventbridge-cloudfront-event-missing-detail-type-not-matched, eventbridge-lambda-invalidation-failure-routes-to-dlq, eventbridge-lambda-target-missing-permission-blocks-delivery, eventbridge-rule-input-path-extracts-distribution-id, eventbridge-target-arn-resolves-to-specific-cloudfront-distribution).
- The import failure itself, as observed elsewhere in this same suite run for identically packaged handlers: {"errorMessage":"Unable to import module 'index': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}.

## Steps to Reproduce
1. CreateFunction (runtime python3.x) from a zip whose index.py does `import boto3` and calls cloudfront.create_invalidation.
2. Invoke it synchronously, or wire it as the target of an EventBridge rule and PutEvents a matching event.
3. Observe HTTP 200 with `x-amz-function-error: Unhandled` and a Runtime.ImportModuleError payload naming boto3; no invalidation is created and nothing reaches the target queue/DLQ.
4. Re-replay the eight checks in trace_ids to confirm.
