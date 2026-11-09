Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__iam__lambda.http-api-head-and-undeclared-method-dispatch-and-forwarded-headers apigatewayv2__iam__lambda.http-api-request-target-decoded-twice-and-normalised-before-route-selection
Issue-Key: apigatewayv2__iam__lambda.http-api-head-and-undeclared-method-dispatch-and-forwarded-headers|_assert_gateway_404#14|AssertionError|assert _hdr(resp, "apigw-requestid"), (
Issue-Key: apigatewayv2__iam__lambda.http-api-request-target-decoded-twice-and-normalised-before-route-selection|_leg_decode_depth#17|AssertionError|assert doc.get("rawPath") == want_path, (
Issue-Commit: a0fae0073c5a57a2ef84b70eabff7d12afa58f91

# apigatewayv2 HTTP API execute-api data plane: 2 net-new causes — gateway-produced responses omit the apigw-requestid header, and the request target is never percent-decoded before route selection

> _Note: this issue was identified against the code base at commit `a0fae0073c5a57a2ef84b70eabff7d12afa58f91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api HTTP API data plane)

## Description
2 net-new root causes, both in the HTTP API (apigatewayv2) execute-api data plane. Neither is covered by an open issue: the nearest ones (20261107T125951Z-apigatewayv2-execute-api-data-plane-7-net-new..., about rawPath carrying the $default stage segment, and 20261109T154749Z-apigatewayv2-5-net-new-root-causes-the-published..., about the published ApiEndpoint host) describe different defects, and in this run the stage DID serve: the warm-up GET on a routed literal was answered 200 by the integration twice in a row before any assertion below was believed.

Cause 1 of 2: a response the gateway produces itself carries no `apigw-requestid` response header.
AWS: recorded behavior only — the execute-api data plane is not modelled in the apigatewayv2 Smithy model (the model binds only control-plane operations such as CreateApi/CreateRoute/CreateStage), so the contract here is the recording's. Every HTTP API response carries the gateway's own request-id header `apigw-requestid`, and its value is the same id the proxy payload reports in `requestContext.requestId`. The gateway's own Not Found envelope is `404` with `content-type: application/json`, `content-length: 23` and body `{"message":"Not Found"}` — body bytes suppressed to zero on the wire for a HEAD while content-length still advertises 23 — and no integration response header. Requirement parts: the fixed body text `{"message":"Not Found"}`, the fixed header NAME `apigw-requestid`, and the rule that its value equals `requestContext.requestId`; the id VALUE itself is minted per request and is not part of the requirement.
Emulator: `HEAD /items` on an API whose only matching key is the method-scoped `GET /items` (no HEAD, ANY or `$default` sibling) is correctly refused 404 with the 23-byte JSON envelope, but the response headers are only `content-type: application/json`, `content-length: 23`, `connection: close`, `date: <clock>` — `apigw-requestid` is absent. The check stops at this first leg, so the paired legs that assert the same header on the PATCH/OPTIONS 404s and assert `apigw-requestid == requestContext.requestId` on a served `GET /ctx` never ran; the header is absent on the one gateway-produced response that was observed.
Why: a client cannot correlate a gateway refusal with anything. The id AWS returns on EVERY response — the one quoted in access logs and in support cases, and the one SDK/middleware and tracing code read off the response — is simply missing, so correlation code reads None and a caller cannot tell which of several in-flight requests the 404 belongs to.
Fix: the execute-api front door's HTTP-API response path — the component that builds the gateway's own envelopes (404 Not Found, 400 Bad Request) and relays integration responses must stamp `apigw-requestid` with the same request id it publishes in the proxy payload's `requestContext.requestId`.
Traces: apigatewayv2__iam__lambda.http-api-head-and-undeclared-method-dispatch-and-forwarded-headers.

Cause 2 of 2: the request target is not percent-decoded before route selection, so `rawPath` and `pathParameters` carry the raw encoded bytes.
AWS: recorded behavior only (data plane, unmodelled as above). An HTTP API decodes the request target TWICE before route selection and no more: `/items/a%41b` selects `GET /items/{id}` with `rawPath` `/items/aAb` and `pathParameters` `{"id":"aAb"}`; `/items/a%2541b` decodes to the same `/items/aAb` / `aAb` on the second pass; `/items/a%252541b` stops at `rawPath` `/items/a%41b` / `{"id":"a%41b"}`, which is what fixes the depth at exactly two. The second pass is real and refusing: a target that decodes once to a malformed escape (`/items/a%25b` → `a%b`) is answered `400` with the gateway envelope `{"message":"Bad Request"}`, while `/items/a%2525b` is served.
Emulator: `GET /items/a%41b` does select `GET /items/{id}`, but the payload reports `rawPath` `/items/a%41b` — the untouched request target, i.e. not one decoding pass was applied. The legs that follow (second-pass depth, the malformed-escape 400, %2F not separating segments, trailing-slash significance, dot-segment normalisation, query mirroring) never ran, so the whole decode/normalise contract is unverified behind this one defect.
Why: a handler that reads `rawPath`/`pathParameters` receives percent-encoded bytes that AWS had already decoded, so path parameters captured from encoded segments are wrong (`a%41b` instead of `aAb`), routing decisions that depend on the decoded form diverge, and a request AWS refuses 400 for a malformed escape is served instead.
Fix: the execute-api route-selection path for HTTP APIs — decode the request target (two passes, rejecting a malformed escape on either pass with the gateway's own 400 envelope) before matching route keys, and build `rawPath`, `requestContext.http.path` and `pathParameters` from the decoded target.
Traces: apigatewayv2__iam__lambda.http-api-request-target-decoded-twice-and-normalised-before-route-selection.

## Evidence
Cause 1 — check apigatewayv2__iam__lambda.http-api-head-and-undeclared-method-dispatch-and-forwarded-headers:
Expected (AWS): the gateway-produced 404 for HEAD /items carries a non-empty `apigw-requestid` header (plus content-type application/json, content-length 23, zero body bytes, no integration `x-probe` header).
Actual (emulator): `AssertionError: HEAD /items: 'GET /items' is method-scoped, so HEAD must not be served by it: a gateway-produced response must carry the gateway's own request id header, got headers [('content-type', 'application/json'), ('content-length', '23'), ('connection', 'close'), ('date', 'Mon, 09 Nov 2026 22:37:25 GMT')]`
(the status, content-type and content-length were already correct at this point; only the request-id header is missing. The `date` value is per-run and not part of the requirement.)

Cause 2 — check apigatewayv2__iam__lambda.http-api-request-target-decoded-twice-and-normalised-before-route-selection:
Expected (AWS): `GET /items/a%41b` → routeKey `GET /items/{id}`, rawPath `/items/aAb`, pathParameters `{"id": "aAb"}`.
Actual (emulator): `AssertionError: GET /items/a%41b: one encoded layer is decoded, so rawPath must be '/items/aAb', got '/items/a%41b'`

## Steps to Reproduce
Cause 1:
1. CreateApi (HTTP, ProtocolType=HTTP) with routes exactly `GET /items`, `GET /ctx`, `ANY /any/{proxy+}` and a second backend on `GET /hd`, `HEAD /hd`, `ANY /anyhd`; create the `$default` stage with AutoDeploy=true. `GET /items` deliberately has no HEAD, ANY or `$default` sibling.
2. Confirm the stage serves: `GET /items` answers 200 from the integration.
3. Send `HEAD /items` to the stage's invoke address.
4. Observe 404 with the 23-byte `{"message":"Not Found"}` envelope but NO `apigw-requestid` response header. The same header is also expected (and missing) on the PATCH/OPTIONS 404s on `/items`, and on a served `GET /ctx` it must equal the payload's `requestContext.requestId`.

Cause 2:
1. CreateApi (HTTP) with routes exactly `GET /items`, `GET /items/{id}`, `GET /pets/{proxy+}`, `GET /a/b`, `GET /b`, `GET /q` — no `$default` and no root greedy route; create the `$default` stage with AutoDeploy=true, backed by a Lambda proxy (payload 2.0) that echoes `routeKey`, `rawPath`, `pathParameters`, `rawQueryString`.
2. `GET /items/a%41b` → expect rawPath `/items/aAb`, pathParameters `{"id":"aAb"}`; emulator reports rawPath `/items/a%41b`.
3. `GET /items/a%2541b` → expect the same decoded `/items/aAb`; `GET /items/a%252541b` → expect rawPath `/items/a%41b`.
4. `GET /items/a%25b` → expect 400 `{"message":"Bad Request"}` from the second decode pass; `GET /items/a%2525b` → expect 200.
