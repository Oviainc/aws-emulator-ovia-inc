Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake apigatewayv2__lambda.ws-route-model-validation-rejects-invalid-body-serves-valid apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake|run#6|AssertionError|assert status == 403, (
Issue-Key: apigatewayv2__lambda.ws-route-model-validation-rejects-invalid-body-serves-valid|run#22|AssertionError|assert err1.get("message") == "Invalid request body", (
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewayv2__lambda: 3 net-new causes on WEBSOCKET routes — a $connect authorizer Deny answers 500 instead of 403, route request-model validation is not enforced, and two per-route scenarios never answer

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoute

## Description
Three distinct causes, all on a deployed WEBSOCKET stage whose handshake now upgrades.

Cause 1 of 3: a REQUEST authorizer returning a Deny policy on $connect is answered 500 instead of refusing the handshake 403.
  AWS: recorded behavior only (the execute-api data plane is not modeled). A Lambda REQUEST authorizer attached to $connect that returns an IAM policy with Effect=Deny makes AWS refuse the handshake with HTTP 403 Forbidden — a client-side refusal, with no 101 and no integration invocation. The 403 status line is the requirement; no variable part.
  Emulator: the handshake is answered 'HTTP/1.1 500 Internal Server Error'. A deny decision is reported as a server fault.
  Why: a client cannot distinguish 'you are not allowed to connect' from 'the gateway broke'. SDKs and browsers retry 5xx but not 403, so a denied client retries a decision that will never change, and authorization regressions look like outages.
  Fix: the apigatewayv2 WEBSOCKET $connect authorizer evaluation path — a Deny policy must map to a 403 handshake refusal rather than falling through to the generic 500.
  Traces: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake.

Cause 2 of 3: a route's RequestModels schema is never enforced — a body violating the model is forwarded to the integration.
  AWS: Smithy: CreateRoute carries RequestModels (jsonName requestModels, map of model name by selection key) and ModelSelectionExpression; CreateApi/CreateModel define the JSON schema. When a frame's body fails the selected model's schema, AWS refuses the frame at the gateway and delivers the documented frame whose message member is exactly 'Invalid request body'; the integration is never invoked.
  Emulator: the body is passed straight through — the integration reports {'routeKey': 'sendmsg', 'connectionId': '<id>', 'body': '{"action": "sendmsg", "text": 12345}'} for a body whose `text` violates the model's string type. No refusal frame is produced. The fixed text of the expected refusal is the message value 'Invalid request body'; the connectionId/requestId values are per-run and are not part of the requirement.
  Why: a client that relies on the gateway to validate request bodies has no validation at all: malformed input reaches the backend, which must then defend itself, and the client receives backend behaviour instead of AWS's uniform 'Invalid request body' refusal.
  Fix: the apigatewayv2 WEBSOCKET route-dispatch request validation — resolve the route's RequestModels via ModelSelectionExpression, validate the frame body against the model, and emit the gateway refusal frame before invoking the integration.
  Traces: apigatewayv2__lambda.ws-route-model-validation-rejects-invalid-body-serves-valid.

Cause 3 of 3: two per-route WEBSOCKET scenarios reach no verdict at all.
  AWS: recorded behavior only — $connect and $disconnect each invoke their own integration and the lifecycle completes in seconds; a REQUEST authorizer whose configured identity source is absent from the handshake short-circuits with HTTP 401 Unauthorized without invoking the authorizer.
  Emulator: both checks end 'Check timed out after 60.0s'. Nothing is answered and nothing is refused.
  Why: a client gets neither the 401 refusal nor the connect/disconnect lifecycle — only a hang, which no error handling can act on.
  Fix: the apigatewayv2 WEBSOCKET execute-api data plane — $connect/$disconnect dispatch and the identity-source pre-check must answer or refuse promptly.
  Traces: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401.

Related open issues 20261107T193731Z-apigatewayv2-lambda-3-net-new-root-causes-the.md and 20261109T154749Z-apigatewayv2-lambda-2-net-new-causes-the-http.md describe earlier shapes of this area and do not carry these checks' replay keys.

## Evidence
Cause 1 — expected: 403 handshake refusal on an authorizer Deny.
actual: AssertionError: a Deny policy from the $connect authorizer should refuse the handshake with 403; got 500 (['HTTP/1.1 500 Internal Server Error']).

Cause 2 — expected: the gateway frame whose message is 'Invalid request body' for a body failing the route model's type constraint.
actual: AssertionError: ... sent '{"action": "sendmsg", "text": 12345}', got {'routeKey': 'sendmsg', 'connectionId': '<id>', 'body': '{"action": "sendmsg", "text": 12345}'} — the integration was invoked with the invalid body.
Smithy: CreateRoute.RequestModels (jsonName requestModels) + ModelSelectionExpression exist and are accepted on create.

Cause 3 — expected: $connect/$disconnect each fire their own Lambda; a missing identity source short-circuits 401.
actual: both checks: Error: Check timed out after 60.0s.

## Steps to Reproduce
1. Create a WEBSOCKET API with $connect/$disconnect/named Lambda integrations, a REQUEST authorizer on $connect, a model-bound route (RequestModels + ModelSelectionExpression), a stage and a deployment.
2. Upgrade with the authorizer returning Deny — the handshake answers 500 instead of 403 (Cause 1).
3. Send a frame whose body violates the route model — the integration is invoked instead of the gateway refusing with 'Invalid request body' (Cause 2).
4. Upgrade/close to exercise $connect/$disconnect, and upgrade omitting the authorizer's identity source — both hang to the 60 s cap (Cause 3).
5. Replay the checks in trace_ids.
