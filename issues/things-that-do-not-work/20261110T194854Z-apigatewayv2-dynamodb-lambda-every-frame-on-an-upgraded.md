Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003 apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009
Issue-Key: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect|run#27|AssertionError|assert json.loads(pushed).get("echo") == text, (
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003|run#10|AssertionError|assert json.loads(pushed).get("sha") == text_sha, (
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake|run#13|AssertionError|assert json.loads(pushed).get("sha") == marker_sha, (
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009|run#10|AssertionError|assert pushed.get("len") == len(under), (
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewayv2__dynamodb__lambda: every frame on an upgraded WebSocket stage is answered with the gateway's 'Internal server error' frame, so no route integration result or PostToConnection push ever reaches the client (4 checks)

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
ONE net-new root cause on the apigatewayv2__dynamodb__lambda service token, covering the 4 failures of this replay that the report marks neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: frames sent on a deployed WEBSOCKET stage are answered with API Gateway's error frame {"connectionId":...,"message":"Internal server error","requestId":...} — the route's Lambda integration result, and the PostToConnection push the integration makes, never reach the client ===

AWS: recorded behavior only for the WebSocket data plane; the push itself is apigatewaymanagementapi PostToConnection (POST /@connections/{ConnectionId}, Data @httpPayload blob @length max 131072, 200 with empty body). Recorded: a text frame is routed by the API's RouteSelectionExpression to its route, the AWS_PROXY integration is invoked with a requestContext carrying routeKey/connectionId/domainName/stage, and whatever that integration sends back — a route response body, or an out-of-band PostToConnection to the same or another connection — is delivered to the client as a TEXT frame containing exactly those bytes. Fragmented client frames are reassembled into ONE message before the integration sees them (the check sends 7 fragments and expects the integration to report a single 114688-byte body). The {"message":"Internal server error"} frame is reserved for a genuinely failed integration; its body is {"message":"Internal server error","connectionId":"<connection id>","requestId":"<request id>"}, where 'Internal server error' is fixed text and the two ids are per-request values.

Emulator: the handshake upgrades and a connection id is minted ('EzCyjTUad5uklZ0=' this run), but every frame the client reads back is the gateway's error frame instead of the expected payload — the echo of the client's message, the SHA-256 digest the integration computes and pushes with PostToConnection, and the reassembled-length report are all replaced by `{'connectionId': 'EzCyjTUad5uklZ0=', 'message': 'Internal server error', 'requestId': '<id>'}`. So the route's integration invocation (or the relay of its result / its PostToConnection push) fails internally for every frame, on the $default route, on two-way routes, and on the admitted socket of a $connect-authorizer scenario alike.

Why: a WebSocket application cannot do anything at all — no echo, no request/reply, no server push, no DynamoDB-backed connection registry flow — and every failure looks like the customer's own handler crashing, so there is no way to tell a wiring defect from a backend bug. Secondary contracts the checks grade behind this (a client binary frame must close the socket with 1003, an oversize client message with 1009, a $connect denial must refuse the handshake while the admitted twin keeps working, fragments must be reassembled into one message) are all unobservable while every frame answers 500.

Fix: the apigatewayv2 WEBSOCKET data-plane session/route dispatcher — route selection, frame reassembly before dispatch, the AWS_PROXY integration invocation and the relay of its result/route response back on the socket — plus the @connections push path the integration calls into (the same PostToConnection surface that is dropping connections in the apigatewaymanagementapi service of this replay).

Related, does NOT cover these 4 checks: 20261109T154749Z-apigatewayv2-dynamodb-lambda-3-net-new-causes-the.md describes 'the WebSocket handshake never upgrades within the run budget' plus 'upgraded frames answer Internal server error' for a different check set and carries only those checks' replay keys, and 20261110T155738Z-apigatewayv2-dynamodb-lambda-websocket-route-dispatch-never-completes.md records the never-completing variant; the replay reports no owner for the four listed here.

Traces: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect, apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003, apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake, apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009.

## Evidence
Suite: simparity-behavioral-aws-v1. apigatewayv2__dynamodb__lambda 0/4 (0%), 4 failed, none marked ALREADY FILED or OWNED. All four are [mismatch] and all four show the same gateway error frame where a payload was expected:

[mismatch] websocket-api-connect-message-post-back-disconnect
  AssertionError: expected PostToConnection to deliver {'echo': 'ws-payload-6b9cb0f837e3'} on the client socket, got '{"connectionId":"EzCyjTUad5uklZ0=","message":"Internal server error","requestId":"c240a87a0157068b"}' (nothing arriving means the management API accepted the call and dropped the payload)
[mismatch] wsduplex-client-binary-frame-rejected-close-1003
  AssertionError: the integration's PostToConnection should have delivered the digest of 'wsduplex-text-b930db0b0b72' (122ecde5ae9dd8cd018e232c8cbdc946ffbf284de5faf1e50cc05faf2f7b7c3f) on the client socket, got '{"connectionId":"EzCyjTUad5uklZ0=","message":"Internal server error","requestId":"c240a8c5c551b19f"}'
[mismatch] wsduplex-connect-denial-rejects-handshake
  AssertionError: the admitted socket should have received the integration's PostToConnection with the digest of 'wsduplex-msg-5be06a07ba3e' (55380cb7fa60874c7149581b4ac5a58e6451228f0e2ddfd82fc11a9049bd0c8c), got '{"connectionId":"EzCyjTUad5uklZ0=","message":"Internal server error","requestId":"c240a8f5015479f9"}'
[mismatch] wsduplex-oversize-client-message-rejected-close-1009
  AssertionError: $default should have seen the 7 fragments as ONE 114688-byte message; the integration reported {'connectionId': 'EzCyjTUad5uklZ0=', 'message': 'Internal server error', 'requestId': 'c240a80fb9e1c5b0'} (a per-fragment length means the target never reassembled the message)

Expected (recorded AWS): the TEXT frame read back carries the integration's bytes — the echo object, the SHA-256 digest of the sent text, or the single reassembled message length. Actual (emulator): the gateway's 'Internal server error' frame. Payload markers, digests, connection id and request ids are per-run values; the requirement is that the integration's result/push is what arrives.

## Steps to Reproduce
1. CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action') with AWS_PROXY Lambda integrations on $connect/$disconnect/$default (+ a two-way route with a route response); the handler records the connection in DynamoDB on $connect and pushes a reply with apigatewaymanagementapi PostToConnection. Grant lambda:InvokeFunction to apigateway.amazonaws.com, CreateDeployment + CreateStage.
2. Open an RFC 6455 handshake to the stage URL (it upgrades, 101) and send a text frame for the route.
3. Observe the frame read back is {"connectionId":...,"message":"Internal server error","requestId":...} instead of the integration's echo/digest body, i.e. neither the route result nor the out-of-band PostToConnection payload is delivered.
4. Send 7 continuation fragments forming one 114688-byte message and observe the integration never reports the reassembled single message.
5. After fixing, re-replay the 4 ids in trace_ids.
