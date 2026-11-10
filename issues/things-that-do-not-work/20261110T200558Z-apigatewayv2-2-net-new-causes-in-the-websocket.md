Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.websocket-connect-mock-integration-rejected apigatewayv2.websocket-inbound-binary-frame-rejected apigatewayv2.websocket-inbound-message-32kb-boundary apigatewayv2.websocket-route-selection-by-action apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client
Issue-Key: apigatewayv2.websocket-connect-mock-integration-rejected|_ws_read#39|AssertionError|raise AssertionError("the WebSocket closed while a frame was being read")
Issue-Key: apigatewayv2.websocket-inbound-binary-frame-rejected|_recv_exact#11|TimeoutError|chunk = self._sock.recv(65536)
Issue-Key: apigatewayv2.websocket-inbound-message-32kb-boundary|_recv_exact#14|ConnectionError|raise ConnectionError("websocket connection closed by peer")
Issue-Key: apigatewayv2.websocket-route-selection-by-action|_ws_read#36|AssertionError|raise AssertionError("the WebSocket closed while a frame was being read")
Issue-Key: apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client|run#23|AssertionError|assert hello.get("marker") == marker, (
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewayv2: 2 net-new causes in the WEBSOCKET execute-api data plane — an upgraded connection is closed mid-frame, and a two-way route's reply is replaced by the gateway's "Internal server error" frame

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
The WebSocket handshake now reaches 101 (earlier open issues such as 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md and 20261107T193414Z-apigatewayv2-the-websocket-execute-api-data-plane-answers.md describe the upgrade being refused 404/500 and do NOT carry these checks' replay keys), so these checks now get PAST the upgrade and fail on what happens to frames afterwards. Two distinct causes.

Cause 1 of 2: an upgraded WEBSOCKET connection is closed by the emulator instead of carrying frames.
  AWS: recorded behavior only — the execute-api WebSocket data plane is not in the Smithy model. After a 101 Switching Protocols, AWS keeps the connection open for the configured idle timeout (10 minutes) and answers each client data frame by dispatching the selected route; a refusal is delivered as a WebSocket frame or an RFC 6455 CLOSE with the documented status code (1003 for an unsupported binary frame, 1009 for a message over the 128 KB client-to-server ceiling), never by a bare TCP close. The connection stays usable for subsequent frames in every one of these recordings.
  Emulator: immediately after the upgrade the peer closes the socket with no CLOSE frame. The checks observe this three ways, all the same cause: 'the WebSocket closed while a frame was being read' (AssertionError raised when 0 bytes of the 2-byte frame header arrive), 'websocket connection closed by peer' (ConnectionError), and 'The read operation timed out' (ssl.py TimeoutError — nothing is ever written back).
  Why: a client that completes a handshake can neither send nor receive. Route selection, MOCK-integration refusal on $connect, the unsupported-binary-frame CLOSE 1003 and the 32 KB inbound boundary are all unobservable, and a real client sees a transport-level abort where AWS gives it a frame or a coded CLOSE.
  Fix: the apigatewayv2 WEBSOCKET execute-api data plane — the post-upgrade frame loop/connection registry that must own the socket after 101, read client frames, dispatch to the route and emit CLOSE frames with the documented codes.
  Traces: apigatewayv2.websocket-connect-mock-integration-rejected, apigatewayv2.websocket-route-selection-by-action, apigatewayv2.websocket-inbound-message-32kb-boundary, apigatewayv2.websocket-inbound-binary-frame-rejected.

Cause 2 of 2: the frame delivered to the client is the gateway's generic internal-error envelope instead of the route's reply.
  AWS: Smithy (apigatewaymanagementapi): PostToConnection POST /@connections/{ConnectionId}, Data @httpPayload @length min 0 max 131072, success code 200, modeled errors ForbiddenException/403, GoneException/410, LimitExceededException/429, PayloadTooLargeException/413. On success AWS delivers EXACTLY the Data bytes the caller posted to that connection's socket — it never substitutes a body of its own. A two-way route's RouteResponse likewise returns the integration's response payload.
  Emulator: the client socket receives the gateway error envelope {"connectionId":"<id>","message":"Internal server error","requestId":"<id>"} in place of the integration's payload. Fixed text of that envelope is the member names connectionId/message/requestId and the message string 'Internal server error'; the id values are per-run and are not part of any requirement. The check asserted the frame carried its own marker and got that envelope instead.
  Why: a client relying on a two-way route (or on any @connections push) receives an error document where the application payload should be, so every request/response protocol built on a WebSocket API silently breaks — and because the envelope is delivered with no error status on the management call, the caller believes the push succeeded.
  Fix: the WEBSOCKET route-dispatch/route-response path and the @connections push path of the apigatewayv2 execute-api data plane — the integration response must be framed to the client verbatim, and a genuine internal failure must surface on the PostToConnection call rather than as a substituted client frame.
  Traces: apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client.

## Evidence
Cause 1 — expected: after 101, each client frame is answered (or refused with CLOSE 1003/1009) and the socket stays open.
actual (apigatewayv2.websocket-connect-mock-integration-rejected, apigatewayv2.websocket-route-selection-by-action): AssertionError: the WebSocket closed while a frame was being read.
actual (apigatewayv2.websocket-inbound-message-32kb-boundary): ConnectionError: websocket connection closed by peer.
actual (apigatewayv2.websocket-inbound-binary-frame-rejected): TimeoutError: The read operation timed out (ssl.py read) — no byte is ever written back.

Cause 2 — expected: the control frame carries the integration's payload (the check's marker).
actual (apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client): AssertionError: the control frame must come from this check's integration (marker 'sim2real-behav-ws-<id>'), got {'connectionId': '<id>', 'message': 'Internal server error', 'requestId': '<id>'}.
Smithy: PostToConnection success code 200 with no response body; Data @length 0..131072; errors ForbiddenException(403)/GoneException(410)/LimitExceededException(429)/PayloadTooLargeException(413).

## Steps to Reproduce
1. CreateApi(ProtocolType=WEBSOCKET, RouteSelectionExpression=$request.body.action); create $connect/$default/named routes with integrations; CreateStage + CreateDeployment.
2. Complete an RFC 6455 upgrade to wss://<stage endpoint> and read: the socket is closed (no CLOSE frame) or never written to (Cause 1).
3. For a two-way route, create a RouteResponse, send a frame and read the reply: the frame is {"connectionId":...,"message":"Internal server error","requestId":...} rather than the integration's payload (Cause 2).
4. Replay the checks in trace_ids.
