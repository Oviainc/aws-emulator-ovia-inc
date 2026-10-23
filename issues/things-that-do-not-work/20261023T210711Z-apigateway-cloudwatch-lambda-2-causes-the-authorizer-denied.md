Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-lambda-authorizer-denied-403-counts-4xx-not-5xx apigateway__cloudwatch__lambda.obs-apigateway-1-cache-control-invalidation-rejected-403-when-unauthorized apigateway__cloudwatch__lambda.obs-apigateway-1-cache-hit-miss-metrics-track-flush-uncached-stage-has-none
Issue-Key: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-lambda-authorizer-denied-403-counts-4xx-not-5xx|_ready_then_align#11|Timeout|time.sleep(0.5)
Issue-Key: apigateway__cloudwatch__lambda|UpdateStage|NotImplemented
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__cloudwatch__lambda: 2 causes — the authorizer-denied 403 metric check hangs to the 60 s cap, and two cache/metric checks abort on UpdateStage 501 NotImplemented

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Two root causes. Open issue 20261023T170254Z-apigateway-cloudwatch-lambda-the-authorizer-denied-403-4xx.md describes a similar symptom but does not carry these replay keys.

Cause 1 of 2: a request to a deployed stage guarded by a Lambda authorizer never returns.
AWS: a request the authorizer denies is answered 403 at the stage and is counted in the stage's CloudWatch 4XXError metric, not 5XXError.
Emulator: no response arrives; the check exhausts the harness budget and is reported as "Check timed out after 60.0s", so neither the 403 nor the metric datapoint can be observed.
Why: the client stalls instead of being refused, and the 4xx-vs-5xx classification of an authorizer denial — the thing an operator alarms on — is unverifiable.
Fix: the execute-api data-plane request path (authorizer invocation and stage response) — it must always produce a response.
Traces: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-lambda-authorizer-denied-403-counts-4xx-not-5xx.

Cause 2 of 2: UpdateStage answers 501 NotImplemented, so stage caching can never be configured.
AWS: Smithy: UpdateStage returns the updated Stage (HTTP 200); modeled errors are BadRequestException 400, ConflictException 409, LimitExceededException 429, NotFoundException 404, TooManyRequestsException 429, UnauthorizedException 401. Enabling the stage cache and per-method caching is done with patch operations on the stage.
Emulator: HTTP 501 `NotImplemented`, message "operation UpdateStage is scaffolded but not implemented in this emulator".
Why: `NotImplemented` is outside the operation's modeled error set, and with caching unconfigurable the cache-hit/miss metrics and the 403 refusal of an unauthorized cache invalidation (Cache-Control: max-age=0 without the InvalidateCache permission) cannot exist.
Fix: the UpdateStage handler, services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/update_stage.rs.
Traces: apigateway__cloudwatch__lambda.obs-apigateway-1-cache-control-invalidation-rejected-403-when-unauthorized, apigateway__cloudwatch__lambda.obs-apigateway-1-cache-hit-miss-metrics-track-flush-uncached-stage-has-none.

## Evidence
Cause 1: detail is exactly "Error: Check timed out after 60.0s".
Cause 2: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateStage operation: operation UpdateStage is scaffolded but not implemented in this emulator (both checks).

## Steps to Reproduce
1. Cause 1: deploy a REST API with a Lambda TOKEN authorizer that denies, request the stage URL, and observe that no response arrives within 60 s.
2. Cause 2: call update_stage with patchOperations enabling cacheClusterEnabled / per-method caching and observe HTTP 501 NotImplemented.
3. Or replay the check ids in trace_ids.
