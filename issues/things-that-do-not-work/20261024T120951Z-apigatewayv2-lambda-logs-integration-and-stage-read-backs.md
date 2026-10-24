Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.lambda-proxy-integration-round-trips-uri-and-resolves-the-backend-function apigatewayv2__lambda__logs.prod-stage-round-trips-its-deployment-and-default-route-settings apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url
Issue-Key: apigatewayv2__lambda__logs.lambda-proxy-integration-round-trips-uri-and-resolves-the-backend-function|run#9|AssertionError|assert defaults == {"IntegrationType": "AWS_PROXY", "IntegrationMethod": "POST",
Issue-Key: apigatewayv2__lambda__logs.prod-stage-round-trips-its-deployment-and-default-route-settings|run#5|AssertionError|assert stage.get("DefaultRouteSettings") == {"DataTraceEnabled": False,
Issue-Key: apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url|_ws_connect#4|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# apigatewayv2__lambda__logs: integration and stage read-backs omit AWS's server-side defaults, and the WebSocket upgrade answers 404 (3 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIntegrations, GetStage, Invoke (execute-api WebSocket upgrade)

## Description
Three distinct root causes; a fixer must close all three.

=== Cause 1 of 3: an AWS_PROXY integration read back omits the defaults the service fills in ===
AWS: Smithy: CreateIntegration input IntegrationMethod, PayloadFormatVersion and PassthroughBehavior are all optional, and GetIntegration/GetIntegrations return them. Recorded behavior: an integration created on a WEBSOCKET API with only IntegrationType=AWS_PROXY and IntegrationUri reads back as IntegrationMethod='POST', PayloadFormatVersion='1.0', PassthroughBehavior='WHEN_NO_MATCH' (alongside ConnectionType='INTERNET' and TimeoutInMillis=29000, both of which the emulator already gets right).
Emulator: the same integration reads back with those three members null — `{'IntegrationType': 'AWS_PROXY', 'IntegrationMethod': None, 'PayloadFormatVersion': None, 'ConnectionType': 'INTERNET', 'PassthroughBehavior': None, 'TimeoutInMillis': 29000}`; the emulator stores only what the request carried and never applies the server-side defaults.
Why: drift detection and IaC round-tripping break — a client comparing a desired integration against the read-back sees three members it never set as absent, and any client that reads PayloadFormatVersion to decide how to shape an event gets nothing to key on.
Fix: the CreateIntegration handler's defaulting (and the state it persists), so GetIntegration/GetIntegrations project the service-applied defaults.
Traces: apigatewayv2__lambda__logs.lambda-proxy-integration-round-trips-uri-and-resolves-the-backend-function.

=== Cause 2 of 3: a stage created without DefaultRouteSettings reads back with the member absent ===
AWS: Smithy: CreateStage/GetStage carry DefaultRouteSettings (RouteSettings), optional on input. Recorded behavior: a stage created with no DefaultRouteSettings reads back with the member present and populated — DataTraceEnabled=false, DetailedMetricsEnabled=false, LoggingLevel='OFF'.
Emulator: GetStage returns no DefaultRouteSettings at all (read as None).
Why: a client that inspects DefaultRouteSettings to decide whether logging or detailed metrics are on must special-case a missing member that AWS never omits, and a drift detector reports a permanent difference on a stage nobody changed.
Fix: the CreateStage handler's defaulting of DefaultRouteSettings and the GetStage serializer that projects it.
Traces: apigatewayv2__lambda__logs.prod-stage-round-trips-its-deployment-and-default-route-settings.

=== Cause 3 of 3: the WebSocket upgrade to a deployed prod stage answers HTTP 404 UnknownService ===
AWS: recorded behavior only (data plane). A deployed WEBSOCKET stage completes the RFC 6455 upgrade with HTTP 101 Switching Protocols, after which the backend's reply is delivered over the socket (here, via the https callback URL built from the connection).
Emulator: the upgrade is answered `HTTP/1.1 404 Not Found` with `content-type: application/x-amz-json-1.1` and `x-amzn-errortype: UnknownService` — the JSON control-plane router's unrouted-request answer, so no connection id exists and no callback delivery can happen.
Why: WebSocket clients cannot connect, and the 404 they receive is shaped like a control-plane protocol error rather than a data-plane response.
Fix: the execute-api WebSocket listener and the router entry mapping a deployed WEBSOCKET stage's announced host/path to it (the same root absence reported for service apigatewayv2 in this sweep).
Traces: apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url.

## Evidence
Cause 1: expected `{'IntegrationType': 'AWS_PROXY', 'IntegrationMethod': 'POST', 'PayloadFormatVersion': '1.0', 'ConnectionType': 'INTERNET', 'PassthroughBehavior': 'WHEN_NO_MATCH', 'TimeoutInMillis': 29000}`; actual `{'IntegrationType': 'AWS_PROXY', 'IntegrationMethod': None, 'PayloadFormatVersion': None, 'ConnectionType': 'INTERNET', 'PassthroughBehavior': None, 'TimeoutInMillis': 29000}`.
Cause 2: expected `DefaultRouteSettings == {'DataTraceEnabled': False, 'DetailedMetricsEnabled': False, 'LoggingLevel': 'OFF'}`; actual `undeclared DefaultRouteSettings read None`.
Cause 3: expected HTTP 101 Switching Protocols; actual `the WebSocket upgrade to the prod stage answered 404: 'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService'`.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: CreateApi(ProtocolType=WEBSOCKET) then CreateIntegration(ApiId=..., IntegrationType='AWS_PROXY', IntegrationUri='arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn arn>/invocations') with no other members, then GetIntegrations — IntegrationMethod, PayloadFormatVersion and PassthroughBehavior come back null.
3. Cause 2: CreateStage(ApiId=..., StageName='prod', DeploymentId=...) with no DefaultRouteSettings, then GetStage — DefaultRouteSettings is absent.
4. Cause 3: send an HTTP/1.1 websocket upgrade to the announced prod stage URL — it answers 404 with x-amzn-errortype: UnknownService.
5. Replay the three check ids in trace_ids.
