Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009 apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009|_poll_until#2|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration|_poll_until#27|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# API Gateway v2 WebSocket: oversize frame not closed 1009, authorizer deny and unmatched-action routes never answer (3 causes)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeWebSocketRoute

## Description
Three net-new root causes in the apigatewayv2__dynamodb__lambda group. Each needs its own fix. (The open issue 20261023T223918Z-apigatewayv2-dynamodb-lambda-all-three-websocket-duplex-checks.md carries three sibling duplex checks; it does not carry these replay keys.)

Cause 1 of 3: an oversize client message is not rejected with close code 1009.
AWS: a WebSocket frame larger than the API's limit is refused and the connection is closed with status code `1009` (message too big).
Emulator: the recorded close/refusal does not happen as AWS does it.
Why: a client that relies on 1009 to resize and retry sees a different (or no) close reason, so the oversize path is unusable.
Fix: the apigatewayv2 WebSocket frame handler's size gate and close-code emission.
Traces: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009

Cause 2 of 3: a $connect REQUEST authorizer deny does not refuse before the integration.
AWS: when the $connect request authorizer denies, the handshake is refused and the route integration is never invoked.
Emulator: the check never gets an answer — `Check timed out after 60.0s` — so neither the refusal nor the "integration not invoked" guarantee is observable.
Why: an unauthorized connection may reach the backend, and the caller gets no handshake answer either way.
Fix: the apigatewayv2 $connect authorizer evaluation ahead of route dispatch.
Traces: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration

Cause 3 of 3: an unmatched action with no $default route is not dropped observably.
AWS: a message whose route-selection expression matches no route, on an API without a $default route, is not delivered to any integration.
Emulator: the check hangs to the 60s budget, so nothing can be concluded about delivery.
Why: a caller cannot tell a dropped message from a hung connection; a misrouted message might still be delivered.
Fix: the apigatewayv2 route-selection dispatcher's no-match path.
Traces: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered

## Evidence
- wsduplex-oversize-client-message-rejected-close-1009: [mismatch] (expected the connection to close with WebSocket status 1009).
- deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration: [mismatch] Error: Check timed out after 60.0s.
- deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered: [mismatch] Error: Check timed out after 60.0s.

## Steps to Reproduce
Boot the emulator and replay apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009, .deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration and .deep-apigatewayv2-1-wsroute-unmatched-action-without-default-route-not-delivered.
