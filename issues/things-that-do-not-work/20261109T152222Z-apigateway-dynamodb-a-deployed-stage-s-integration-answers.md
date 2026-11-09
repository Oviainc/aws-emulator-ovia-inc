Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-cache-ttl-expiry-and-flush-eviction
Issue-Key: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-cache-ttl-expiry-and-flush-eviction|run#14|AssertionError|assert st == 200, f"backend {url} never answered 200 before caching: {st} {b!r}"
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__dynamodb: a deployed stage's integration answers 500, so stage-cache TTL expiry and flush eviction can never be exercised

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Cause 1 of 1: a deployed REST API stage never serves a successful response from its integration, so nothing can be cached and the cache contract is unreachable.
AWS: a deployed stage with a cache cluster and a per-method caching/ttlInSeconds setting answers the method from its integration (HTTP 200) on the first request, serves subsequent identical requests from the stage cache until the TTL elapses, and re-fetches from the integration after FlushStageCache or after the TTL expires.
Emulator: the stage never answers 200 — the first request through the method is answered HTTP 500 with the default gateway response {"message":"Internal server error"}, so the check never reaches the caching phase at all.
Why: because no response is ever produced, a caller can neither use the stage nor distinguish "cached" from "not cached" from "not served": the TTL-expiry and flush-eviction behaviour the stage cache exists for is entirely unobservable, and the 500 reports an internal failure where AWS returns the integration's result.
Fix: the execute-api invoke path's integration execution for a deployed stage (the 500 is produced before any caching decision), and then the stage-cache TTL/flush eviction it feeds.
Traces: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-cache-ttl-expiry-and-flush-eviction

## Evidence
"AssertionError: backend https://<host>/_aws/execute-api/<apiId>/ttl/short never answered 200 before caching: 500 '{\"message\":\"Internal server error\"}'". Expected: 200 with the integration's body, then a cache hit, then a miss after TTL/flush. The host, port and api id are per-run values.

## Steps to Reproduce
1. Create a REST API with a resource and method over a working integration; CreateDeployment to a stage with cacheClusterEnabled.
2. UpdateStage setting the method's caching/enabled and caching/ttlInSeconds.
3. GET the stage URL.
Expected: 200 with the integration's body (then a cached hit, then a miss after the TTL elapses or FlushStageCache). Actual: 500 {"message":"Internal server error"} for every attempt.
