Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__iam__lambda__logs.websocket-header-mapping-connect-only apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers
Issue-Key: apigatewayv2__iam__lambda__logs.websocket-header-mapping-connect-only|_poll_until#10|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers|_poll_until#15|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# WebSocket $connect handshake to a deployed stage answers 500, so no connect event ever reaches the backend

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: the RFC 6455 upgrade to a deployed WebSocket stage is answered with HTTP 500 instead of 101.
AWS: a WebSocket API whose $connect route is wired to a Lambda integration answers a (optionally SigV4-presigned) upgrade request with 'HTTP/1.1 101 Switching Protocols' and then invokes the $connect handler with a requestContext carrying eventType=CONNECT, connectionId, domainName, stage and the handshake headers (Smithy apigatewayv2 CreateApi.ProtocolType=WEBSOCKET, Route.RouteKey '$connect').
Emulator: the upgrade is answered 'HTTP/1.1 500 Internal Server Error' with content-type application/json and body {"message":"Internal server error"}; no connection is established and the $connect integration is never invoked, so the backend logs no handshake event ('the HTTP backend's logged event for this $connect handshake not met within 90.0s. Last result: None'). The status 500 and that body are the divergence; the port and generated api id are per-run values.
Why: every WebSocket client is refused at the handshake, so connect/disconnect routing, header mapping and the whole $connect event contract are unobservable.
Fix: the execute-api WebSocket upgrade handler for a deployed stage (protocol upgrade and $connect route dispatch) in the apigatewayv2 crate.
Note: the open issue 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md describes the same symptom under the apigatewayv2__lambda__logs heading; it does not carry these checks' replay keys, so they are filed here.
Traces: apigatewayv2__iam__lambda__logs.websocket-header-mapping-connect-only, apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers.

## Evidence
Expected: 101 Switching Protocols and a $connect invocation carrying direction/domain/stage and the handshake headers. Actual: 'AssertionError: SigV4-presigned WebSocket handshake upgrading not met within 120.0s. Last result: (500, 'HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json...', '{"message":"Internal server error"}')' and 'AssertionError: the HTTP backend's logged event for this $connect handshake not met within 90.0s. Last result: None'.

## Steps to Reproduce
Replay apigatewayv2__iam__lambda__logs.websocket-header-mapping-connect-only and .ws-connect-event-carries-direction-domain-stage-and-handshake-headers: each creates a WEBSOCKET API with a $connect Lambda integration, deploys a stage and opens a (presigned) WebSocket to the stage URL; the upgrade answers 500.
