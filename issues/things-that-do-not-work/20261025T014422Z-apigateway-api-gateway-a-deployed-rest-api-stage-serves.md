Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration apigateway.apigw-usage-plan-daily-quota-exhaustion-rejects-over-http apigateway.apigw-cache-key-parameters-index-the-stage-cache-per-declared-key-only apigateway.apigw-import-rest-api-aws-proxy-integration-deploys-and-invokes-lambda apigateway.deployed-stage-serves-mock-integration apigateway.e2e-authorizer-cache-invalidation-flow apigateway.e2e-usage-plan-key-throttle-consequence apigateway.stage-serves-deployed-snapshot-not-live-config
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration|run#37|AssertionError|assert denied_status == 403, (
Issue-Key: apigateway.apigw-usage-plan-daily-quota-exhaustion-rejects-over-http|run#28|AssertionError|assert status == 403, (
Issue-Key: apigateway.apigw-cache-key-parameters-index-the-stage-cache-per-declared-key-only|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-import-rest-api-aws-proxy-integration-deploys-and-invokes-lambda|_poll_http#30|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.deployed-stage-serves-mock-integration|_poll_http#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.e2e-authorizer-cache-invalidation-flow|_poll_until#40|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.e2e-usage-plan-key-throttle-consequence|run#27|Timeout|time.sleep(1.0)
Issue-Key: apigateway.stage-serves-deployed-snapshot-not-live-config|_poll_http#10|Timeout|time.sleep(interval_s)
Issue-Commit: 2c888af5aa33aca2a1e4835e22187ff259bac9bc

# API Gateway: a deployed REST API stage serves no data plane — the invoke URL answers 404 UnknownService and traffic checks hang to the 60 s cap

> _Note: this issue was identified against the code base at commit `2c888af5aa33aca2a1e4835e22187ff259bac9bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: an HTTPS request to a deployed REST API stage's invoke URL is never routed to the stage's methods; the emulator's front door treats it as an unresolvable control-plane call.

AWS: once CreateDeployment has published a stage, the stage's execute-api invoke URL serves the API's methods. Smithy: CreateDeployment (POST /restapis/{restApiId}/deployments, 201) returns a Deployment whose apiSummary is the published method snapshot; the invoke URL is the data plane for that snapshot. A request to a method with apiKeyRequired=true and a mismatched x-api-key is refused at the edge with HTTP 403 before the integration is invoked (recorded behavior: 403 Forbidden, integration never reached); requests that consume a usage-plan daily quota answer 200 while admitted and 429 once the quota is exhausted; a MOCK integration answers its configured integration response. None of these are control-plane calls — they are plain HTTPS requests to the stage host.

Emulator: the request is handled by the generic AWS request router, which cannot identify a target service and answers HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}. Checks that assert a single invoke see that 404 directly; checks that poll the invoke URL for the stage to come up never observe a well-formed response and hang until the harness 60 s cap. So no API Gateway v1 data-plane behaviour — API-key enforcement, usage-plan quota/throttle metering, stage cache keying, deployed-snapshot-vs-live-config isolation, authorizer caching, AWS_PROXY Lambda invocation — can be exercised at all.

Why: a client that deploys a REST API and then calls it gets a 404 with an AWS-internal error shape instead of its own API's response. Every integration type, every authorization mode and every metering rule on the v1 data plane is unobservable, and clients that retry until the stage serves (the normal post-deploy pattern) loop forever instead of converging.

Fix: the execute-api data-plane listener for API Gateway v1 — host/path recognition for the stage invoke URL (`{restApiId}.execute-api.<region>.<host>/{stageName}/{path}`) must dispatch to the deployed stage's method snapshot instead of falling through to the control-plane service router, and the stage request pipeline (API-key check, usage-plan metering, authorizer cache, integration dispatch) must run behind it.

Traces: apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration and apigateway.apigw-usage-plan-daily-quota-exhaustion-rejects-over-http show the 404 UnknownService body verbatim; the other six listed check ids hang to the cap at the same URL. Note the already-open per-composite issues 20261023T210711Z-apigateway-cloudfront-s3-every-cloudfront-s3-integration-check.md and 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md report the same symptom for other service tuples and for v2; none of them carries these `apigateway`-only check ids.

## Evidence
apigateway.apigw-apikeyrequired-mismatched-key-rejected-before-proxy-integration:
  expected: 403 Forbidden for a mismatched x-api-key (integration not invoked)
  actual:   404, body b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'

apigateway.apigw-usage-plan-daily-quota-exhaustion-rejects-over-http:
  expected: only 200 (admitted), 429 (quota) or 403 (association not yet effective) while consuming the quota
  actual:   404 '{"__type":"UnknownService","message":"could not resolve a target service from the request ..."}'; sequence so far [404]

Hangs to the 60 s harness cap while polling the same invoke URL:
  apigateway.apigw-cache-key-parameters-index-the-stage-cache-per-declared-key-only — Check timed out after 60.0s
  apigateway.apigw-import-rest-api-aws-proxy-integration-deploys-and-invokes-lambda — Check timed out after 60.0s
  apigateway.deployed-stage-serves-mock-integration — Check timed out after 60.0s
  apigateway.e2e-authorizer-cache-invalidation-flow — Check timed out after 60.0s
  apigateway.e2e-usage-plan-key-throttle-consequence — Check timed out after 60.0s
  apigateway.stage-serves-deployed-snapshot-not-live-config — Check timed out after 60.0s

## Steps to Reproduce
1. CreateRestApi, PutMethod (MOCK integration, 200 integration response) on the root resource, CreateDeployment with stageName=prod.
2. Issue a plain HTTPS GET to the stage invoke URL for that method.
3. Observe HTTP 404 with body {"__type":"UnknownService",...} instead of the MOCK integration's 200 response.
4. Repeat with apiKeyRequired=true and a mismatched x-api-key header: expect 403, observe the same 404.
5. Replay the eight check ids in trace_ids; the polling ones never converge and hit the 60 s cap.
