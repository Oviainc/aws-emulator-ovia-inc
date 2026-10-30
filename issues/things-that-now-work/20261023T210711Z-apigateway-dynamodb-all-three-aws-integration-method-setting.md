Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-cache-ttl-expiry-and-flush-eviction apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-unauthorized-cache-control-strategies
Issue-Key: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-cache-ttl-expiry-and-flush-eviction|_wait_for_status#14|Timeout|time.sleep(3)
Issue-Key: apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-unauthorized-cache-control-strategies|_wait_for_status#13|Timeout|time.sleep(3)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__dynamodb: the method-setting cache checks hang to the 60 s cap at the deployed stage

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a request to a deployed REST API stage never returns.
AWS: a deployed stage with an AWS-service integration to DynamoDB executes the request mapping template and answers with the integration's response (200 for the PutItem/GetItem legs here); with per-method caching enabled the stage serves a cached body until the TTL expires or a flush evicts it, and refuses an unauthorized Cache-Control invalidation rather than honouring it. Recorded behavior only for the per-case statuses; what is structural is that every request to a deployed stage produces an HTTP response.
Emulator: no response arrives. All three checks exhaust the harness budget and are reported as "Check timed out after 60.0s".
Why: the API Gateway-to-DynamoDB integration path and the stage cache (TTL expiry, flush eviction, unauthorized-invalidation strategy) are entirely unobservable, and the hang stalls the caller instead of failing it.
Fix: the execute-api data-plane request path — stage/route resolution, request-template evaluation and integration dispatch must always produce a response.
Traces: apigateway__dynamodb.apigw-aws-integration-mapping-template-writes-ddb, apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-cache-ttl-expiry-and-flush-eviction, apigateway__dynamodb.deep-apigateway-1-apigw-methodsetting-unauthorized-cache-control-strategies.

## Evidence
Detail for all three checks is exactly "Error: Check timed out after 60.0s" — no HTTP status line and no body ever arrives from the deployed stage.

## Steps to Reproduce
1. Create a DynamoDB table and a REST API with an AWS-service integration to dynamodb:PutItem driven by a request mapping template; deploy to a stage.
2. POST to the stage invoke URL and observe that no response arrives within 60 s.
3. Or replay the check ids in trace_ids.

## Resolution
Fixed for the stage-cache half of this report, which is the two method-setting traces
(`…methodsetting-cache-ttl-expiry-and-flush-eviction`, `…methodsetting-unauthorized-cache-control-strategies`).
The `execute-api` data plane now holds the response an integration produced for a method
whose settings in force enable caching on a cache cluster that is up, serves that held
response to every later request with the same cache key until the method's
`cacheTtlInSeconds` elapses, the cluster is emptied by `FlushStageCache` or re-provisioned
by `UpdateStage`, or a caller authorized for `execute-api:InvalidateCache` asks for it to be
reloaded with `Cache-Control: max-age=0`; an unauthorized ask is answered as the stage's
`unauthorizedCacheControlHeaderStrategy` says (403 through the api's `ACCESS_DENIED` gateway
response, served with the ignored header stated, or served silently). See
`services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs`
and its wire suite `tests/all/stage_response_cache.rs`.

STILL OPEN, tracked by its own report
(`issues/things-that-do-not-work/20261030T120000Z-apigateway-aws-service-integration-to-another-services-action-is-not-performed.md`),
which now carries that trace id and its `Issue-Key` so the key is tracked in ONE place:
the `apigw-aws-integration-mapping-template-writes-ddb` trace. An `AWS`-type integration
naming another service's action is still answered with the api's `API_CONFIGURATION_ERROR`
gateway response, because performing it needs two capabilities this emulator does not have
yet — see that report.
