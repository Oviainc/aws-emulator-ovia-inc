Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served apigatewayv2.http-api-path-without-stage-prefix-not-delivered apigatewayv2.lambda-payload-format-1-0-and-2-0-envelopes-differ apigatewayv2.parameter-mapping-rewrites-a-real-request-and-response apigatewayv2.deep-apigatewayv2-2-enforce-integration-timeout-503-while-twin-route-serves apigatewayv2.e2e-custom-domain-mapping-invoke apigatewayv2.deep-apigatewayv2-3-stagesem-request-authorizer-cache-expires-after-ttl apigatewayv2.client-initiated-close-invokes-disconnect-route apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected apigatewayv2.http-api-cors-preflight-caught-by-any-route-is-authorized apigatewayv2.route-throttling-returns-429-while-twin-route-serves
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served|run#34|AssertionError|assert status == 403, (
Issue-Key: apigatewayv2.http-api-path-without-stage-prefix-not-delivered|run#16|AssertionError|assert status == 404, f"{label}: expected 404, got {status}: {body[:300]!r}"
Issue-Key: apigatewayv2.lambda-payload-format-1-0-and-2-0-envelopes-differ|run#34|AssertionError|assert v1_mv_headers.get("X-Sim2Real-Probe") == [marker], (
Issue-Key: apigatewayv2.parameter-mapping-rewrites-a-real-request-and-response|run#37|AssertionError|assert mapped_status == 201, (
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-integration-timeout-503-while-twin-route-serves|_poll_until#31|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2.e2e-custom-domain-mapping-invoke|_poll_domain_invoke#18|PublishedUrlRejected|url = f"{ctx.endpoint_url(gw_host)}{path}"
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-request-authorizer-cache-expires-after-ttl|run#45|AssertionError|assert inside_samples >= 3, (
Issue-Key: apigatewayv2.client-initiated-close-invokes-disconnect-route|_open_ws_connection#3|Timeout|time.sleep(2)
Issue-Key: apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered|_poll_http#18|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot|run#45|Timeout|assert not wrong, (
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected|_http#28|Timeout|with urllib.request.urlopen(req, timeout=timeout_s) as resp:
Issue-Key: apigatewayv2.http-api-cors-preflight-caught-by-any-route-is-authorized|_poll_http#18|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.route-throttling-returns-429-while-twin-route-serves|_http#18|Timeout|with urllib.request.urlopen(req, timeout=timeout_s) as resp:
Issue-Commit: 6cc05b66b5dc4f3acc804c4a889717eb31d245db

# apigatewayv2: 9 net-new causes — AWS_IAM routes serve unsigned requests, missing-stage path answers 403 not 404, 1.0 payload omits multiValueHeaders, overwrite:statuscode ignored, timed-out integration never dispatched, custom domain published on an AWS host, and seven data-plane scenarios reach no verdict

> _Note: this issue was identified against the code base at commit `6cc05b66b5dc4f3acc804c4a889717eb31d245db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoute

## Description
Nine net-new root causes in the apigatewayv2 execute-api data plane and in the address it publishes for a custom domain. Each must be closed before submitting.

Cause 1 of 9: an AWS_IAM-authorized route performs no SigV4 verification.
AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2) CreateRoute.AuthorizationType -> AuthorizationType enum NONE|AWS_IAM|CUSTOM|JWT, documented "AWS_IAM for using AWS IAM permissions". A request to such a route whose SigV4 signature does not verify is refused at the data plane with HTTP 403 and the gateway envelope {"message":"Forbidden"} (fixed text), and only a verifying signature is dispatched to the integration.
Emulator: the request with a non-verifying signature was answered 200 with the integration's body (the check saw the backend marker), i.e. the signature is never checked.
Why: authorization bypass — any unauthenticated caller reaches an IAM-protected route, so no IAM-gated API can be exercised or relied on.
Fix: the authorization step of the execute-api data-plane handler (SigV4 verification for AuthorizationType=AWS_IAM) in the apigatewayv2 crate.
Traces: apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served.

Cause 2 of 9: a request to the bare host with no stage prefix is refused 403 instead of 404.
AWS: recorded behavior only — a request whose path does not begin with an existing stage segment matches no stage and is refused with HTTP 404 and the gateway's Not Found envelope {"message":"Not Found"} (fixed text).
Emulator: answers HTTP 403 with {"message":"Forbidden"}.
Why: a client distinguishing "this path/stage does not exist" (404) from "you are not allowed" (403) draws the wrong conclusion, and SDK/edge retry and error-mapping logic keyed on 404 misbehaves.
Fix: route/stage resolution in the execute-api front-door handler — the no-stage-match branch must produce the 404 Not Found envelope.
Traces: apigatewayv2.http-api-path-without-stage-prefix-not-delivered.

Cause 3 of 9: the Lambda payload format 1.0 event omits multiValueHeaders.
AWS: recorded behavior only (payload format version is modeled as CreateIntegration.PayloadFormatVersion, StringWithLengthBetween1And64). In format 1.0 the event carries a multiValueHeaders member listing every header's values under the ORIGINAL request casing (1.0 does not lowercase header names), while format 2.0 carries lowercased headers and no multiValueHeaders — that difference is the whole point of the two formats.
Emulator: the 1.0 event has no multiValueHeaders at all (the member read back None), so a header sent as 'X-Sim2Real-Probe: <value>' can be found under no casing.
Why: a handler written for payload format 1.0 (the only format that exposes repeated headers) sees no headers at all and either crashes or drops request data.
Fix: the payload-format-1.0 event builder in the apigatewayv2 Lambda-integration (AWS_PROXY) request mapper.
Traces: apigatewayv2.lambda-payload-format-1-0-and-2-0-envelopes-differ.

Cause 4 of 9: integration ResponseParameters 'overwrite:statuscode' does not rewrite the client-visible status.
AWS: Smithy CreateIntegration.ResponseParameters -> ResponseParameters (map of status code to IntegrationParameters), documented "Supported only for HTTP APIs. You use response parameters to transform the HTTP response from a backend integration before returning the response to clients." With ResponseParameters {'200': {'overwrite:statuscode': '201'}} the integration answering 200 is returned to the client as 201.
Emulator: the client still receives 200 (the twin route proves the integration itself answered 200), so the mapping is stored but never applied.
Why: response parameter mapping is silently inert; status-based client logic and any API contract built on a rewritten status is wrong.
Fix: response parameter mapping in the execute-api data-plane response path (status overwrite, then header overwrite/append/remove).
Traces: apigatewayv2.parameter-mapping-rewrites-a-real-request-and-response.

Cause 5 of 9: a request through an integration whose TimeoutInMillis is shorter than the backend's latency is never dispatched to the backend.
AWS: Smithy CreateIntegration.TimeoutInMillis (IntegerWithLengthBetween50And30000, range 50-30000). AWS dispatches the request to the integration target and refuses the CLIENT when the target exceeds the timeout — the target has still been invoked, which its own log shows.
Emulator: the backend's log never shows the timed-out request (polled 120 s, last result []), so the gateway decides the timeout without ever calling the target.
Why: side effects a client expects from a request that timed out at the edge (a write the backend already performed) never happen in the emulator, so timeout-retry and idempotency behavior cannot be reproduced.
Fix: the integration dispatch/timeout path in the execute-api data-plane handler — arm the timeout around a real dispatch instead of short-circuiting it.
Traces: apigatewayv2.deep-apigatewayv2-2-enforce-integration-timeout-503-while-twin-route-serves.

Cause 6 of 9: a regional custom domain's target host is published as a real-AWS execute-api address the emulator does not serve.
AWS: DomainNameConfigurations.ApiGatewayDomainName is the address a mapped custom domain is reached on; in an emulator the only reachable address is one of the emulator's own hosts/ports.
Emulator: publishes 'd-<id>.execute-api.<region>.amazonaws.com'; replay refuses it with simparity.published.PublishedUrlRejected "its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". No request can be sent through the custom domain at all.
Why: the only address a client is given for a custom domain points outside the emulator, so custom-domain + API-mapping invoke is unreachable.
Fix: the ApiGatewayDomainName construction in CreateDomainName/GetDomainName(s) plus the front-door listener that must serve the published address. (Related open issues 20261026T023803Z-apigatewayv2-custom-domain-and-api-mapping-control-plane.md and 20261027T053233Z-apigatewayv2-9-net-new-root-causes-api-mapping.md cover control-plane reads of domains/mappings and do not name the published host.)
Traces: apigatewayv2.e2e-custom-domain-mapping-invoke.

Cause 7 of 9: an HTTP API invoke through a REQUEST-authorizer route takes seconds, so authorizer-cache TTL behavior is unobservable.
AWS: recorded behavior only — an invoke through a cached REQUEST authorizer answers in tens to low hundreds of milliseconds, so several samples fit inside a short ResultTtlInSeconds window.
Emulator: measured per-request latency of 4.18 s; only 1 sample fitted inside the TTL window (guard 8.4 s derived from that latency), so the cache-hit/expiry contrast cannot be sampled at all.
Why: any behavior scoped to a short TTL or rate window (authorizer caching, throttle refill) cannot be reproduced, and interactive clients see an API that is orders of magnitude slower than AWS.
Fix: the per-request cost of the execute-api data-plane path (authorizer invocation and integration dispatch) in the apigatewayv2 data-plane handler.
Traces: apigatewayv2.deep-apigatewayv2-3-stagesem-request-authorizer-cache-expires-after-ttl.

Cause 8 of 9: WebSocket API data-plane scenarios never reach a verdict — the check is killed at the 60 s cap.
AWS: recorded behavior only — a client-initiated close frame invokes the $disconnect route, and a two-way route delivers its integration response to the client only when a RouteResponse exists; both complete in seconds.
Emulator: both checks hang and are terminated with "Check timed out after 60.0s" — no close/disconnect propagation and no two-way reply is ever observed.
Why: WebSocket session lifecycle (disconnect side effects, reply delivery) cannot be exercised at all; a client never learns a connection closed.
Fix: the WebSocket side of the execute-api data plane — handshake/frame loop, $disconnect dispatch and RouteResponse delivery.
Traces: apigatewayv2.client-initiated-close-invokes-disconnect-route, apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client.

Cause 9 of 9: five HTTP API data-plane enforcement/stage-semantics scenarios never reach a verdict — each check is killed at the 60 s cap.
AWS: recorded behavior only — the 10 MiB payload gate refuses one byte over the limit with 413 and does not deliver it, a manual (non-auto-deploy) stage keeps serving its deployment snapshot until redeployed, a stage-variable integration target without an invoke permission is refused, a CORS preflight caught by an ANY route is authorized, and a route-level throttle answers 429 while its twin route keeps serving. All settle within seconds on AWS.
Emulator: all five checks hang to "Check timed out after 60.0s", so none of these contracts is ever answered.
Why: the data plane does not converge for these shapes, so payload limits, stage/deployment semantics, preflight and per-route throttling are all unreproducible.
Fix: the execute-api HTTP data-plane handler (payload-size gate, deployment-snapshot resolution per stage, stage-variable target resolution, CORS preflight short-circuit, per-route throttle) — and whatever makes these requests hang rather than answer.
Traces: apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered, apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot, apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected, apigatewayv2.http-api-cors-preflight-caught-by-any-route-is-authorized, apigatewayv2.route-throttling-returns-429-while-twin-route-serves.

## Evidence
Cause 1: expected 403 {"message":"Forbidden"}; actual "a request whose SigV4 signature does not verify must be refused with 403, got 200: 'sim2real-behav-iam-<id>'".
Cause 2: expected 404 {"message":"Not Found"}; actual "bare host (no stage prefix): expected 404, got 403: '{\"message\":\"Forbidden\"}'".
Cause 3: expected the 1.0 event's multiValueHeaders['X-Sim2Real-Probe'] == [<sent value>]; actual None (member absent).
Cause 4: expected ResponseParameters['200']['overwrite:statuscode']='201' to make the client-visible status 201; actual 200.
Cause 5: expected the backend's log to show it was invoked for the timed-out request; actual "not met within 120.0s. Last result: []".
Cause 6: expected the published custom-domain host to be an emulator address; actual simparity.published.PublishedUrlRejected for 'https://d-<id>.execute-api.us-west-2.amazonaws.com'.
Cause 7: expected several invokes inside the authorizer TTL window; actual "only 1 inside-TTL samples fitted in the window (guard 8.4s from measured latency 4.18s)".
Causes 8 and 9: every listed check ends with "Error: Check timed out after 60.0s" with no response observed.

## Steps to Reproduce
1. CreateApi(ProtocolType='HTTP'), CreateIntegration/CreateRoute/CreateStage, CreateDeployment.
2. Cause 1: set the route's AuthorizationType='AWS_IAM' and send a request with a corrupted SigV4 signature — it is served 200 instead of 403.
3. Cause 2: GET the stage host with no stage prefix — 403 instead of 404.
4. Cause 3: create an AWS_PROXY integration with PayloadFormatVersion='1.0' and echo the event — multiValueHeaders is missing.
5. Cause 4: set ResponseParameters={'200': {'overwrite:statuscode': '201'}} and invoke — the client sees 200.
6. Cause 5: set TimeoutInMillis below the backend's latency and invoke — the backend log never records the request.
7. Cause 6: ImportCertificate + CreateDomainName + CreateApiMapping, then read DomainNameConfigurations.ApiGatewayDomainName — it is an amazonaws.com host.
8. Causes 7-9: replay the remaining check ids in trace_ids; each hangs to the 60 s cap or cannot sample inside its window.
