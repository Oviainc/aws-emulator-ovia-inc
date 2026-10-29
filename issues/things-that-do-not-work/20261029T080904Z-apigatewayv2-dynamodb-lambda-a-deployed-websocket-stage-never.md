Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003|_ws_handshake#2|Timeout|sock = ssl.create_default_context().wrap_socket(sock, server_hostname=http_host)
Issue-Commit: 8acf21a252327a123d13b159a352e2f9f2a954c2

# apigatewayv2__dynamodb__lambda: a deployed WEBSOCKET stage never completes the RFC 6455 upgrade, so a client binary frame is never refused with CLOSE 1003 (1 cause)

> _Note: this issue was identified against the code base at commit `8acf21a252327a123d13b159a352e2f9f2a954c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
$connect (apigatewayv2 WebSocket data plane)

## Description
This issue carries ONE root cause. Cause 1 of 1 must be closed before submitting.

Cause 1 of 1: the WebSocket data plane of a deployed apigatewayv2 WEBSOCKET stage never answers the opening handshake, so neither the duplex round trip nor the binary-frame refusal it brackets can happen. Failing check id: apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003.

AWS: recorded/documented behavior only — the opening handshake and frame routing are the execute-api data plane, not a modelled operation. Model context (apigatewayv2): Api.ProtocolType enum [WEBSOCKET, HTTP]; Api.RouteSelectionExpression is required and selects the route from the frame; Api.ApiEndpoint is `{api-id}.execute-api.{region}.amazonaws.com` with the stage name appended; push-back is ApiGatewayManagementApi PostToConnection (POST /@connections/{ConnectionId}, restJson1). What the recording requires: the upgrade is answered `HTTP/1.1 101 Switching Protocols` with a Sec-WebSocket-Accept derived from the client key; $connect runs with requestContext.connectionId/domainName/stage; a client TEXT frame (opcode 0x1) is routed to $default, which writes a msg#<sha256> row and pushes the digest back on the same socket; a client BINARY frame (opcode 0x2) on the same socket is answered by the service with an RFC 6455 CLOSE frame (opcode 0x8) carrying close code 1003 and the close reason `Binary is not supported`, the socket is then torn down (a following recv returns b''), $default is never invoked for that payload (no msg#<sha256> row for its digest), and a fresh socket on the same stage still completes the duplex round trip. The close code 1003 and the reason text `Binary is not supported` are fixed; the markers and digests in the recording are per-run values.

Emulator: no 101 is ever returned on the deployed stage. The check blocks in its handshake poll (`_poll_until(lambda: _try_ws_connect(ctx, api_id, stage), ..., timeout_s=150.0, description='WebSocket handshake against stage <stage>')`) and is cut by the harness cap, reporting `Error: Check timed out after 60.0s`. Because nothing is upgraded, no CLOSE frame, no close code, no close reason and no absence-of-invocation evidence can be produced.

Why: a client gets no WebSocket data plane at all — wss:// never upgrades, $connect/$default/$disconnect never fire, nothing is written to DynamoDB, PostToConnection push-back never happens, and the opcode policy that AWS enforces (refuse binary with 1003 and close) is indistinguishable from an API that relays binary frames straight to the integration. Instead of a wrong answer the client gets an indefinite wait.

Fix: the execute-api data-plane front door plus the apigatewayv2 WebSocket session layer — recognise the RFC 6455 upgrade on a deployed WEBSOCKET API's stage path and answer 101 + Sec-WebSocket-Accept, mint a connection id, invoke $connect, route frames through the route-selection expression, enforce the frame-opcode policy (CLOSE 1003 `Binary is not supported`, then close, without invoking the route), and fire $disconnect on close.

Note: the open issue 20261023T223918Z-apigatewayv2-dynamodb-lambda-all-three-websocket-duplex-checks.md describes this same silent handshake and lists this check id, but under a stale replay key (`_poll_until#2|Timeout|time.sleep(interval_s)`); in this replay the failure surfaces as the bare harness cap with no frame, so the suite did not mark this check owned by it. The sibling check wsduplex-connect-denial-rejects-handshake is owned by 20261027T173333Z-apigatewayv2-dynamodb-lambda-the-connect-denial-websocket-leg-68dc11.md, which does not carry this one.

Traces: apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003.

## Evidence
Expected (AWS): HTTP/1.1 101 on the deployed WEBSOCKET stage; a text frame round-trips (the $default integration's PostToConnection delivers {"sha": <sha256 of the payload>, "len": <byte length>} on the client socket and a msg#<sha> row appears); a binary frame is answered with opcode 0x8, close code 1003 and close reason b'Binary is not supported', the socket is then closed (recv -> b''), no msg# row exists for the binary payload's digest, and a second handshake on the same stage completes the round trip again.

Actual (emulator): apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003 [mismatch] — `Error: Check timed out after 60.0s`. The check never gets past `_poll_until(... description=f"WebSocket handshake against stage {api['stage']}")`; control-plane setup (CreateApi ProtocolType=WEBSOCKET, AWS_PROXY integration, $connect/$disconnect/$default routes, lambda AddPermission, CreateDeployment, CreateStage) completed, so the divergence is the absence of any upgrade response on the stage endpoint. Generated api ids, stage names, markers and digests differ per run and are not requirements.

Model facts used: apigatewayv2 Api.ProtocolType [WEBSOCKET, HTTP]; Api.RouteSelectionExpression required; Api.ApiEndpoint '{api-id}.execute-api.{region}.amazonaws.com' with the stage appended; ApiGatewayManagementApi PostToConnection POST /@connections/{ConnectionId} (restJson1).

## Steps to Reproduce
1. Boot the emulator and replay apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003 from simparity-behavioral-ga-prod-v1.
2. Create a DynamoDB table and a Lambda; CreateApi(ProtocolType=WEBSOCKET, RouteSelectionExpression='$request.body.action'); create an AWS_PROXY integration on the function; create routes $connect, $disconnect, $default; lambda AddPermission for apigateway; CreateDeployment; CreateStage.
3. Open an RFC 6455 handshake against wss://<api-id>.execute-api.<region>.<host>/<stage> and poll for a 101.
4. Observe no 101 ever arrives, so the text-frame round trip, the binary-frame CLOSE 1003 'Binary is not supported' refusal, the absence check on the msg# row and the second-socket positive control are all unreachable; the check is killed at the harness cap.
