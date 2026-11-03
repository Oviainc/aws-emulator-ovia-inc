Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009|_ws_handshake#2|Timeout|return sock, text, rest
Issue-Commit: 6f0c16a0b241f53b494cf0d5c14f6ff644768faf

# apigatewayv2__dynamodb__lambda: the oversize client message / CLOSE 1009 contract is still ungradeable — the deployed WEBSOCKET stage never completes the RFC 6455 upgrade (1 cause)

> _Note: this issue was identified against the code base at commit `6f0c16a0b241f53b494cf0d5c14f6ff644768faf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
ONE net-new root cause, from a check the replay reports as neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: a client RFC 6455 opening handshake on a deployed apigatewayv2 WEBSOCKET stage never reaches '101 Switching Protocols', so the client-to-server per-MESSAGE size ceiling and its CLOSE 1009 refusal are never exercised ===
AWS: the opening handshake and the client-to-server frame/message limits are the execute-api data plane, not modelled operations, so this is recorded behavior only. Recorded: a GET on the deployed stage path carrying Connection: Upgrade, Upgrade: websocket, Sec-WebSocket-Key and Sec-WebSocket-Version: 13 is answered HTTP/1.1 101 Switching Protocols with a Sec-WebSocket-Accept derived from the client key; the socket then accepts a fragmented message whose total payload is under the 128 KiB per-message ceiling (the check sends 7 x 16 KiB = 114688 bytes, each fragment inside the 32 KiB per-frame quota) and the route integration runs for it, while a message over that ceiling (10 x 16 KiB = 163840 bytes) is refused by the service closing the connection with a CLOSE frame carrying status code 1009 (message too big). The modelled surface the same flow feeds back through is ApiGatewayManagementApi PostToConnection POST /@connections/{ConnectionId} (Data @httpPayload blob @length 0..131072, restJson1/sigv4), which the $default integration uses to post the handler's reply back to the caller.
Emulator: the check never gets past the upgrade and is cut by the harness cap — replay detail `Check timed out after 60.0s`. Its helper asserts " 101 " in the first response line (`WebSocket upgrade on stage <stage> was not accepted; expected '101 Switching Protocols', got: ...`) and its readiness variant returns a socket only when that status line contains ' 101 '; neither ever succeeds, although the whole control-plane setup before it (DynamoDB table, Lambda function Active, create_api WEBSOCKET + AWS_PROXY integration + $connect/$disconnect/$default routes, add_permission, create_deployment, create_stage) completes.
Why: without an upgraded socket the emulator enforces no client-to-server message ceiling at all and never emits a CLOSE 1009, so an application that relies on AWS refusing an oversize inbound message (and on under-limit fragmented messages being delivered intact to the route integration) sees unlimited acceptance here; the divergence only appears in production. The whole duplex WebSocket surface for this service combination is unobservable.
Fix: the execute-api data-plane front door / request router plus the apigatewayv2 WebSocket session layer — recognise an upgrade request on a deployed WEBSOCKET-protocol API's stage path, complete the handshake (101 + Sec-WebSocket-Accept) instead of falling through to the generic not-found path, reassemble continuation frames, enforce the per-frame and per-message ceilings, and close with status code 1009 when a reassembled message exceeds the per-message limit.
Related (does NOT cover this check): the open issues 20261030T180049Z-apigatewayv2-dynamodb-lambda-the-client-to-server-per.md, 20261030T071735Z-apigatewayv2-dynamodb-lambda-the-oversize-fragmented-client-message.md and 20261103T042714Z-apigatewayv2-dynamodb-lambda-a-deployed-api-gateway-v2-websocket-stage-never.md describe the same missing upgrade for other check/replay keys; the replay reports no owner for the check listed here, so it is filed with this issue's replay key.
Traces: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009.

## Evidence
Suite: simparity-behavioral-aws-v1. Totals: 0/47 passed; apigatewayv2__dynamodb__lambda 0/4 (0%), of which three are OWNED by 20261023T223918Z-apigatewayv2-dynamodb-lambda-all-three-websocket-duplex-checks.md and this fourth one is not owned.

[mismatch] apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009
  Error: Check timed out after 60.0s

From the check source carried in the report: SERVICES = ["apigatewayv2", "dynamodb", "lambda"]; GROUP = "websocket-duplex"; _FRAGMENT = 16 * 1024 (inside the 32 KiB per-frame quota), _UNDER_LIMIT_FRAGMENTS = 7 (114688 bytes, under the 128 KiB per-message ceiling), _OVER_LIMIT_FRAGMENTS = 10 (163840 bytes, over it).
_ws_connect(): sends `GET <stage path> HTTP/1.1` with Host, `Upgrade: websocket`, `Connection: Upgrade`, `Sec-WebSocket-Key: <base64 16 bytes>`, `Sec-WebSocket-Version: 13` and asserts `" 101 " in lines[0]` with the message "WebSocket upgrade on stage {stage} was not accepted; expected '101 Switching Protocols', got: {lines[0]!r}"; _try_ws_connect() returns a socket only when the status line contains ' 101 ' and None otherwise. The check hangs on this upgrade and is cut by the 60 s harness cap, so neither the under-limit delivery nor the expected CLOSE frame with status code 1009 is ever observed.

Model facts used (smithy_query, model_path services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/smithy-model/com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json), for the post-back leg the handler uses: PostToConnection POST /@connections/{ConnectionId}, Data @httpPayload blob @length {min 0, max 131072}, error PayloadTooLargeException @httpError 413; restJson1 / sigv4. The handshake and the client-to-server message ceiling are not modelled (recorded behavior only).

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, and replay apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009 from simparity-behavioral-aws-v1.
2. Setup the check performs: create a DynamoDB table; create a Lambda function (python runtime) whose handler hashes the inbound body, stores it in the table and posts a reply back with apigatewaymanagementapi PostToConnection against the endpoint built from requestContext.domainName + stage; create an apigatewayv2 API with ProtocolType=WEBSOCKET, an AWS_PROXY integration to that function, routes $connect/$disconnect/$default, lambda add_permission for apigateway.amazonaws.com, a deployment and the stage 'dev'.
3. Open a TCP (or TLS) connection to ctx.api_url(api_id, 'dev', websocket=True) and send the RFC 6455 opening handshake (GET on the stage path with Connection: Upgrade, Upgrade: websocket, Sec-WebSocket-Key, Sec-WebSocket-Version: 13).
4. Observe that the first response line never contains ' 101 ', so no socket is ever established; the check hangs and is reported as 'Check timed out after 60.0s'.
5. With the upgrade fixed, the remaining contract must hold: a message sent as 7 masked fragments of 16 KiB (114688 bytes total, FIN only on the last) is reassembled and delivered to the $default integration, whose reply comes back as a TEXT frame; a message sent as 10 such fragments (163840 bytes) is refused by the service with a CLOSE frame whose status code is 1009.
6. Re-replay exactly the check id in trace_ids to confirm.
