Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-stage-metrics-count-errors-latency-under-apiname-stage apigateway__cloudwatch__lambda.obs-apigateway-1-cache-hit-miss-metrics-track-flush-uncached-stage-has-none
Issue-Key: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-stage-metrics-count-errors-latency-under-apiname-stage|_ready_then_align#7|Timeout|time.sleep(0.5)
Issue-Key: apigateway__cloudwatch__lambda.obs-apigateway-1-cache-hit-miss-metrics-track-flush-uncached-stage-has-none|_ready_then_align#23|Timeout|time.sleep(0.5)
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# apigateway__cloudwatch__lambda: both REST-stage metric checks hang to the 60 s cap — no traffic can be generated at the deployed stage, so Count/4XX/5XX/Latency and cache hit/miss metrics are unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: the deployed REST stage never answers, so the metric-generating traffic never happens and the checks hang.
AWS: driving N requests at a deployed stage emits the AWS/ApiGateway metrics Count, 4XXError, 5XXError and Latency dimensioned by ApiName/Stage, and CacheHitCount/CacheMissCount for a cache-enabled stage (zero for an uncached one); every request gets an HTTP response first. Recorded behavior only for the metric values; what is structural is that each request returns so the metrics can be published at all.
Emulator: both checks report exactly 'Check timed out after 60.0s' — the stage requests never produce a response, so GetMetricStatistics is never reached. Hypothesis: the dead execute-api data plane described in 20261023T210711Z-apigateway-api-gateway-data-plane-is-dead-30-checks.md and in the sibling issues 20261023T170254Z-apigateway-cloudwatch-lambda-the-authorizer-denied-403-4xx.md / 20261023T210711Z-apigateway-cloudwatch-lambda-2-causes-the-authorizer-denied.md, none of which carries these two keys.
Why: no API Gateway observability is reachable — a client cannot alarm on error rate or latency, and cannot tell a cache that is not working from a stage that is not serving. The hang stalls the caller instead of failing it.
Fix: the execute-api data-plane request path (stage/route resolution and integration dispatch) so a deployed stage always produces a response, and the per-request emission of the AWS/ApiGateway metrics that follow from it.
Traces: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-stage-metrics-count-errors-latency-under-apiname-stage, apigateway__cloudwatch__lambda.obs-apigateway-1-cache-hit-miss-metrics-track-flush-uncached-stage-has-none.

## Evidence
Error: Check timed out after 60.0s for both checks — no HTTP status line from the stage, so no metric datapoints were ever requested.

## Steps to Reproduce
1. create_rest_api -> put_method -> put_integration (AWS_PROXY to a Lambda) -> create_deployment -> create_stage (one cache-enabled with cacheClusterSize 0.5 and a 300 s TTL, one uncached).
2. Issue the request batch at each stage's invoke URL.
3. Observe the requests never return within the 60 s budget; cloudwatch.get_metric_statistics for Count/4XXError/5XXError/Latency and CacheHitCount/CacheMissCount is never reached.
4. Or replay the two check ids in trace_ids.
