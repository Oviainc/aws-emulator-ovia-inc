Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003 apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009 apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake|_poll_until#4|AssertionError|raise AssertionError(
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003|_poll_until#2|AssertionError|raise AssertionError(
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009|_poll_until#2|AssertionError|raise AssertionError(
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration|_poll_until#27|AssertionError|raise AssertionError(
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered|_poll_until#20|AssertionError|raise AssertionError(
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# apigatewayv2__dynamodb__lambda: a deployed WebSocket API stage never completes a WebSocket handshake, so no route, authorizer or close-code behaviour can be exercised

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: the execute-api data plane never upgrades a WebSocket connection on a deployed stage.
AWS: a WebSocket API (apigatewayv2 CreateApi with ProtocolType=WEBSOCKET, RouteSelectionExpression) deployed to a stage accepts `wss://<api-id>.execute-api.<region>.amazonaws.com/<stage>` and answers the Upgrade request with HTTP/1.1 101 Switching Protocols once the $connect route's integration (or its REQUEST authorizer) allows the connection; a denying $connect authorizer or a $connect integration returning non-2xx instead fails the handshake (403/502, no 101). Only after a 101 does route selection apply, so the documented downstream behaviours — $default vs a custom action route, an unmatched action with no $default being dropped, a binary frame being closed with 1003, and an over-128 KiB client frame being closed with 1009 — are reachable.
Emulator: the handshake never completes at all. Every one of these checks polls for a successful (or explicitly refused) handshake against stage `dev` and sees nothing: `WebSocket handshake against stage dev not met within 150.0s. Last result: None`. Because no connection is ever established, neither the Allow nor the Deny branch of the $connect authorizer, nor any close code, is observable.
Why: a client cannot open a WebSocket connection to an API it has created and deployed, so the whole WebSocket surface — connection management, route selection, authorizers and protocol-level close codes — is unusable, and a deny-path test cannot distinguish "correctly refused" from "never served".
Fix: the execute-api data-plane router and the apigatewayv2 WebSocket connection handler — the stage path must be recognised and the RFC 6455 upgrade performed, with $connect authorization/integration deciding 101 vs refusal, before route dispatch and frame-policy enforcement can be fixed.
Traces: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake, .wsduplex-client-binary-frame-rejected-close-1003, .wsduplex-oversize-client-message-rejected-close-1009, .deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration, .deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered. Related open issues that do not carry these checks' replay keys: 20261023T223918Z-apigatewayv2-dynamodb-lambda-all-three-websocket-duplex-checks.md and 20261029T065950Z-apigatewayv2-dynamodb-lambda-the-deployed-execute-api-data.md.

## Evidence
Expected: `HTTP/1.1 101 Switching Protocols` on the deployed stage's wss endpoint for an allowed $connect, and an explicit handshake refusal (no 101) for a denying $connect authorizer. Actual, for all five checks: `AssertionError: WebSocket handshake against stage dev not met within 150.0s. Last result: None` (and, for the authorizer check, `an Allow-token WebSocket handshake against stage dev of <api-id> not met within 150.0s. Last result: None`). No connection is ever established, so close codes 1003/1009 and route selection are never reached.

## Steps to Reproduce
CreateApi(ProtocolType=WEBSOCKET, RouteSelectionExpression="$request.body.action"), create $connect/$disconnect/$default routes with Lambda integrations, CreateDeployment and CreateStage("dev"), then open a WebSocket handshake to wss://<api-id>.execute-api.<region>.amazonaws.com/dev. Observe no 101 and no explicit refusal. Re-replay the five checks in trace_ids.
