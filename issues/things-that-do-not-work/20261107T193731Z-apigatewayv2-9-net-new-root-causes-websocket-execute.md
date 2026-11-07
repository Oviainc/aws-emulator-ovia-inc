Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.websocket-broadcast-to-multiple-connections apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect apigatewayv2.websocket-route-selection-by-action apigatewayv2.mock-integration-response-selected-by-status-code-pattern apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client apigatewayv2.custom-domain-mapping-key-routes-to-stage-routes apigatewayv2.e2e-custom-domain-mapping-invoke-flow apigatewayv2.deploy-api-mapping-cfn-behaves apigatewayv2.http-api-cors-configuration-overrides-backend-cors-headers apigatewayv2.reset-authorizers-cache-server-side apigatewayv2.http-api-cors-preflight-caught-by-any-route-is-authorized apigatewayv2.lambda-payload-format-1-0-and-2-0-envelopes-differ apigatewayv2.route-throttling-returns-429-while-twin-route-serves apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced
Issue-Key: apigatewayv2.websocket-broadcast-to-multiple-connections|_poll_until#20|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect|_poll_until#18|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2.websocket-route-selection-by-action|_poll_until#19|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2.mock-integration-response-selected-by-status-code-pattern|_poll_until#14|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client|_poll_until#14|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2.custom-domain-mapping-key-routes-to-stage-routes|_poll_until#20|Timeout|raise AssertionError(f"condition not met within {timeout_s}s; last observed value={last!r}")
Issue-Key: apigatewayv2.e2e-custom-domain-mapping-invoke-flow|_poll_until#18|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last observed value={last!r}")
Issue-Key: apigatewayv2.deploy-api-mapping-cfn-behaves|_poll_until#21|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last value={last!r}")
Issue-Key: apigatewayv2.http-api-cors-configuration-overrides-backend-cors-headers|run#11|AssertionError|assert status == 200 and body == "backend-served", (
Issue-Key: apigatewayv2.reset-authorizers-cache-server-side|_poll_invoke_until_ok#46|AssertionError|raise AssertionError(f"invoke never succeeded within {timeout_s}s: {last_err}")
Issue-Key: apigatewayv2.http-api-cors-preflight-caught-by-any-route-is-authorized|run#18|AssertionError|assert status == 204, (
Issue-Key: apigatewayv2.lambda-payload-format-1-0-and-2-0-envelopes-differ|run#33|AssertionError|assert v1_mv_headers.get("X-Sim2Real-Probe") == [marker], (
Issue-Key: apigatewayv2.route-throttling-returns-429-while-twin-route-serves|run#20|AssertionError|assert len(limited_429) >= 1, (
Issue-Key: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected|_http_until#18|AssertionError|raise AssertionError(
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered|_poll_http#17|AssertionError|raise AssertionError(
Issue-Key: apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced|_poll_http#18|Timeout|raise AssertionError(
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigatewayv2: 9 net-new root causes — WebSocket execute-api upgrade answers 404 UnknownService, custom-domain/API-mapping data plane never routes, AWS_PROXY routes answer 500, CORS preflight 404, payload-1.0 multiValueHeaders missing, route throttling not enforced, Lambda resource policy ignored, HTTP_PROXY answers 307, CFN CORS lifecycle never settles

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane), CreateApiMapping, CreateIntegration, UpdateStage/RouteSettings, CreateApi(CorsConfiguration)

## Description
Nine distinct root causes observed against the apigatewayv2 emulator in this replay. Related open issues that do NOT cover these checks (they carry other replay keys) and should be read alongside: 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md, 20261026T023803Z-apigatewayv2-custom-domain-and-api-mapping-control-plane.md, 20261101T041930Z-apigatewayv2-3-net-new-root-causes-the-http.md, 20261027T053233Z-apigatewayv2-an-http-api-aws-proxy-route-still.md, 20261105T154551Z-apigatewayv2-a-cdk-httpapi-whose-aws-lambda-permission.md, 20261106T180247Z-apigatewayv2-a-cfn-provisioned-http-api-with-corsconfiguration.md.

Cause 1 of 9: the WEBSOCKET execute-api data plane refuses every RFC 6455 upgrade with 404 UnknownService.
AWS: a deployed WEBSOCKET API stage answers an upgrade request on wss://<api-id>.execute-api.<region>.amazonaws.com/<stage> with 101 Switching Protocols and the Sec-WebSocket-Accept header, then dispatches frames by the API's RouteSelectionExpression. Recorded behavior only for the handshake itself (the Smithy model describes only the control plane; Integration.IntegrationResponseSelectionExpression / TemplateSelectionExpression are documented "Supported only for WebSocket APIs", which presumes a served socket).
Emulator: every upgrade attempt is answered 'HTTP/1.1 404 Not Found' with headers content-type: application/x-amz-json-1.1 and x-amzn-errortype: UnknownService — i.e. the front door never recognises the execute-api path at all and routes it to its unknown-service handler, so no handshake, no route dispatch, no @connections push and no post-close GoneException is observable. The fixed text of the diverging response is the 404 status, the x-amzn-errortype: UnknownService header and a JSON error body; the api id and stage in the request URI are request values.
Why: a client that opens a WebSocket against a deployed stage gets a hard 404 instead of a live socket, so every WebSocket feature (route selection, two-way routes, broadcast, GoneException after disconnect) is unusable.
Fix: the execute-api front door / request router must recognise an Upgrade: websocket request on the execute-api WebSocket path and hand the connection to the apigatewayv2 WebSocket data-plane handler instead of the unknown-service responder.
Traces: apigatewayv2.websocket-broadcast-to-multiple-connections, apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect, apigatewayv2.websocket-route-selection-by-action, apigatewayv2.mock-integration-response-selected-by-status-code-pattern, apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client (the last one records no response at all rather than a 404).

Cause 2 of 9: a custom DomainName with an ApiMapping never routes to the mapped stage.
AWS: once CreateDomainName and CreateApiMapping (ApiMappingKey -> stage) have succeeded, an HTTPS request to https://<domain>/<mapping-key>/<route> is served by that stage's routes, and DeleteApiMapping removes it immediately.
Emulator: requests to the custom domain never produce a served response — the probe either never observes any status (last observed value None) or the connection is actively refused ('[Errno 111] Connection refused'), so the mapping-key-to-stage routing contract reaches no verdict.
Why: clients that front an API with a custom domain (the normal production shape, including CFN/CDK stacks that create DomainName + ApiMapping) get nothing on the domain even though the control plane reports the mapping exists.
Fix: the custom-domain data plane — the listener/vhost resolution that maps a Host header plus mapping key to an api+stage — must serve mapped requests; the ApiMapping state must be consulted by the data-plane router.
Traces: apigatewayv2.custom-domain-mapping-key-routes-to-stage-routes, apigatewayv2.e2e-custom-domain-mapping-invoke-flow, apigatewayv2.deploy-api-mapping-cfn-behaves.

Cause 3 of 9: an HTTP API AWS_PROXY (Lambda) route answers 500 instead of the function's proxy response.
AWS: a route whose integration is AWS_PROXY onto a Lambda function returns the function's proxy response — statusCode, headers and body as the handler produced them (200 with the handler's body in these scenarios).
Emulator: the route answers HTTP 500 with the generic body {"message":"Internal Server Error"} on every attempt; the body is the emulator's own text, not the integration's. ('Internal Server Error' is AWS's generic 500 body text, but AWS emits it only when the integration genuinely failed.)
Why: no Lambda-backed HTTP API behaviour can be exercised — CORS header precedence over backend headers, authorizer caching, and any application response are all masked by a 500.
Fix: the execute-api HTTP data-plane integration path for AWS_PROXY — invoke the target function and serialise its proxy response — and the 500 fallback must only be used when the invoke or the envelope actually fails.
Traces: apigatewayv2.http-api-cors-configuration-overrides-backend-cors-headers, apigatewayv2.reset-authorizers-cache-server-side.

Cause 4 of 9: a CORS preflight with no matching OPTIONS route is answered 404 instead of being answered by the API's CorsConfiguration.
AWS: for an HTTP API carrying a CorsConfiguration, API Gateway answers the OPTIONS preflight itself with 204 No Content and the Access-Control-Allow-Origin / -Allow-Methods / -Allow-Headers / Max-Age headers derived from that configuration, even when no OPTIONS route exists. Smithy: Cors (AllowOrigins, AllowMethods, AllowHeaders, ExposeHeaders, MaxAge range -1..86400), documented "Supported only for HTTP APIs. See Configuring CORS".
Emulator: the preflight is answered 404 with body {"message":"Not Found"}; the gateway-level CORS responder is never consulted. Fixed text of the correct response: status 204 with an empty body; its variable parts are the echoed Origin and the configured method/header lists.
Why: browsers block every cross-origin call to such an API, because the preflight fails even though the API declares CORS.
Fix: the execute-api HTTP data plane's request router must short-circuit OPTIONS preflights against the API's CorsConfiguration before route matching / 404 generation.
Traces: apigatewayv2.http-api-cors-preflight-caught-by-any-route-is-authorized.

Cause 5 of 9: the payload format 1.0 Lambda event omits multiValueHeaders and does not preserve the request's original header casing.
AWS: an AWS_PROXY integration with PayloadFormatVersion 1.0 hands the function an event containing multiValueHeaders, a map from each request header name — in the casing the client sent — to the list of its values; format 1.0 does not lowercase header names (2.0 has no multiValueHeaders and lowercases). Smithy: Integration.PayloadFormatVersion, "Supported values for Lambda proxy integrations are 1.0 and 2.0".
Emulator: the 1.0 event's multiValueHeaders lookup for the originally-cased request header yields None — the member is absent (or keyed only in lowercase), so the two payload formats are not distinguishable.
Why: handlers written for format 1.0 that read multiValueHeaders (repeated headers, case-sensitive header names) see nothing and mis-handle the request.
Fix: the AWS_PROXY payload serialiser for PayloadFormatVersion 1.0 — emit multiValueHeaders with original request casing, and keep 1.0/2.0 envelope differences (queryStringParameters last-value, cookies splitting) distinct.
Traces: apigatewayv2.lambda-payload-format-1-0-and-2-0-envelopes-differ.

Cause 6 of 9: per-route throttling in RouteSettings is not enforced — no request is ever shed with 429.
AWS: a stage route setting with ThrottlingRateLimit and ThrottlingBurstLimit sheds requests above the token bucket with HTTP 429 and body {"message":"Too Many Requests"}, while a sibling route without the setting keeps serving 200. Smithy: RouteSettings.ThrottlingRateLimit (double), RouteSettings.ThrottlingBurstLimit (integer).
Emulator: a route limited to rate 1.0 / burst 1 answered an entire burst with 200 — every one of the requests was served, so the limiter is not applied at all. The requirement is the 429 status and the Too Many Requests message text; the rate, burst and burst size are request values.
Why: clients and tests that depend on throttling (back-pressure, 429 retry handling, quota behaviour) see an API that never throttles.
Fix: the execute-api data plane's per-route rate limiter, reading the stage's RouteSettings state.
Traces: apigatewayv2.route-throttling-returns-429-while-twin-route-serves.

Cause 7 of 9: the target Lambda function's resource policy is never evaluated — a route whose function grants API Gateway no lambda:InvokeFunction still serves 200.
AWS: if the function's resource policy does not allow apigateway.amazonaws.com (or the API's source ARN) to call lambda:InvokeFunction, the HTTP API route answers 500 with body {"message":"Internal Server Error"} and the function is not invoked.
Emulator: the route answers 200 with the handler's own body, proving the integration invoked the function without consulting its resource policy.
Why: a permission misconfiguration that AWS refuses is silently permitted, so IaC that (de)provisions AWS::Lambda::Permission appears to work either way and the emulator cannot be used to validate least-privilege wiring.
Fix: the AWS_PROXY integration invoke path must evaluate the target function's resource policy before invoking and answer 500 when the invoke is not permitted.
Traces: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected.

Cause 8 of 9: an HTTP_PROXY integration answers 307 instead of proxying the request, so the 10 MiB payload ceiling is unobservable.
AWS: an HTTP_PROXY route forwards the request to the configured IntegrationUri and returns the upstream response (a 2xx for a small accepted body); a request body above the HTTP API maximum payload size (10 MiB, not adjustable) is refused by the gateway with 413 and the body is never delivered upstream.
Emulator: every attempt at the proxied route answers 307 with an empty body and never reaches an accepted status, so neither the proxy path nor the 413 ceiling can be graded. The requirement is that a small body is proxied (upstream status) and an over-limit body is answered 413; the host, api id and stage in the URI are per-run values.
Why: HTTP_PROXY integrations are unusable and the payload ceiling is unenforced — a client can push an arbitrarily large body and gets a redirect, not the gateway's refusal.
Fix: the execute-api HTTP data plane's HTTP_PROXY integration handler (it must not emit a redirect) and the request-size gate that produces 413.
Traces: apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered.

Cause 9 of 9: a CFN-provisioned HTTP API with CorsConfiguration never completes the CORS-removed-in-place-then-API-replaced lifecycle.
AWS: removing CorsConfiguration by a stack update takes effect in place on the same API id (preflights stop being answered by CORS), and a change that requires replacement creates a new API id that serves the same routes; each step settles in bounded time.
Emulator: the check reaches no verdict at all — it exhausts the 180 s harness cap, i.e. one of the update/serve steps never settles.
Why: a client cannot tell whether a CORS change has been applied; stack updates that AWS completes hang here.
Fix: the CFN apigatewayv2 Api resource update path (in-place CorsConfiguration removal vs replacement) together with the stage serve path it depends on.
Traces: apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced.

## Evidence
Cause 1: expected 101 Switching Protocols; got repeated 'HTTP/1.1 404 Not Found' with 'content-type: application/x-amz-json-1.1' and 'x-amzn-errortype: UnknownService' (e.g. "a 101 WebSocket upgrade at wss://<host>/_aws/execute-api/<api-id>/<stage> not met within 120.0s. Last result: {'status': 404, ... x-amzn-errortype: UnknownService ...}"); the two-way-route check records 'Last result: None'.
Cause 2: "condition not met within 90s; last observed value=None" and "condition not met within 150.0s; last value=(None, '[Errno 111] Connection refused')"; the mapping-key routing check times out at 180 s.
Cause 3: "allowed origin: expected 200 + backend body, got 500: '{\"message\":\"Internal Server Error\"}'" and "invoke never succeeded within 60s: HTTP Error 500: Internal Server Error".
Cause 4: "with no route matching OPTIONS the preflight is answered by CORS; expected 204, got 404: '{\"message\":\"Not Found\"}'".
Cause 5: "format 1.0 multiValueHeaders must list each header value under the ORIGINAL request casing ... got None".
Cause 6: "a route limited to 1.0 rps / burst 1 must shed part of a 24-request burst with 429; got statuses [200, 200, ... 200]" (24 x 200, no 429).
Cause 7: "a route whose Lambda grants API Gateway no lambda:InvokeFunction must be answered 500, got 200 with body '<marker>:/ungranted'".
Cause 8: "PUT /warm proxying a small body to S3 (https://<host>/_aws/execute-api/<api-id>/$default/warm) never reached an accepted status within 150.0s. Last: 307 ''".
Cause 9: "Check timed out after 180.0s".

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks listed in trace_ids. Per cause: (1) CreateApi ProtocolType=WEBSOCKET, routes + integration, CreateStage/CreateDeployment, then open an RFC 6455 upgrade against the stage's execute-api WebSocket URL. (2) CreateDomainName + CreateApiMapping onto an HTTP API stage, then GET https://<domain>/<mapping-key>/<route>. (3) HTTP API + AWS_PROXY Lambda route, GET the route. (4) CreateApi ProtocolType=HTTP with CorsConfiguration and no OPTIONS route, then send OPTIONS with Origin + Access-Control-Request-Method. (5) Two AWS_PROXY integrations on one function, PayloadFormatVersion 1.0 and 2.0, send a request with a mixed-case header and compare the echoed events. (6) UpdateStage RouteSettings ThrottlingRateLimit=1.0, ThrottlingBurstLimit=1 on one route, fire a 24-request burst. (7) Create an AWS_PROXY route and remove/omit the AWS::Lambda::Permission for apigateway, then GET the route. (8) Create an HTTP_PROXY route to an upstream URL and PUT a small body, then one byte over 10 MiB. (9) Deploy a CFN HTTP API with CorsConfiguration, update it to remove CorsConfiguration, then force replacement.
