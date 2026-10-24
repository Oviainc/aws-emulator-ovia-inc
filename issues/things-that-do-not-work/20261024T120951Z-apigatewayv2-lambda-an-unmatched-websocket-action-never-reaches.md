Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.websocket-unmatched-route-goes-to-default-route
Issue-Key: apigatewayv2__lambda.websocket-unmatched-route-goes-to-default-route|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# apigatewayv2__lambda: an unmatched WebSocket action never reaches $default — the deployed stage serves nothing (1 cause)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api WebSocket route dispatch)

## Description
Cause 1 of 1: the deployed WEBSOCKET stage never serves, so the $default fallback for an unmatched route key cannot be observed.
AWS: recorded behavior only (data plane). On a WEBSOCKET API, a frame whose selection key matches no declared route is dispatched to the $default route's integration, and that integration's reply is delivered back over the socket.
Emulator: the check polls the deployed endpoint and never gets a servable answer, so it runs past the suite's 60 s cap and is reported as `Check timed out after 60.0s`; no frame is ever dispatched to $default.
Why: a client that relies on $default as its catch-all handler gets no evidence it is wired, and the emulator's silence is indistinguishable from a correct dispatch that produced no reply.
Fix: the execute-api WebSocket listener and its route-dispatch fallback to $default (the same root data-plane absence reported for service apigatewayv2 in this sweep). Related open issue 20261023T223918Z-apigatewayv2-lambda-all-three-websocket-authorizer-routing-checks.md covers three sibling checks but not this one.
Traces: apigatewayv2__lambda.websocket-unmatched-route-goes-to-default-route.

## Evidence
Expected: a frame with an undeclared action is handled by the $default route's Lambda and its reply arrives over the socket. Actual: `Error: Check timed out after 60.0s` — the deployed WEBSOCKET stage never served the frame.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi(ProtocolType=WEBSOCKET, RouteSelectionExpression='$request.body.action'), declare $connect and $default over an AWS_PROXY Lambda integration, CreateDeployment, CreateStage.
3. Connect to the announced stage URL and send a frame whose action matches no route — nothing is dispatched.
4. Replay apigatewayv2__lambda.websocket-unmatched-route-goes-to-default-route.
