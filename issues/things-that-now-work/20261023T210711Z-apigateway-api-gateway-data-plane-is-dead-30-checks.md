Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-cache-key-parameters-index-the-stage-cache-per-declared-key-only apigateway.apigw-concurrent-invocations-through-one-stage-keep-per-request-identity apigateway.apigw-cors-preflight-served-from-mock-options-header-mappings apigateway.apigw-default-execute-api-endpoint-disabled-returns-403-until-reenabled apigateway.apigw-extended-request-id-is-minted-per-api-per-request-not-echoed apigateway.apigw-gateway-response-customization-is-served-for-an-unmatched-route apigateway.apigw-http-proxy-passes-cors-header-plain-http-integration-drops-it apigateway.apigw-iam-auth-unknown-access-key-rejected-at-stage apigateway.apigw-iam-auth-unsigned-request-rejected-at-stage apigateway.apigw-iam-auth-wrong-secret-signature-rejected-at-stage apigateway.apigw-payload-over-the-ten-megabyte-limit-rejected-with-413 apigateway.apigw-response-compression-gzips-above-the-minimum-and-not-below apigateway.apigw-response-override-status-requires-numeric-value apigateway.apigw-response-template-selected-by-accept-header apigateway.apigw-stage-cache-serves-a-stale-body-until-flush-invalidates-it apigateway.apigw-stage-variable-selects-a-different-backend-per-stage-from-one-deployment apigateway.apigw-strict-passthrough-rejects-an-unmapped-content-type-with-415 apigateway.authorizer-ttl-caches-result-within-window apigateway.authorizer-ttl-zero-disables-caching apigateway.canary-deployment-traffic-shift-and-promotion apigateway.deployed-stage-serves-mock-integration apigateway.e2e-authorizer-cache-invalidation-flow apigateway.flush-stage-authorizers-cache-bypasses-ttl apigateway.flush-stage-authorizers-cache-forces-reinvoke apigateway.gateway-response-default-4xx-customized-at-stage apigateway.greedy-proxy-and-path-parameter-routing apigateway.mock-selection-pattern-selects-status-and-headers apigateway.resource-policy-denies-unlisted-path-at-stage apigateway.stage-serves-deployed-snapshot-not-live-config apigateway.unrouted-path-and-method-rejected-at-stage apigateway.apigw-request-template-selected-by-content-type-drives-mock-status apigateway.apigw-vtl-request-and-response-templates-transform-a-real-body apigateway.concurrent-create-deployment-serializes-stage-update apigateway.concurrent-stage-update-serializes apigateway.delete-deployment-active-on-stage-rejected apigateway.delete-deployment-referenced-by-stage-conflict apigateway.delete-integration-response-nonexistent-error-contract
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.apigw-cache-key-parameters-index-the-stage-cache-per-declared-key-only|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-concurrent-invocations-through-one-stage-keep-per-request-identity|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-cors-preflight-served-from-mock-options-header-mappings|_poll#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-default-execute-api-endpoint-disabled-returns-403-until-reenabled|_poll_http#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-extended-request-id-is-minted-per-api-per-request-not-echoed|_poll_http#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-gateway-response-customization-is-served-for-an-unmatched-route|_poll_http#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-http-proxy-passes-cors-header-plain-http-integration-drops-it|_poll_http#20|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-iam-auth-unknown-access-key-rejected-at-stage|_poll#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-iam-auth-unsigned-request-rejected-at-stage|_poll#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-iam-auth-wrong-secret-signature-rejected-at-stage|_poll#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-payload-over-the-ten-megabyte-limit-rejected-with-413|_poll_http#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-response-compression-gzips-above-the-minimum-and-not-below|_poll_raw#20|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-response-override-status-requires-numeric-value|_poll#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-response-template-selected-by-accept-header|_poll#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-stage-cache-serves-a-stale-body-until-flush-invalidates-it|_poll_body#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-stage-variable-selects-a-different-backend-per-stage-from-one-deployment|_poll_http#9|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-strict-passthrough-rejects-an-unmapped-content-type-with-415|_poll_http#25|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.authorizer-ttl-caches-result-within-window|_http_get_retry#21|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.authorizer-ttl-zero-disables-caching|_wait_http_ok#33|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.canary-deployment-traffic-shift-and-promotion|_poll_http#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.deployed-stage-serves-mock-integration|_poll_http#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.e2e-authorizer-cache-invalidation-flow|_poll_until#40|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.flush-stage-authorizers-cache-bypasses-ttl|_get_json_with_retries#19|Timeout|time.sleep(3)
Issue-Key: apigateway.flush-stage-authorizers-cache-forces-reinvoke|_http_get_retry#20|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.gateway-response-default-4xx-customized-at-stage|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.greedy-proxy-and-path-parameter-routing|_poll_http#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.mock-selection-pattern-selects-status-and-headers|_poll_http#16|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.resource-policy-denies-unlisted-path-at-stage|_poll_http#18|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.stage-serves-deployed-snapshot-not-live-config|_poll_http#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.unrouted-path-and-method-rejected-at-stage|_poll_http#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-request-template-selected-by-content-type-drives-mock-status|run#12|AssertionError|assert (status, _json(text)) == (200, {"routed": "json"}), (
Issue-Key: apigateway.apigw-vtl-request-and-response-templates-transform-a-real-body|run#21|AssertionError|assert status == 200, (
Issue-Key: apigateway.concurrent-create-deployment-serializes-stage-update|run#20|AssertionError|assert results["update_stage_error"] == "ConflictException", (
Issue-Key: apigateway.concurrent-stage-update-serializes|run#13|AssertionError|assert successes >= 1, "at least one of the two concurrent UpdateStage calls must succeed"
Issue-Key: apigateway.delete-deployment-active-on-stage-rejected|run#7|AssertionError|assert code == "BadRequestException", (
Issue-Key: apigateway.delete-deployment-referenced-by-stage-conflict|run#8|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code}"
Issue-Key: apigateway.delete-integration-response-nonexistent-error-contract|run#11|AssertionError|assert code == "NotFoundException", f"expected NotFoundException deleting a never-created response, got {code}"
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# API Gateway data plane is dead: 30 checks driving a deployed stage hang to the 60 s cap, the invoke URL answers 404 UnknownService when it does answer, and five control-plane checks abort on 501 stubs (3 causes)

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Three root causes. Open issues 20261023T114649Z-apigateway-api-gateway-execute-api-stage-endpoints-are-not.md, 20261023T113357Z-apigateway-api-gateway-a-deployed-rest-api-s-invoke.md and 20261023T125454Z-apigateway-traffic-driven-at-a-deployed-rest-api-and.md describe the same family of symptoms but none of them carries the 37 replay keys listed here, so those keys are unowned and are filed as this work item.

Cause 1 of 3: a request to a deployed stage's invoke URL never completes.
AWS: a REST API with a method, an integration and a deployment serves traffic at its stage invoke URL; every request gets an HTTP response (200 for a MOCK/HTTP/AWS integration that matches, or a modeled gateway response such as 403 for a missing base-path mapping / unauthorized caller, 413 for a payload over the fixed 10485760-byte API payload quota, 415 for an unmapped content type with strict passthrough, 429 for a throttle). Recorded behavior only for the per-case statuses; what is structural is that a well-formed HTTP status line always arrives.
Emulator: no response arrives at all. 30 checks that drive the deployed stage over HTTP exhaust the harness's 60 s budget and are reported as "Check timed out after 60.0s". Nothing distinguishes a hung integration from a hung router: the connection simply never produces a response.
Why: every data-plane behaviour of API Gateway is unobservable — routing, mapping templates, caching, compression, gateway responses, IAM/authorizer enforcement, canary traffic split, request-id minting. A client cannot make a single successful call against an API it just deployed, and a hang (rather than an error) also stalls the caller instead of failing it.
Fix: the execute-api data-plane request path — stage/route resolution and integration dispatch for a deployed REST API stage; it must always produce a response.
Traces: the 30 timing-out check ids in trace_ids, from apigateway.apigw-cache-key-parameters-index-the-stage-cache-per-declared-key-only through apigateway.unrouted-path-and-method-rejected-at-stage.

Cause 2 of 3: when the invoke URL does answer, it answers 404 UnknownService instead of running the stage.
AWS: the stage invoke URL is served by the execute-api data plane and returns the integration's response (HTTP 200 for the MOCK integrations these checks deploy).
Emulator: HTTP 404 with body `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}` — i.e. the request is handed to the emulator's generic AWS-API front door, which cannot classify it, so the execute-api host/path is not bound to the API Gateway data plane at all.
Why: the response is not an API Gateway response of any kind. A client sees a service-routing error with an error code (`UnknownService`) that API Gateway never emits, instead of its integration's body, so no request/response mapping, content-type selection or VTL transformation can be observed.
Fix: the emulator's front-door request routing — recognise the execute-api stage endpoint and dispatch it to the API Gateway data plane instead of the SigV4/X-Amz-Target service resolver.
Traces: apigateway.apigw-request-template-selected-by-content-type-drives-mock-status, apigateway.apigw-vtl-request-and-response-templates-transform-a-real-body.

Cause 3 of 3: five control-plane checks abort on operations that still answer 501 NotImplemented.
AWS: Smithy: UpdateStage, DeleteDeployment and DeleteIntegrationResponse are modeled operations that return their resource representation (or 204) and refuse bad input with BadRequestException 400 / ConflictException 409 / NotFoundException 404.
Emulator: each answers HTTP 501 with error code `NotImplemented` and the message "operation <Op> is scaffolded but not implemented in this emulator", so the checks never reach the behaviour they assert (concurrent UpdateStage serialization, refusing deletion of a deployment that a stage still points at, NotFoundException for deleting a never-created integration response).
Why: the client gets an error code that is not in the operation's modeled error set, so no retry/branch logic matches it, and the state transition never happens.
Fix: the UpdateStage, DeleteDeployment and DeleteIntegrationResponse handlers in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/. Existing stub issues 20261023T125454Z-apigateway-updatestage-stub.md and 20261023T170254Z-apigateway-* name those files; these five replay keys are not attached to any of them.
Traces: apigateway.concurrent-create-deployment-serializes-stage-update, apigateway.concurrent-stage-update-serializes, apigateway.delete-deployment-active-on-stage-rejected, apigateway.delete-deployment-referenced-by-stage-conflict, apigateway.delete-integration-response-nonexistent-error-contract.

## Evidence
Cause 1 (30 checks): detail is exactly "Error: Check timed out after 60.0s" for every one — no HTTP status, no body, the deployed-stage request never returns. Example: apigateway.deployed-stage-serves-mock-integration (a MOCK integration deployed to a stage, then fetched over HTTPS) and apigateway.apigw-payload-over-the-ten-megabyte-limit-rejected-with-413 (expected HTTP 413 with AWS's fixed body "HTTP content length exceeded 10485760 bytes.").
Cause 2 (2 checks): apigateway.apigw-request-template-selected-by-content-type-drives-mock-status — "AssertionError: Content-Type application/json must select the 200 template; got (404, '{\"__type\":\"UnknownService\",\"message\":\"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)\"}')". apigateway.apigw-vtl-request-and-response-templates-transform-a-real-body — "expected 200 ... got 404: '{\"__type\":\"UnknownService\", ...}'".
Cause 3 (5 checks): "AssertionError: UpdateStage raced and failed with an unexpected code: NotImplemented"; "AssertionError: at least one of the two concurrent UpdateStage calls must succeed"; "AssertionError: expected BadRequestException deleting a deployment active on a stage, got NotImplemented"; "AssertionError: expected BadRequestException, got NotImplemented"; "AssertionError: expected NotFoundException deleting a never-created response, got NotImplemented".

## Steps to Reproduce
1. create_rest_api -> get_resources (root) -> put_method(authorizationType='NONE') -> put_integration(type='MOCK', requestTemplates={'application/json': '{"statusCode": 200}'}) -> put_method_response(statusCode='200') -> put_integration_response(statusCode='200') -> create_deployment(stageName='prod').
2. Issue an HTTPS GET at the stage's invoke URL. Observe either no response at all within 60 s (cause 1) or HTTP 404 with the UnknownService body (cause 2).
3. For cause 3, call update_stage / delete_deployment / delete_integration_response against the same API and observe HTTP 501 NotImplemented.
4. Or replay the check ids in trace_ids directly.
