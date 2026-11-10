Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401 apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake|run#6|AssertionError|assert status == 403, (
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewayv2__lambda: 2 net-new causes — a $connect REQUEST-authorizer Deny answers the handshake 500 instead of 403, and the missing-identity-source and $connect/$disconnect route scenarios reach no verdict in the harness budget (3 checks)

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
TWO net-new root causes on the apigatewayv2__lambda service token, taken from the 3 failures of this replay that the report marks neither ALREADY FILED nor OWNED. They are numbered below and each carries its own failing check ids; BOTH must be closed before this issue is submitted.

=== Cause 1 of 2: a WebSocket $connect REQUEST authorizer returning a Deny policy makes the handshake answer HTTP 500 Internal Server Error instead of refusing it with 403 ===
AWS: the handshake is not a modelled operation (recorded behavior only); the wiring is apigatewayv2 CreateAuthorizer(AuthorizerType=REQUEST, IdentitySource=['route.request.querystring.token']) + CreateRoute('$connect', AuthorizationType=CUSTOM). Recorded: when the REQUEST authorizer runs and returns an IAM policy whose effect is Deny, API Gateway refuses the opening handshake with status line `HTTP/1.1 403 Forbidden`, no `Upgrade: websocket` header and no upgrade — and the $connect integration Lambda does NOT run (observable on a watcher connection). An ALLOWED attempt upgrades with 101 and the $connect Lambda receives the authorizer's principalId. AWS never answers a well-formed handshake 500.
Emulator: the denied handshake is answered `HTTP/1.1 500 Internal Server Error` — the check's first graded assertion fails with `a Deny policy from the $connect authorizer should refuse the handshake with 403; got 500 (['HTTP/1.1 500 Internal Server Error'])`. '500' is the whole divergence: the status class is wrong (server fault instead of a terminal client refusal), so a client cannot tell a rejected connection from a broken gateway and SDK/browser clients retry a decision AWS made final.
Why: authorization is the gate every WebSocket application puts in front of $connect. A 500 is retryable and carries no authorization meaning, so clients retry forever, and an application cannot distinguish 'the authorizer denied you' (403, terminal) from 'the service is unwell'. Nothing behind the gate — the principalId reaching $connect, the proof that $connect did not run for a denied attempt — can be verified.
Fix: the apigatewayv2 WEBSOCKET execute-api front door's $connect authorization seam — invoke the REQUEST authorizer, evaluate its returned policy, and translate a Deny into the handshake refusal (403 Forbidden, no upgrade headers, integration not invoked) instead of letting the path fall through to a 500.
Traces: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake.

=== Cause 2 of 2: the missing-identity-source refusal and the $connect/$disconnect route scenarios never reach a verdict — both checks consume the whole 60 s harness budget ===
AWS: recorded behavior only. (a) A handshake that omits the REQUEST authorizer's identity source (`route.request.querystring.token`) is refused `HTTP/1.1 401 Unauthorized` with no upgrade header, WITHOUT invoking the authorizer or $connect — no Lambda may run for it — while the same API with the token present upgrades and routes normally. (b) $connect and $disconnect each invoke THEIR OWN integration Lambda: opening the socket invokes the $connect function, closing it invokes the $disconnect function, each with the connection's requestContext.
Emulator: both checks are killed by the harness — `Error: Check timed out after 60.0s` — so neither the 401 refusal nor the per-route $connect/$disconnect invocations are ever produced inside the budget. Given Cause 1 (the same authorizer path answering 500) and this replay's other WebSocket findings, the hang sits in the same $connect/session path: no terminal response and no route invocation arrives, so the checks' poll loops run to the cap. AWS always answers a handshake with a terminal status promptly and fires the two lifecycle routes.
Why: a client gets neither an answer nor a connection; a hang is the worst possible shape for an authorization refusal (it cannot be caught, retried or surfaced), and lifecycle-driven application state (registering a connection on $connect, cleaning it up on $disconnect) never runs.
Fix: the same WEBSOCKET front-door/session path as Cause 1 — identity-source presence must be checked before the authorizer runs and answered 401 immediately, and $connect/$disconnect must dispatch to their own route integrations; any request that cannot be served must still get a terminal HTTP status rather than hanging.
Traces: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401, apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas.

## Evidence
Suite: simparity-behavioral-aws-v1. apigatewayv2__lambda 3/6 (50%), 3 failed, none marked ALREADY FILED or OWNED.

Cause 1 — [mismatch] apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake
  File "check_apigatewayv2__lambda_xsvc_apigatewayv2_2_ws_request_authorizer_deny_policy_forbidden_403_handshake.py", line 52, in run -> assert status == 403
  AssertionError: a Deny policy from the $connect authorizer should refuse the handshake with 403; got 500 (['HTTP/1.1 500 Internal Server Error'])
  The check then asserts `"upgrade: websocket" not in head.lower()` and, on a watcher connection, that NO Lambda ran for the denied marker, and that an allowed attempt upgrades 101 with the authorizer's principalId reaching $connect.
  Expected: HTTP/1.1 403 Forbidden, no upgrade header, $connect not invoked. Actual: HTTP/1.1 500 Internal Server Error.

Cause 2 — two checks with no verdict:
  [mismatch] apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401 -> Error: Check timed out after 60.0s. Its first graded assertion is `assert status == 401` on a handshake omitting `route.request.querystring.token`, followed by `"upgrade: websocket" not in head.lower()`, then a watcher assertion that the authorizer ran for the allowed attempt and that NO Lambda ran for the missing-token marker, then a ping round-trip on the admitted socket.
  [mismatch] apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas -> Error: Check timed out after 60.0s. It expects opening the socket to invoke the $connect function and closing it to invoke the $disconnect function.
  Expected: a terminal 401 within the budget / the two lifecycle Lambdas invoked. Actual: no verdict inside 60 s.

## Steps to Reproduce
1. CreateApi(ProtocolType='WEBSOCKET'); create a REQUEST authorizer (CreateAuthorizer AuthorizerType=REQUEST, IdentitySource=['route.request.querystring.token']) whose Lambda returns a Deny policy for token=deny and an Allow with a principalId otherwise; create $connect (AuthorizationType=CUSTOM), $disconnect and a 'ping' route, each with its OWN AWS_PROXY Lambda; grant lambda:InvokeFunction to apigateway.amazonaws.com; deploy and create a stage.
2. Open a handshake with ?token=deny. Observe `HTTP/1.1 500 Internal Server Error` where AWS answers `HTTP/1.1 403 Forbidden` with no upgrade header and without invoking $connect.
3. Open a handshake with the token query string absent. AWS answers `HTTP/1.1 401 Unauthorized` without invoking the authorizer or $connect; the emulator produces no verdict and the check is killed at 60 s.
4. Open and then close an authorized socket and observe whether the $connect and $disconnect Lambdas are each invoked; the check runs to the 60 s cap instead.
5. After fixing, re-replay the 3 ids in trace_ids.
