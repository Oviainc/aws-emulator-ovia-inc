Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint
Issue-Key: apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint|_ws_connect#3|Timeout|assert status >= 500 and time.monotonic() < deadline, (
Issue-Commit: 7f27151474f220c715589d81820913dcd4b7b62b

# apigatewayv2__lambda__logs: the RFC 6455 upgrade to a deployed WEBSOCKET prod stage is now answered a retryable 5xx instead of 101, so the agent-registration check never reaches a definite outcome (1 cause)

> _Note: this issue was identified against the code base at commit `7f27151474f220c715589d81820913dcd4b7b62b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
One net-new root cause, grounded in one failing behavioral check. The divergence is in the execute-api WEBSOCKET data plane, not in the control-plane call named above; every control-plane step the check makes (DynamoDB tables, IAM role, Lambda function, CreateApi/CreateIntegration/CreateRoute/CreateStage/CreateDeployment, log group) succeeds.

Cause 1 of 1: a handshake to a deployed WEBSOCKET stage is answered a 5xx the client must treat as transient, so the WebSocket session never opens and the scenario cannot start.
Failing check ids: apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint

AWS: recorded behavior only (the Smithy model covers only the control plane). An HTTP/1.1 GET carrying Upgrade: websocket, Connection: Upgrade, Sec-WebSocket-Key and Sec-WebSocket-Version: 13 against a deployed WEBSOCKET API stage is answered HTTP/1.1 101 Switching Protocols with the Sec-WebSocket-Accept derived from the client's key; the $connect route's integration then runs (here: writing the agent's row into the connections table), the api's RouteSelectionExpression dispatches subsequent frames, and the handler's reply is pushed back through the stage's https @connections callback URL. A gateway-side failure is a DEFINITE non-2xx (e.g. 500 when the route's integration cannot be invoked, 403 for a stage with no deployment) — not a status a client is expected to retry indefinitely.

Emulator: the handshake is answered a 5xx and keeps answering it. The check's _ws_connect retries only while `status >= 500` (every 3 s, up to its own 120 s propagation deadline) precisely because AWS answers a transient 5xx while a fresh stage/permission settles, so a permanent 5xx makes the call spin and the check is cut off by the harness: "Check timed out after 60.0s". No 101, no connection row, no frame, and none of the scenario's assertions (the connection row, the handler's logged TypeError, the silent client, the repaired reply after WEBSOCKET_ENDPOINT is set) is ever reached. This is a CHANGE of answer since the last sweep: the front door no longer returns 404 UnknownService for this handshake — services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/websocket.rs now CLAIMS it and refuses it from its own serve path (501 NotImplemented with the NO_WEBSOCKET_TRANSPORT sentence when every resolution step passes, or 500 {"message":"Internal server error"} when the $connect connection request is unserved).

Why: a client relying on AWS's behavior sees a status its retry classifier treats as "come back later" for an endpoint that will never upgrade, so it hangs instead of failing, and every behavior that depends on the WebSocket session (route dispatch, @connections replies, $disconnect bookkeeping) is unobservable.

Fix: the WEBSOCKET execute-api serving path — services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/websocket.rs (its claim, its serve, and the connection-request dispatch feeding it) together with the front-door seam that must be able to answer 101 and hand the upgraded connection to the service. Relationship to what is already filed: the open report 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md lists THIS check id in Issue-Trace-Ids, but its replay key for it (_ws_connect#3 | AssertionError | assert status >= 500 and time.monotonic() < deadline) no longer matches — that key was recorded when the answer was 404 UnknownService and the assertion fired immediately; now the answer is a 5xx, the assertion never fires, and the check dies on the harness clock, so the report does not own it and the suite reports it unowned. It is filed here so this outcome is not recorded as working; a fixer closing that report should re-replay this check id too.

Traces: apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint.

## Evidence
From the simparity-behavioral-aws-v1 replay against this checkout.

[mismatch] apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint
  Error: Check timed out after 60.0s

Expected (AWS): the handshake to the deployed `prod` stage is answered HTTP/1.1 101 Switching Protocols with the derived Sec-WebSocket-Accept; the $connect integration runs and the agent's row appears in the connections table.
Actual (emulator): the handshake is answered a 5xx (the apigatewayv2 WebSocket data-plane module's own refusals: 501 NotImplemented carrying the NO_WEBSOCKET_TRANSPORT sentence, or 500 {"message":"Internal server error"} when the $connect connection request is unserved). The check's _ws_connect loop (`assert status >= 500 and time.monotonic() < deadline`, sleeping 3 s between attempts) therefore retries instead of asserting, and the check is cut off at the harness's 60 s budget with no definite handshake outcome.
The previous sweep recorded this same check failing with: "the WebSocket upgrade to the prod stage answered 404: 'HTTP/1.1 404 Not Found … x-amzn-errortype: UnknownService'" — the status changed, the absence of a 101 did not. Host, port, api id and date are per-run and are not requirements; the 101 (and the frames after it) is.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create the scenario's estate: three PAY_PER_REQUEST DynamoDB tables (connections/nodes/deployments), an IAM role, a Node.js Lambda handler, and a WEBSOCKET API with $connect/$default/$disconnect routes bound to an AWS_PROXY integration on that function, with WEBSOCKET_ENDPOINT deliberately unset.
3. CreateStage `prod` and CreateDeployment; wait for DEPLOYED.
4. Open a TCP/TLS socket to the stage's WebSocket URL and send GET /<stage> HTTP/1.1 with Upgrade: websocket, Connection: Upgrade, Sec-WebSocket-Key: <base64 16 bytes>, Sec-WebSocket-Version: 13.
5. Observe a 5xx instead of 101 Switching Protocols; because the client treats a 5xx as a settling stage it retries until the harness cuts the check off. Re-replay apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint to confirm.
