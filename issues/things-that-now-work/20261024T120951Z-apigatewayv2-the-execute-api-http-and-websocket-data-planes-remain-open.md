Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.create-stage-and-deployment apigatewayv2.invoke-http-api-httpproxy apigatewayv2.invoke-http-api-lambda-proxy apigatewayv2.delete-stage-stops-serving-requests apigatewayv2.update-route-target-switch-takes-effect-on-autodeploy apigatewayv2.http-api-cors-configuration-overrides-backend-cors-headers apigatewayv2.http-api-cors-preflight-caught-by-any-route-is-authorized apigatewayv2.http-api-cors-preflight-origin-allowlist-and-delete apigatewayv2.http-api-named-stage-rawpath-includes-stage-prefix apigatewayv2.http-api-v2-binary-body-base64-encoded-both-ways apigatewayv2.parameter-mapping-rewrites-a-real-request-and-response apigatewayv2.quick-create-lambda-target-invoked-directly-delivers-v2-event apigatewayv2.lambda-authorizer-array-context-value-passes-through-unchanged apigatewayv2.lambda-authorizer-payload-format-1-0-iam-policy-response apigatewayv2.lambda-authorizer-simple-response-explicit-deny-forbidden apigatewayv2.lambda-payload-format-2-0-response-inference-and-cookies apigatewayv2.simple-response-authorizer-under-payload-format-1-0-fails-500 apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value apigatewayv2.request-authorizer-missing-one-of-two-identity-sources-unauthorized apigatewayv2.reset-authorizers-cache-affects-invoke apigatewayv2.reset-authorizers-cache-server-side apigatewayv2.route-throttling-returns-429-while-twin-route-serves apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served apigatewayv2.deep-apigatewayv2-2-enforce-integration-timeout-503-while-twin-route-serves apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot apigatewayv2.deep-apigatewayv2-3-stagesem-request-authorizer-cache-expires-after-ttl apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-selects-lambda-backend apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected apigatewayv2.client-initiated-close-invokes-disconnect-route apigatewayv2.websocket-inbound-binary-frame-rejected apigatewayv2.websocket-inbound-message-32kb-boundary apigatewayv2.websocket-broadcast-to-multiple-connections apigatewayv2.websocket-connect-request-authorizer-token-gate apigatewayv2.websocket-route-selection-by-action
Issue-Key: apigatewayv2.create-stage-and-deployment|_poll_http#3|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.invoke-http-api-httpproxy|_get_until_200#4|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.invoke-http-api-lambda-proxy|run#14|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.delete-stage-stops-serving-requests|_await#11|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.update-route-target-switch-takes-effect-on-autodeploy|_poll_version#11|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.http-api-cors-configuration-overrides-backend-cors-headers|_poll_http#10|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.http-api-cors-preflight-caught-by-any-route-is-authorized|_poll_http#10|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.http-api-cors-preflight-origin-allowlist-and-delete|_poll_http#13|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.http-api-named-stage-rawpath-includes-stage-prefix|_poll_http#8|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.http-api-v2-binary-body-base64-encoded-both-ways|_poll_http#9|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.parameter-mapping-rewrites-a-real-request-and-response|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.quick-create-lambda-target-invoked-directly-delivers-v2-event|_poll_http#17|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.lambda-authorizer-array-context-value-passes-through-unchanged|_poll_http#15|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.lambda-authorizer-payload-format-1-0-iam-policy-response|_poll_http#4|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.lambda-authorizer-simple-response-explicit-deny-forbidden|_poll_http#2|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.lambda-payload-format-2-0-response-inference-and-cookies|_poll_http#12|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.simple-response-authorizer-under-payload-format-1-0-fails-500|_poll_http#15|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value|_poll_http#21|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.request-authorizer-missing-one-of-two-identity-sources-unauthorized|_poll_http#19|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.reset-authorizers-cache-affects-invoke|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.reset-authorizers-cache-server-side|_poll_invoke_until_ok#46|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.route-throttling-returns-429-while-twin-route-serves|_poll_http#15|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served|_wait_routes_deployed#17|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-integration-timeout-503-while-twin-route-serves|_wait_routes_deployed#17|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered|_poll_http#18|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot|_wait_stable#24|Timeout|time.sleep(0.5)
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-request-authorizer-cache-expires-after-ttl|_wait_stable#28|Timeout|time.sleep(0.5)
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-selects-lambda-backend|_poll_http#27|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected|_poll_http#25|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.client-initiated-close-invokes-disconnect-route|_do_ws_handshake#3|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewayv2.websocket-inbound-binary-frame-rejected|_open_ws_connection#2|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewayv2.websocket-inbound-message-32kb-boundary|_open_ws_connection#2|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewayv2.websocket-broadcast-to-multiple-connections|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.websocket-connect-request-authorizer-token-gate|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.websocket-route-selection-by-action|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# apigatewayv2: the two execute-api data planes still cannot complete a request — an HTTP route's integration never answers and the RFC 6455 upgrade has no seam to be served from (2 causes)

> _RESOLVED for CAUSE 1, which this report was filed for: an HTTP api's route now has its
> INTEGRATION performed. A Lambda proxy integration (`AWS_PROXY` naming no
> `IntegrationSubtype`) invokes the function it names through Lambda's own modelled `Invoke`
> over the in-process transport `OperationContext::callback` hands a handler, with the event
> its `PayloadFormatVersion` defines, and answers with the response that version defines — so
> a client receives the function's own status, headers and body. The `REQUEST` authorizer's
> own decision is obtained the same way, cached for the TTL it declares and keyed by its
> identity sources, which is what `ResetAuthorizersCache` now empties. An integration that
> overruns its own `TimeoutInMillis` is `503`, a route the serving stage's `RouteSettings`
> throttle is `429`, and everything else that produced no response stays the gateway's `500`.
> See `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/invoke/` and
> `tests/all/http_api_backend_dispatch.rs`._
>
> _An `HTTP_PROXY` route still answers `307` to the upstream the integration resolved, and
> deliberately so: the project's own forwarding rule is that a destination a request or a
> configuration chooses must not become a request this process makes, and that the caller's
> credential must never travel to one. The upstream is a URI the caller can reach, so it is
> told where the backend is. `apigatewayv2.invoke-http-api-httpproxy` and
> `apigatewayv2.parameter-mapping-rewrites-a-real-request-and-response` therefore stay
> unserved._
>
> _CAUSE 2 (the RFC 6455 upgrade) is NOT fixed and is not fixable from inside this emulator:
> `vendor/server/src/listener_guard.rs:126` + `vendor/server/src/listeners.rs:588` refuse every
> WebSocket upgrade with `501` before a service is reached,
> `vendor/smithy-server-runtime/src/service.rs:390` offers no seam that hands a service the
> connection, and `vendor/smithy-server-runtime/src/multi_service_router.rs:590-593` strips a
> `101`'s own hop-by-hop fields. It stays OPEN under the two reports that own it —
> `20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md` and
> `20261023T124140Z-apigatewayv2-websocket-duplex-data-plane-is-not-served.md` — and one core
> seam closes all three. The six `client-initiated-close-invokes-disconnect-route`,
> `websocket-*` replay keys below belong to that cause and to those reports._
>
> _Two further behaviours those checks also need are NOT in this fix and have no report of
> their own yet: the Lambda resource-policy gate that refuses an invoke of a function API
> Gateway may not call (`...stage-variable-target-without-invoke-permission-rejected`), and
> the three CORS checks, whose requests carry an `Origin` header: the router refuses a request
> it names an operation for when the browser-set headers say a browser sent it from another
> site (`vendor/smithy-server-runtime/src/multi_service_router.rs:362`), and hands an
> `OperationContext` to no request it does not name one for, so such an invoke is served from
> the seam that carries no transport and its integration is reported as the class it falls
> into rather than performed. Closing those three needs a core seam that lets a named
> data-plane operation supply (or opt out of) that browser verdict._

> _Split out of `issues/things-that-now-work/20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md`, whose causes 3 to 8 are fixed or already served (`CreateVpcLink` now resolves its `SubnetIds` against EC2 and requires one VPC, `ResetAuthorizersCache` is implemented, `CreateDomainName` refuses an empty `DomainNameConfigurations`, `GetRoutes` refuses a `NextToken` it never issued, a stage variable inside the documented character set is accepted, and a completed stack's `Outputs` carry an `OutputValue`). This report carries the replay keys of that report's causes 1 and 2, which do NOT pass, re-filed here so they are not recorded as working. Everything below is what that fix established about them; a later fixer starts from here rather than from scratch._
>
> _One further check of the parent, `apigatewayv2.http-proxy-stage-variable-selects-backend-host`, now gets past its own `CreateStage` assertion (the parent's cause 6, pinned by `services/service-com-amazonaws-apigatewayv2-apigatewayv2/tests/all/stage_variable_values.rs`) and cannot complete for cause 1 below; its replay key (`apigatewayv2|CreateStage|BadRequestException`) stays with the parent._

## Operation
Invoke (execute-api data plane, HTTP and WEBSOCKET)

## Description
Two causes, and each is a CAPABILITY this emulator does not have rather than a wrong answer.

=== Cause 1 of 2: an HTTP api's route is reached, authorized and matched, but its integration never answers ===

AWS: recorded behavior only (data plane; not modelled in Smithy). A GET of a deployed stage's own invoke URL for a matching route key is answered HTTP 200 with the integration's response, and with the documented statuses (403/404/413/429/503) for the negative legs these checks exercise.

Emulator: everything UP TO the integration is implemented and has its own suites — the claim on the invoke endpoint, the three addressings a local client can use, stage resolution, the deployment gate, route-key matching with precedence, the `AuthorizationType` gate (`AWS_IAM`, `JWT`, the identity-source precondition of a `REQUEST` authorizer), the `CorsConfiguration` decoration and the 10 MiB payload refusal (`services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/invoke.rs`, with `tests/all/http_api_servable_stage.rs`, `http_api_route_precedence.rs`, `http_api_cors_data_plane.rs`, `http_api_jwt_data_plane.rs`, `http_api_payload_quota.rs`, `http_api_signed_invoke.rs`, `http_api_request_authorizer.rs`). The step that cannot be served is the last one:

* an `AWS_PROXY` route needs the Lambda function it names to RUN, and `Invoke` answers 501 for `RequestResponse` because no deployment package executes in this emulator — owned by `20261025T030504Z-apigatewayv2-lambda-an-aws-proxy-route-cannot-answer-200-until-a.md` and `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`. A `REQUEST` (Lambda) authorizer's decision, its per-identity cache and that cache's TTL are the same gap, which is why `reset-authorizers-cache-affects-invoke`, `reset-authorizers-cache-server-side` and `...request-authorizer-cache-expires-after-ttl` cannot pass even though `ResetAuthorizersCache` itself is now implemented;
* an `HTTP_PROXY` route addresses an endpoint over the network, which this emulator does not open. In these checks that endpoint is an api THIS emulator serves, so it could be reached in process — but the only serving seam is `vendor/smithy-server-runtime/src/service.rs:390`, `override_routing_error(&self, &RtRequest) -> Option<RtResponse>`, which is synchronous, buffered and handed no callback, so no hop to another service can be awaited from where an invoke is answered. Today the caller is sent to the stored `IntegrationUri` with a `307`, which preserves the method and body and invents no response.

Required core change: a serving seam that is async and carries the in-process callback (so a claimed data-plane request can ask another emulated service for its answer), plus the Lambda runtime the two reports above own.

=== Cause 2 of 2: a deployed WEBSOCKET api cannot complete the RFC 6455 upgrade ===

AWS: recorded behavior only. The handshake on the stage's `wss://` URL is answered `HTTP/1.1 101 Switching Protocols` with the derived `Sec-WebSocket-Accept`, after which frames dispatch by `RouteSelectionExpression` and `$disconnect` fires on close.

Emulator: the apigatewayv2 crate DOES carry this data plane (`src/service/websocket.rs`, claimed from `Service::can_handle` and served from `Service::override_routing_error`, with `tests/all/websocket_handshake_data_plane.rs` and `tests/all/websocket_connect_authorization.rs`), and in process those suites pass — but nothing in the vendored path can hand a service the connection's byte stream:

* `vendor/smithy-server-runtime/src/service.rs:390` — every `Service` seam takes an already-buffered request and returns a buffered response; there is no upgrade seam;
* `vendor/smithy-server-runtime/src/multi_service_router.rs:590-593` — `connection` and `upgrade` are stripped from every response, so even a hand-built `101` reaches the client without the fields RFC 6455 §4.1 requires of the server's handshake.

Required core change: a seam that lets the claiming service answer `101` and then receive the upgraded connection, and a response path that does not strip a `101`'s hop-by-hop fields. Answering `101` without it would be a fabricated success: the client would see the upgrade and then read EOF, with no socket to carry `$connect`.

This is the same gap already reported, with the same citations, by `20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md` (which names the parent of this report among its related reports) and by `20261023T124140Z-apigatewayv2-websocket-duplex-data-plane-is-not-served.md`; one core seam closes all of them. It is re-filed here only so these six replay keys stay attributed to a cause.

## Evidence
From the parent report, unchanged by the fix that split it:

Cause 1: expected HTTP 200 from the stage's own invoke URL within the check's 90 s poll; actual — never the expected status, so each check runs past the suite's 60 s cap (`Error: Check timed out after 60.0s`). All 29 carry the suite's `DATAPLANE_UNIMPLEMENTED` marker.

Cause 2: expected HTTP 101 Switching Protocols on the WEBSOCKET stage URL; actual `ConnectionError: websocket handshake failed: b'HTTP/1.1 404 Not Found'` with `x-amzn-errortype: UnknownService` (the host/port and date in that line are per-environment; the status and the error type are the requirement).

## Steps to Reproduce
1. Boot the emulator from this checkout and point an SDK/CLI client at it.
2. Cause 1: CreateApi(ProtocolType=HTTP) -> CreateIntegration(AWS_PROXY or HTTP_PROXY) -> CreateRoute -> CreateStage(StageName='$default', AutoDeploy=true), then GET the invoke URL announced for that api id and stage. The request is now CLAIMED and resolved — the answer is the gateway's `500` for an integration that produced no response, or a `307` to the stored `IntegrationUri` — never the recorded 200.
3. Cause 2: CreateApi(ProtocolType=WEBSOCKET) with $connect/$disconnect/$default routes, CreateDeployment, CreateStage, then open an RFC 6455 upgrade to the stage URL; it is answered 404 with `x-amzn-errortype: UnknownService`.
4. Replay the check ids in Issue-Trace-Ids to confirm each cause.
