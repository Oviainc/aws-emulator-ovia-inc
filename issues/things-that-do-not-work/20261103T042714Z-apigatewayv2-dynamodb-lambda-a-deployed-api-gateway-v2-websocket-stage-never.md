Issue-Severity: HIGH
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# A deployed API Gateway v2 WEBSOCKET stage never completes the RFC 6455 upgrade, so the no-$default unmatched-action drop path and the binary-frame CLOSE 1003 refusal both reach no verdict (1 cause)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
One net-new root cause. Several open issues describe the same WebSocket data-plane gap for other checks — 20261029T180026Z-apigatewayv2-dynamodb-lambda-api-gateway-v2-websocket-oversize-frame-not-closed.md, 20261029T080904Z-apigatewayv2-dynamodb-lambda-a-deployed-websocket-stage-never.md, 20261029T093753Z-apigatewayv2-dynamodb-lambda-the-binary-frame-refusal-check.md, 20261029T190217Z-apigatewayv2-dynamodb-lambda-a-deployed-websocket-stage-never-2ab070.md, 20261030T123305Z-apigatewayv2-dynamodb-lambda-a-deployed-websocket-stage-never-b8efca.md, 20261102T013348Z-apigatewayv2-dynamodb-lambda-a-deployed-websocket-stage-never-9d3b50.md and 20261023T223918Z-apigatewayv2-dynamodb-lambda-all-three-websocket-duplex-checks.md — but the suite did not mark either of the two checks below as owned by any of them, so their replay keys are not carried. Close them together if the fix is one code path.

=== Cause 1 of 1: the WEBSOCKET stage's data plane never serves the handshake, so no frame-level contract can be observed ===
AWS: a deployed API Gateway v2 API with ProtocolType WEBSOCKET answers an RFC 6455 upgrade on its stage's wss:// endpoint with HTTP 101 Switching Protocols and the correct Sec-WebSocket-Accept, then applies the API's RouteSelectionExpression to each text frame. Two frame-level contracts follow from that: (a) with no $default route configured, a frame whose selected action matches no route is DROPPED — the gateway invokes no integration and the client receives no reply, and the connection stays open; (b) a client-sent BINARY frame is refused and the gateway closes the connection with CLOSE status 1003 (unsupported data). Recorded behavior for the exact close code and the drop semantics.
Emulator: the upgrade never completes, so neither check ever gets a connection to send a frame on; both block in their connect/receive loop until the harness's 60 s cap and are reported as mismatches with "Check timed out after 60.0s" and no verdict at all. (Elsewhere in this same suite a WebSocket upgrade to a deployed stage was answered HTTP 404 with x-amzn-errortype: UnknownService, which is consistent with the stage's wss endpoint not being routed to the WebSocket data plane at all.)
Why: no WebSocket client can connect to a deployed stage. Every frame-level behaviour — route selection, $default fallback, the unmatched-action drop, binary-frame refusal, per-message size limits, CLOSE codes — is unreachable, and a client developed against the emulator cannot be exercised at all, let alone verified. Because the checks observe integration invocation out of band (each invocation writes a DynamoDB row before returning), even the negative contract "no integration was invoked" cannot be distinguished from "nothing happened because nothing connected".
Fix: the apigatewayv2 WEBSOCKET data plane — the deployed stage's endpoint must complete the RFC 6455 handshake (101 + Sec-WebSocket-Accept), then dispatch frames through RouteSelectionExpression, drop unmatched actions when no $default route exists, and close with 1003 on a client binary frame.
Traces: deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered, wsduplex-client-binary-frame-rejected-close-1003.

## Evidence
deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered: [mismatch] Error: Check timed out after 60.0s. The check creates a WEBSOCKET API with three integrations that each write one DynamoDB row per invocation, deploys a stage, opens a wss connection and sends a frame whose action matches no route, then asserts the Query count for that frame is 0 on every integration — it never gets past the connect.
wsduplex-client-binary-frame-rejected-close-1003: [mismatch] Error: Check timed out after 60.0s. The check opens a wss connection to the deployed stage and sends a binary frame, expecting a CLOSE frame with status 1003; it never reaches the assertion.
Corroborating observation from the same suite run (apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle): "the WebSocket upgrade to the prod stage answered 404: 'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService...'".

## Steps to Reproduce
1. apigatewayv2.create_api(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action').
2. Create a Lambda integration and routes for two named actions (no $default route); create a deployment and a stage.
3. Open a WebSocket connection to wss://<api-id>.execute-api.<region>.<host>/<stage> — expect HTTP 101 Switching Protocols with a valid Sec-WebSocket-Accept; observe that the upgrade never completes (a plain HTTP request to the same path answers 404 with x-amzn-errortype: UnknownService).
4. Send {"action": "not-a-route"} and confirm no integration was invoked and the connection stays open.
5. Send a binary frame and expect a CLOSE frame with status code 1003.
Then replay the apigatewayv2__dynamodb__lambda checks named in trace_ids.
