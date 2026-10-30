Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009|_ws_handshake#2|Timeout|sock = ssl.create_default_context().wrap_socket(sock, server_hostname=http_host)
Issue-Commit: aa75f5d199f9816d8a358732bf51b7bd305abb42

# apigatewayv2__dynamodb__lambda: the oversize fragmented client message check never reaches its 1009 CLOSE assertion — the deployed WEBSOCKET stage never completes the RFC 6455 upgrade (1 cause)

> _Note: this issue was identified against the code base at commit `aa75f5d199f9816d8a358732bf51b7bd305abb42`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeWebSocketRoute

## Description
The open issue 20261029T180026Z-apigatewayv2-dynamodb-lambda-api-gateway-v2-websocket-oversize-frame-not-closed.md describes a similar symptom, but the replay suite does not report this check as owned by it (its replay key no longer matches that issue's recorded key), so the failure is filed here. 1 net-new root cause.

Cause 1 of 1: a deployed WEBSOCKET stage never completes the RFC 6455 upgrade, so the client-to-server per-message size ceiling is never exercised and never enforced.

AWS: not modeled in Smithy — the WebSocket execute-api data plane is RFC 6455 framing on a long-lived socket, not a modeled operation (smithy_query over the apigatewayv2 model exposes only the control plane: CreateApi/CreateStage/CreateDeployment/CreateRoute/CreateIntegration; there is no shape for a frame or a close code). Recorded behavior only: a GET to wss://<api-id>.execute-api.<region>.amazonaws.com/<stage> carrying the Upgrade: websocket / Connection: Upgrade / Sec-WebSocket-Key / Sec-WebSocket-Version: 13 headers is answered 101 Switching Protocols with a Sec-WebSocket-Accept derived from the offered key, after which (a) a message sent as RFC 6455 continuation fragments is REASSEMBLED and delivered to the matched route's integration as ONE body of the summed length, and (b) a reassembled message past the per-message ceiling is refused at the protocol level with a CLOSE frame (opcode 0x8) whose status code is 1009 and whose reason is the fixed text `Message too big`, the socket is then torn down, and the integration is never invoked for those bytes. The fixed parts of that refusal are the close opcode 0x8, the code 1009 and the reason string `Message too big`; the only request-derived values are the connection being closed and the message bytes that were refused. Other sockets on the same stage are unaffected — the refusal is message-scoped, not API-scoped.

Emulator: the check does not get far enough to observe any of that. It polls the handshake (its own budget is 150 s) and the suite kills the whole check first: `Error: Check timed out after 60.0s`. No 101 is ever returned, so there is no socket on which to send the 7x16 KiB (114688-byte) accepted control message or the 10x16 KiB (163840-byte) refused one; neither the reassembly guarantee nor the 1009 CLOSE, the tear-down, the 'integration never invoked' guarantee, nor the surviving-twin-socket round trip can be reached.

Why: a client that uses the WebSocket data plane gets nothing at all. It cannot connect, so it cannot send a fragmented message and cannot discover the per-message ceiling: instead of a 1009 CLOSE carrying `Message too big` that tells it to resize and retry, it sees a handshake that never completes and hangs until its own deadline. Code that back-pressures on 1009 never runs, and a target with no per-message ceiling would silently accept oversize bodies once the handshake is fixed.

Fix: the apigatewayv2 WEBSOCKET execute-api data-plane listener — the RFC 6455 upgrade path for a deployed stage (it must answer 101 with Sec-WebSocket-Accept rather than leaving the request unanswered), and behind it the frame reader's continuation-fragment reassembly plus the per-reassembled-message size gate that emits the opcode-0x8 CLOSE with code 1009 and reason `Message too big` and then closes the connection without invoking the route integration.

Traces: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009

## Evidence
Expected: the check connects (101 Switching Protocols) to the deployed WEBSOCKET stage; 7 continuation fragments of 16 KiB arrive at $default as ONE 114688-byte body whose SHA-256 digest and length are echoed back down the same socket and written to the DynamoDB table; the same fragment shape carried to 10 fragments (163840 bytes) is answered with an RFC 6455 CLOSE frame, opcode 0x8, close code 1009, reason exactly b'Message too big'; the socket is then torn down (EOF or reset); no msg# row appears for the refused payload's digest; and the first socket still completes a duplex round trip afterwards.

Actual: [mismatch] apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009 — `Error: Check timed out after 60.0s`. The check is killed inside its handshake poll (_poll_until over _try_ws_connect, timeout_s=150.0, interval_s=3.0, description 'WebSocket handshake against stage <stage>'), i.e. before any frame is sent, so neither the accepted 114688-byte reassembly nor the 1009 CLOSE for the 163840-byte message is ever observed. Sibling checks in the same group fail the same way and are tracked by 20261023T223918Z-apigatewayv2-dynamodb-lambda-all-three-websocket-duplex-checks.md and 20261029T080904Z-apigatewayv2-dynamodb-lambda-a-deployed-websocket-stage-never.md; this check's replay key is carried by neither.

## Steps to Reproduce
1. Boot the emulator from the checkout and replay the behavioral suite check apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009.
2. The check creates a WEBSOCKET API with a $connect/$disconnect/$default Lambda integration backed by a DynamoDB table, creates a stage and a deployment, then polls the RFC 6455 upgrade against wss://<api-id>.execute-api.<region>.amazonaws.com/<stage> with Upgrade: websocket, Connection: Upgrade, Sec-WebSocket-Key and Sec-WebSocket-Version: 13.
3. Observe that the upgrade never returns 101 Switching Protocols, so the check never leaves its handshake poll and is terminated at the 60 s suite cap.
4. After the upgrade is served, re-run and observe the two message legs: 7 fragments of 16 KiB (114688 bytes total) must reach $default as a single body of that length, and 10 fragments (163840 bytes) must be answered with opcode 0x8 / code 1009 / reason 'Message too big', the socket closed, and no msg# row written for the refused digest, while the other socket still round-trips.
