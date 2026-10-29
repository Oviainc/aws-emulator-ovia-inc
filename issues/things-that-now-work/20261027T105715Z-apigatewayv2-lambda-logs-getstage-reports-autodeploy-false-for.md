Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.prod-stage-round-trips-its-deployment-and-default-route-settings apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url
Issue-Key: apigatewayv2__lambda__logs.prod-stage-round-trips-its-deployment-and-default-route-settings|run#6|AssertionError|assert "AutoDeploy" not in stage and stage.get("RouteSettings") == {}, (
Issue-Key: apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url|_ws_connect#4|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Commit: 5df60a3e41d7e0b7654dd2b65ebe4fbcad6c7940

# apigatewayv2__lambda__logs: GetStage reports AutoDeploy=false for a stage that never set it, and the WEBSOCKET execute-api upgrade answers 404 UnknownService (2 causes)

> _Note: this issue was identified against the code base at commit `5df60a3e41d7e0b7654dd2b65ebe4fbcad6c7940`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStage

## Description
Two net-new root causes, both grounded in failing behavioral checks of the apigatewayv2__lambda__logs group. Both must be closed before submitting.

=== Cause 1 of 2: GetStage materialises an unset optional AutoDeploy as false ===
Failing check: apigatewayv2__lambda__logs.prod-stage-round-trips-its-deployment-and-default-route-settings

AWS: Smithy (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json) models GetStage output member `AutoDeploy` as target `__boolean`, jsonName `autoDeploy`, NOT required and carrying NO `@default` trait. A boolean member with no default is absent from the wire until it has a value, so for a stage created without AutoDeploy the GET /v2/apis/{ApiId}/stages/{StageName} body contains no `autoDeploy` key at all; AWS reports the member only once CreateStage/UpdateStage has actually set it. The same stage's undeclared `DefaultRouteSettings` does read back as the documented OFF defaults (DataTraceEnabled=false, DetailedMetricsEnabled=false, LoggingLevel=OFF) and `RouteSettings` reads back as the empty map — the emulator already matches on both of those, so the divergence is isolated to AutoDeploy. Recorded behavior confirms: the real response for a `prod` stage created without AutoDeploy has no autoDeploy member.

Emulator: GetStage serialises `"autoDeploy": false`. The check's presence assertion `"AutoDeploy" not in stage` fails with `AssertionError: AutoDeploy=False RouteSettings={}` — i.e. the member is present with value false where AWS omits it.

Why: presence of this member is the only signal distinguishing "auto-deploy was never configured on this stage" from "auto-deploy was configured and turned off". A client that branches on membership (CDK/CloudFormation drift detection, config round-trip and import tooling, `aws apigatewayv2 get-stage` diffing) concludes the stage carries an explicit AutoDeploy setting the user never made, and a round-trip that echoes the read value back into UpdateStage writes a setting that did not exist.

Fix: the apigatewayv2 GetStage handler's output assembly and the stage record's `to_wire` conversion in this service's state — the stored auto_deploy must stay `None` until CreateStage/UpdateStage supplies a value, instead of being defaulted to `Some(false)` at creation or at serialisation. GetStages must read back the same way, since the check also lists stages.

=== Cause 2 of 2: the WEBSOCKET execute-api data plane answers 404 UnknownService instead of completing the RFC 6455 upgrade ===
Failing check: apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url

AWS: recorded behavior only — the execute-api data plane is not described by any Smithy shape. A deployed WEBSOCKET API's stage endpoint (wss://{api-id}.execute-api.{region}.amazonaws.com/{stage}) answers a GET carrying `Upgrade: websocket`, `Connection: Upgrade`, `Sec-WebSocket-Key` and `Sec-WebSocket-Version: 13` with `HTTP/1.1 101 Switching Protocols` plus the derived `Sec-WebSocket-Accept` header, after which the $connect/$default/$disconnect routes dispatch to their integrations and the handler can push its reply back through the stage's https:// callback URL.

Emulator: the upgrade is refused with `HTTP/1.1 404 Not Found`, `content-type: application/x-amz-json-1.1`, `x-amzn-errortype: UnknownService` (fixed text: the status line, the content-type and the `UnknownService` error type; the body length and date are per-run values). The listener answers as if no service owned the execute-api host at all, so the handshake never happens.

Why: no WebSocket client can connect to a deployed API. Route dispatch, the registration reply delivered through the https:// callback URL, and the connections-table row the handler writes are all unreachable — every WebSocket-shaped application is non-functional against the emulator.

Fix: the execute-api data-plane listener and its host/stage routing for WEBSOCKET protocol APIs, not the apigatewayv2 control-plane handlers (CreateApi/CreateStage/CreateDeployment all succeed in this check).

Related already-open issues describe the same symptom for other checks but do NOT carry this check's replay key, and the suite did not mark this check owned: 20261024T120951Z-apigatewayv2-dynamodb-lambda-logs-s3-the-websocket-upgrade.md and 20261025T030504Z-apigatewayv2-the-websocket-execute-api-data-plane-still.md. Fixing the listener should close all of them together.

## Evidence
Cause 1 — expected (AWS): GetStage body for a stage created without AutoDeploy has NO `autoDeploy` member; `defaultRouteSettings` = {DataTraceEnabled:false, DetailedMetricsEnabled:false, LoggingLevel:"OFF"}; `routeSettings` = {}.
Cause 1 — actual (emulator): `autoDeploy` present with value false. Replay output:
  File "check_apigatewayv2__lambda__logs_prod_stage_round_trips_its_deployment_and_default_route_settings.py", line 33, in run
    assert "AutoDeploy" not in stage and stage.get("RouteSettings") == {}, (
  AssertionError: AutoDeploy=False RouteSettings={}
Smithy citation: GetStage output member AutoDeploy -> __boolean, jsonName autoDeploy, not required, no @default (model com-amazonaws-apigatewayv2-apigatewayv2.json). Emulator code path: services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/get_stage.rs:187 (`auto_deploy: stored.auto_deploy`) fed by the stage record's `to_wire` in that crate's src/state.rs.

Cause 2 — expected (AWS): HTTP/1.1 101 Switching Protocols with Sec-WebSocket-Accept on the WEBSOCKET stage endpoint.
Cause 2 — actual (emulator): HTTP 404. Replay output:
  File "check_apigatewayv2__lambda__logs_registration_reply_is_delivered_via_the_https_callback_url.py", line 509, in _ws_connect
    assert status >= 500 and time.monotonic() < deadline, (
  AssertionError: the WebSocket upgrade to the prod stage answered 404: 'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService\r\ncontent-length: 149\r\ndate: ...'
The check had already verified the function's WEBSOCKET_ENDPOINT env var held the stage's https:// callback URL, so the failure is on the upgrade itself.

## Steps to Reproduce
Cause 1:
1. CreateApi (ProtocolType=WEBSOCKET, RouteSelectionExpression=$request.body.action), create the $connect/$default/$disconnect routes with AWS_PROXY integrations, CreateDeployment.
2. CreateStage with ApiId, StageName='prod', DeploymentId=<deployment> and NO AutoDeploy and NO DefaultRouteSettings/RouteSettings.
3. GetStage(ApiId, StageName='prod'). Observe the response contains `autoDeploy: false`; AWS returns no `autoDeploy` member. Re-replay check apigatewayv2__lambda__logs.prod-stage-round-trips-its-deployment-and-default-route-settings.

Cause 2:
1. With the same deployed WEBSOCKET API and 'prod' stage, open a TCP/TLS socket to {api-id}.execute-api.{region}.<emulator host> and send GET /prod HTTP/1.1 with Upgrade: websocket, Connection: Upgrade, Sec-WebSocket-Key: <base64 16 bytes>, Sec-WebSocket-Version: 13.
2. Observe HTTP/1.1 404 Not Found with x-amzn-errortype: UnknownService instead of 101 Switching Protocols. Re-replay check apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url.
