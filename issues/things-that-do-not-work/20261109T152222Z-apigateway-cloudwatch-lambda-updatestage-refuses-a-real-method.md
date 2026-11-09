Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__lambda.obs-apigateway-1-cache-control-invalidation-rejected-403-when-unauthorized apigateway__cloudwatch__lambda.obs-apigateway-1-cache-hit-miss-metrics-track-flush-uncached-stage-has-none apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-stage-metrics-count-errors-latency-under-apiname-stage apigateway__cloudwatch__lambda.obs-apigateway-1-5xx-alarm-ok-to-alarm-notifies-sqs-via-sns apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-lambda-authorizer-denied-403-counts-4xx-not-5xx
Issue-Key: apigateway__cloudwatch__lambda|UpdateStage|BadRequestException
Issue-Key: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-stage-metrics-count-errors-latency-under-apiname-stage|_poll_until#12|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigateway__cloudwatch__lambda.obs-apigateway-1-5xx-alarm-ok-to-alarm-notifies-sqs-via-sns|left#33|AssertionError|raise AssertionError(f"{_DEADLINE_S}s check deadline exceeded during: {phase}")
Issue-Key: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-lambda-authorizer-denied-403-counts-4xx-not-5xx|run#13|AssertionError|assert (status, body) == (403, _DENY_BODY), (
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__cloudwatch__lambda: UpdateStage refuses a real method-setting path, stage metrics never reach CloudWatch, and the explicit-deny body omits AWS's policy clause (3 causes)

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Three net-new root causes. Numbered; each carries its own failing check ids.

Cause 1 of 3: UpdateStage refuses a methodSettings patch path whose {resourcePath}/{httpMethod} segments name an actual resource and method.
AWS: Smithy apigateway UpdateStage takes patchOperations and models BadRequestException (httpError 400). The supported methodSettings op paths are templates — /{resourcePath}/{httpMethod}/caching/enabled and siblings — and a caller fills them in with a real resource path and HTTP method (e.g. the caching/enabled setting of GET on the /ready resource). AWS applies the patch and returns 200 with the resulting methodSettings.
Emulator: such a patch is refused 400 BadRequestException "Invalid method setting path: /<resourcePath>/<httpMethod>/caching/enabled. Must be one of: [...]", where the enumerated list is the literal template strings — i.e. the validator compares the caller's filled-in path against the templates verbatim instead of matching them.
Why: per-method caching (and, by the same validator, per-method metrics/logging/throttling) cannot be configured at all, so every method-level stage setting is unreachable and the checks that depend on it (unauthorized cache-control strategies, cache hit/miss accounting and flush) cannot even be set up.
Fix: the UpdateStage handler's methodSettings patch-path validation — match /{resourcePath}/{httpMethod}/<setting> as a pattern, not as a literal.
Traces: apigateway__cloudwatch__lambda.obs-apigateway-1-cache-control-invalidation-rejected-403-when-unauthorized, apigateway__cloudwatch__lambda.obs-apigateway-1-cache-hit-miss-metrics-track-flush-uncached-stage-has-none

Cause 2 of 3: no AWS/ApiGateway stage metrics are published, so Count/4XXError/5XXError/Latency are absent and an alarm on them never fires.
AWS: a stage with metrics enabled emits AWS/ApiGateway Count, 4XXError, 5XXError and Latency under the (ApiName, Stage) dimension set for every request served; a CloudWatch alarm on 5XXError therefore transitions OK -> ALARM once 5xx traffic is driven, and notifies its SNS action.
Emulator: after 14 requests the (ApiName, Stage) Count Sum stays (0, 0) and no datapoints appear; driving 5xx traffic never moves the alarm out of OK within 600s.
Why: a client gets no telemetry for its API and no alarm/notification pipeline on API Gateway errors — the metric the alarm reads is never produced.
Fix: the execute-api request path's AWS/ApiGateway metric emission into the cloudwatch metric store (same component as the apigateway__cloudwatch finding).
Traces: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-stage-metrics-count-errors-latency-under-apiname-stage, apigateway__cloudwatch__lambda.obs-apigateway-1-5xx-alarm-ok-to-alarm-notifies-sqs-via-sns

Cause 3 of 3: the Lambda-authorizer explicit-deny body omits the clause naming which policy denied.
AWS: when a Lambda authorizer returns a policy with an explicit Deny for the requested method, API Gateway answers HTTP 403 with body {"Message":"User is not authorized to access this resource with an explicit deny in an identity-based policy"} — the fixed text includes the trailing "in an identity-based policy".
Emulator: HTTP 403 with {"Message":"User is not authorized to access this resource with an explicit deny"} — the status and code are right, the message is truncated.
Why: a client that distinguishes an identity-based explicit deny from the other 403 deny classes by this message cannot, and a message match against AWS's wording fails.
Fix: the execute-api authorizer denial serializer's explicit-deny message text.
Traces: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-lambda-authorizer-denied-403-counts-4xx-not-5xx

## Evidence
Cause 1: "botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the UpdateStage operation: Invalid method setting path: /ready/GET/caching/enabled. Must be one of: [/deploymentId, /description, ... /{resourcePath}/{httpMethod}/caching/enabled, ...]" (and the same for /c/GET/caching/enabled).
Cause 2: "Count Sum [{'Name': 'ApiName', 'Value': '<api>'}, {'Name': 'Stage', 'Value': 'live'}] reaching 14 not met within 240s. Last result: (0, 0)"; "600s check deadline exceeded during: driving 5xx until the alarm enters ALARM".
Cause 3: "deny 0: 403 b'{\"Message\":\"User is not authorized to access this resource with an explicit deny\"}', expected 403 b'{\"Message\":\"User is not authorized to access this resource with an explicit deny in an identity-based policy\"}'".

## Steps to Reproduce
1. Cause 1: create a REST API with a resource /ready and a GET method, deploy to a stage with a cache cluster, then UpdateStage(patchOperations=[{op:'replace', path:'/ready/GET/caching/enabled', value:'true'}]). Expect 200 and the setting applied.
2. Cause 2: drive N requests (including 5xx) through a metrics-enabled stage and read AWS/ApiGateway Count/5XXError under (ApiName, Stage); put a CloudWatch alarm on 5XXError with an SNS action and wait for ALARM.
3. Cause 3: attach a Lambda authorizer returning an explicit Deny policy for the method and GET the stage; compare the 403 body with AWS's text.
