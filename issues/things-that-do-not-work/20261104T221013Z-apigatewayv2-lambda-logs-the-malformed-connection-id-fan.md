Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request
Issue-Key: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request|_ws_connect#7|AssertionError|assert status >= 500 and time.monotonic() < deadline, (
Issue-Commit: d9cf74aafe764d4a8806a821a1189d14d75bb9bb

# apigatewayv2__lambda__logs: the malformed-connection-id fan-out check cannot proceed — the RFC 6455 upgrade to the deployed prod WEBSOCKET stage is answered 404 Not Found with x-amzn-errortype: UnknownService

> _Note: this issue was identified against the code base at commit `d9cf74aafe764d4a8806a821a1189d14d75bb9bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
ONE net-new root cause on the apigatewayv2__lambda__logs service token, from the single failing check in that group the replay marks neither ALREADY FILED nor OWNED. It must be closed before this issue is submitted.

=== Cause 1 of 1: a client WebSocket opening handshake against a deployed apigatewayv2 WEBSOCKET stage is not claimed by any emulated service — the front door answers HTTP/1.1 404 Not Found with x-amzn-errortype: UnknownService — so no connection is ever established and the check reaches no verdict ===

AWS: the opening handshake is the execute-api data plane and is not a modelled operation, so this is recorded behavior only. Recorded: a GET on the deployed stage path of a WEBSOCKET-protocol API carrying Connection: Upgrade, Upgrade: websocket, Sec-WebSocket-Key: <base64 16 bytes> and Sec-WebSocket-Version: 13 is answered HTTP/1.1 101 Switching Protocols with a Sec-WebSocket-Accept derived from the client key; the $connect route runs, the service mints the connection id, and that id is what the modelled @connections surface then addresses (Smithy ApiGatewayManagementApi, restJson1/sigv4: GetConnection GET /@connections/{ConnectionId}; PostToConnection POST /@connections/{ConnectionId} with Data @httpPayload blob @length 0..131072; DeleteConnection DELETE /@connections/{ConnectionId} code 204; common errors ForbiddenException 403, GoneException 410, LimitExceededException 429). An upgrade request to a deployed stage is never answered 404, and never with an x-amzn-errortype header: a 404 with an AWS JSON error envelope is the answer of a service router that did not recognise the request as belonging to any service at all.

Emulator: the upgrade is answered `HTTP/1.1 404 Not Found` with `content-type: application/x-amz-json-1.1`, `x-amzn-errortype: UnknownService` and a 149-byte JSON error body. The check's _ws_connect helper retries while the status is >= 500 and asserts on anything else, so it fails immediately with: `AssertionError: the WebSocket upgrade to the prod stage answered 404: 'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService\r\ncontent-length: 149\r\n...'`. Everything before that point works: the three DynamoDB tables reach ACTIVE, the IAM role and the Lambda are created, the WEBSOCKET API with $connect/$default/$disconnect routes is created and deployed to the stage 'prod', and the check's first assertion — PostToConnection to a malformed connection id answering BadRequestException 'Invalid connectionId: <id>' — already passed. Only the upgrade is missing.

Why: with no 101 and no minted connection id, nothing downstream of a WebSocket client exists against this emulator: $connect/$default/$disconnect never fire, the registration handler never writes its connections row, server-push fan-out to a live socket cannot be exercised, and the application contract this check grades — that a 400 BadRequestException must NOT trigger a handler's GoneException-only prune while a LIVE connection on the same stage is still notified — is ungradeable. A developer pointing a wss:// client at the stage gets an AWS-shaped 'UnknownService' 404, which reads as "this endpoint is not an AWS service" rather than "this emulator has no WebSocket data plane", so a correctly configured client looks like a configuration error.

Fix: the emulator's front door / service router plus the apigatewayv2 WEBSOCKET session layer — recognise an upgrade request addressed to a deployed WEBSOCKET-protocol API's stage path, hand it to the apigatewayv2 service instead of falling through to the router's UnknownService 404, complete the handshake (101 Switching Protocols + Sec-WebSocket-Accept), mint a connection id, dispatch $connect/$disconnect and route selection, and register the connection in the store the @connections handlers read. This is a capability gap in the data plane, not an error-shape bug in a handler.

Related (does NOT cover this check): the open issue 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md describes the same unservable upgrade and owns the four sibling checks in this group (default-route-response-is-silent-until-redeployed..., registration-prunes-a-stale-ui-connection..., registration-reply-carries-the-node-id..., unmatched-action-without-a-default-route-is-rejected-forbidden); the replay reports NO owner for post-to-a-malformed-connection-id-is-rejected-bad-request, so its replay key is carried here. The apigatewaymanagementapi-side issues 20261023T223918Z-... and 20261103T054456Z-... cover the same missing upgrade under a different service token.

Traces: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request.

## Evidence
Suite: simparity-behavioral-aws-v1. Totals: 0/47 passed (0.0%); apigatewayv2__lambda__logs 0/5 (0%). Four of the five are OWNED by 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md; the one below is not.

[mismatch] apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request
  File "check_apigatewayv2__lambda__logs_post_to_a_malformed_connection_id_is_rejected_bad_request.py", line 43, in run
    live = _ws_connect(ctx, est)
  File "...", line 529, in _ws_connect
    assert status >= 500 and time.monotonic() < deadline, (
  AssertionError: the WebSocket upgrade to the prod stage answered 404: 'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService\r\ncontent-length: 149\r\ndate: Wed, 04 Nov 2026 21:40:20 GMT'

Expected (real AWS): the status line HTTP/1.1 101 Switching Protocols with Upgrade: websocket, Connection: Upgrade and Sec-WebSocket-Accept = base64(SHA1(Sec-WebSocket-Key + the RFC 6455 GUID)); the $connect integration runs and a connection id is minted.
Actual (emulator): HTTP/1.1 404 Not Found with x-amzn-errortype: UnknownService and an AWS JSON 1.1 error body — i.e. the request matched no service claim at the router, so no upgrade was attempted. (Only the status line, headers and the error type are requirements here; the date and content-length of that one recording are per-run values.)

Check shape (what already worked before the failing step): run() builds the estate (three PAY_PER_REQUEST DynamoDB tables polled to ACTIVE, an IAM role with AWSLambdaBasicExecutionRole + table access + execute-api:ManageConnections, a Lambda handler, a WEBSOCKET API with $connect/$default/$disconnect and the stage 'prod'), asserts post_to_connection(ConnectionId='AAAAAAAAAAAAAAA=', Data=b'x') answers ('BadRequestException', 'Invalid connectionId: AAAAAAAAAAAAAAA=') — which PASSES — plants that id as a 'ui' row in the connections table, and then calls _ws_connect to obtain a LIVE socket. The traceback is at that call, so every assertion about the live socket, the registration reply, the node_discovered notification, the 'Failed to notify UI <id>' log line and the surviving planted row is unreached.

Model facts used (smithy_query, model_path services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/smithy-model/com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json): GetConnection GET /@connections/{ConnectionId} code 200; PostToConnection POST /@connections/{ConnectionId}, Data @httpPayload blob @length{min 0,max 131072}, errors ForbiddenException(403)/GoneException(410)/LimitExceededException(429)/PayloadTooLargeException(413); DeleteConnection DELETE /@connections/{ConnectionId} code 204. The handshake itself is modelled nowhere (execute-api data plane), hence recorded behavior only.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, and replay apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request from simparity-behavioral-aws-v1.
2. Minimal reproduction: create an apigatewayv2 API with ProtocolType=WEBSOCKET and RouteSelectionExpression=$request.body.action; create an AWS_PROXY integration to a Lambda function; create the routes $connect, $default and $disconnect targeting it; add the lambda:InvokeFunction permission for apigateway.amazonaws.com; create a deployment and the stage 'prod'.
3. Open a TCP (or TLS) connection to that API's stage URL and send an RFC 6455 opening handshake: `GET /prod HTTP/1.1` with Host, `Connection: Upgrade`, `Upgrade: websocket`, `Sec-WebSocket-Key: <base64 of 16 random bytes>`, `Sec-WebSocket-Version: 13`.
4. Observe the response status line `HTTP/1.1 404 Not Found` with `x-amzn-errortype: UnknownService` and an application/x-amz-json-1.1 error body, instead of `HTTP/1.1 101 Switching Protocols` with a Sec-WebSocket-Accept derived from the key. No connection id is minted, so nothing can be posted to a live connection.
5. After fixing, re-replay the check id in trace_ids: the upgrade must answer 101 with a valid Sec-WebSocket-Accept, $connect/$default must fire, the registration reply must come back over the socket, and the check must reach its assertions that a 400 BadRequestException does not trigger the handler's GoneException-only prune while the live UI connection is still notified.
