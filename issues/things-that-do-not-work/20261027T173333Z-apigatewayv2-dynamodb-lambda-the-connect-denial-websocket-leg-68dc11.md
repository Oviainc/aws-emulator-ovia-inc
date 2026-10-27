Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake|_ws_handshake#4|Timeout|sock = ssl.create_default_context().wrap_socket(sock, server_hostname=http_host)
Issue-Commit: 4d94bbd939fdf81ed7b2b48d25e272629e15a4d9

# apigatewayv2__dynamodb__lambda: the $connect-denial WebSocket leg still never converges — the deployed WEBSOCKET stage does not refuse the handshake with the integration's non-2xx status and body (1 cause)

> _Note: this issue was identified against the code base at commit `4d94bbd939fdf81ed7b2b48d25e272629e15a4d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
$connect (WebSocket data plane upgrade / execute-api)

## Description
REOPENS 20261023T223918Z-apigatewayv2-a-deployed-websocket-stage-never-completes-the.md: its fix left the call refused. That issue is marked fixed and claimed to make a deployed WEBSOCKET stage complete the RFC 6455 upgrade (101 + Sec-WebSocket-Accept) instead of falling through to a generic 404, and to then mint a connection id and dispatch $connect. In this replay the upgrade on a deployed WEBSOCKET stage is still refused, so the one behaviour layered directly on top of $connect dispatch — the integration's authorization decision — remains unobservable.

One root cause.

Cause 1 of 1: the WebSocket $connect authorization path of a deployed apigatewayv2 WEBSOCKET stage never converges, so the check that grades a $connect integration REFUSING the handshake blocks and is cut by the harness cap. Failing check id: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake.

AWS: recorded behavior only — the opening handshake and the $connect authorization decision are the execute-api data plane, not a modeled operation. Smithy (apigatewayv2 2018-11-29) establishes the surface: shape ProtocolType is an enum with values [WEBSOCKET, HTTP]; Api.ProtocolType and Api.RouteSelectionExpression are both required, RouteSelectionExpression selecting the route for each frame; Api.ApiEndpoint doc: "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI to form a complete path to a deployed API stage." On real AWS the opening handshake against a deployed WEBSOCKET stage runs the $connect route's integration BEFORE completing the upgrade, passing it requestContext.connectionId / domainName / stage plus queryStringParameters. When that integration answers a non-2xx statusCode, API Gateway refuses the upgrade: the HTTP response line carries that status, the response head carries NO Sec-WebSocket-Accept header, the response body is the integration's own body, and the socket is closed. The status and the body are VALUES the integration returns (here 403 and the literal text the handler returns), not constants of the service; the requirement is that the service propagates whatever non-2xx status and body the integration returned, and omits the upgrade. The refusal is connection-scoped: side effects the integration performed before refusing persist (the item it wrote keyed by its denial marker, carrying the connection id the service minted), no admitted connection is recorded, a frame written into the refused socket never reaches $default, and sockets admitted before and after the refusal still round-trip.

Emulator: none of that is observable. The check reports `Check timed out after 60.0s` — some leg of the sequence (the handshake upgrade, the $connect integration's DynamoDB write, the non-2xx refusal carrying the integration's body, or the post-refusal round trip) never completes, so the emulator never produces an observable $connect denial at all. The corroborating status line this emulator returns for such an upgrade, observed directly elsewhere in this same suite for the same stack shape, is `HTTP/1.1 404 Not Found` rather than either 101 or the integration's own non-2xx.

Why: a client relying on AWS behavior has no way to implement or test connection admission control. $connect is the only hook for refusing a WebSocket client, so with it unreachable an application either cannot deploy its authorization at all, or — if the upgrade were later completed without honouring the integration's status — would be silently over-permissive, admitting every client the integration meant to deny. Instead of a wrong answer the caller gets an indefinite wait or a 404 on wss://.

Fix: the execute-api data-plane front door / WebSocket session layer of apigatewayv2 — recognise the RFC 6455 upgrade on a deployed WEBSOCKET API's stage path, mint a connection id, invoke the $connect route integration synchronously, and gate the upgrade on its response: on a non-2xx statusCode answer the handshake with that status, omit Sec-WebSocket-Accept, write the integration's body, close the socket, record no admitted connection, and refuse to route any subsequent frame on that socket to $default — while leaving other connections on the same API unaffected. This is a capability gap, not an error-shape bug.

Ownership note (why this is filed and not skipped): the suite marks this check neither ALREADY FILED nor OWNED. Two open issues name this check id but no longer carry its replay key, so neither claims this replay: 20261023T223918Z-apigatewayv2-dynamodb-lambda-all-three-websocket-duplex-checks.md records it with key `...|_poll_until#4|Timeout|time.sleep(interval_s)` (the suite marks the other two checks of that bundle OWNED and leaves this one unowned), and 20261027T160731Z-apigatewayv2-dynamodb-lambda-the-connect-denial-websocket-leg.md records it with key `...|_ws_handshake#4|Timeout|chunk = sock.recv(4096)`. The block site has moved again, so this failure is unclaimed.

Traces: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake.

## Evidence
Suite: simparity-behavioral-ga-prod-v1. Overall 0/20 passed. Service group apigatewayv2__dynamodb__lambda 0/3 (0%).

Expected (real AWS), for the deny leg of this check:
  - the handshake response line carries the non-2xx statusCode the $connect integration returned (403 in this check's handler);
  - the response head contains no `sec-websocket-accept` header, i.e. the protocol upgrade is not completed;
  - the response body is the integration's own body (the text the handler returns);
  - a DynamoDB item exists under the handler's denial key (pk = 'denied#<marker>', where <marker> is the run-unique value from the request query string) carrying a non-empty connectionId — proving the integration ran and made the decision;
  - no item ever appears under the admitted key (pk = 'conn#<marker>'), and none under the refused frame's digest key (pk = 'msg#<sha256 of the frame payload>'), i.e. a frame written into the refused socket never reaches $default;
  - the socket admitted BEFORE the refusal still completes a duplex round trip, and a fresh un-denied handshake still upgrades (the refusal is connection-scoped, not API-scoped).

Actual (emulator):
  [mismatch] apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake
  Error: Check timed out after 60.0s
None of the above assertions is reached — the check blocks in one of its poll loops (`_poll_until(lambda: _try_marked_connect(ctx, api, allow_marker, deny=False), lambda s: s is not None, timeout_s=150.0, interval_s=3.0, description=f"WebSocket handshake against stage {api['stage']}")` is its first) until the 60 s harness cap. Corroborating status line this emulator returns for a WebSocket upgrade on a deployed stage, observed in the same run for the same stack shape: `b'HTTP/1.1 404 Not Found'`.

Control-plane setup preceding the data plane (create_api ProtocolType=WEBSOCKET, create_integration AWS_PROXY, create_route for $connect/$disconnect/$default, lambda add_permission, create_deployment, create_stage) is not reported as failing, so the divergence is in the data plane.

Model facts used: smithy_query shape ProtocolType (com.amazonaws.apigatewayv2#ProtocolType) -> enum values [WEBSOCKET, HTTP]; smithy_query shape Api -> ApiEndpoint doc ({api-id}.execute-api.{region}.amazonaws.com with the stage appended), ProtocolType required, RouteSelectionExpression required.

## Steps to Reproduce
1. Boot the emulator.
2. Create a DynamoDB table and a Lambda whose handler, on requestContext.routeKey == '$connect', reads queryStringParameters and: (a) when deny == '1', writes an item {pk: 'denied#<marker>', connectionId: <requestContext.connectionId>} and returns {'statusCode': 403, 'body': 'denied'}; (b) otherwise writes {pk: 'conn#<marker>', connectionId: ...} and returns {'statusCode': 200}. On '$default' it records the frame's sha256 digest under pk = 'msg#<sha>' and pushes the digest back via apigatewaymanagementapi PostToConnection using requestContext.domainName/stage. On '$disconnect' it writes {pk: 'disc#<connectionId>'}.
3. create_api(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'); create an AWS_PROXY integration to that Lambda; create routes $connect, $disconnect and $default; lambda add_permission; create_deployment; create_stage.
4. Open an RFC 6455 opening handshake to the stage's WebSocket URL with query '?marker=<m1>' and confirm the 101 upgrade, the conn#<m1> row, and one duplex round trip (positive control).
5. Open a second handshake to the same stage with query '?marker=<m2>&deny=1'.
6. Observe that the emulator never answers that handshake with the integration's 403 and body (and no Sec-WebSocket-Accept), never writes denied#<m2>, and the check hangs to the 60 s cap instead.
7. Replay apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake.
