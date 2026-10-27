Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake|_ws_handshake#4|Timeout|chunk = sock.recv(4096)
Issue-Commit: f2b5fdf3236bf81a246aedac6a795483308b42ed

# apigatewayv2__dynamodb__lambda: the $connect-denial WebSocket leg never converges — the deployed WEBSOCKET stage does not produce a 403-refused handshake carrying the integration's body

> _Note: this issue was identified against the code base at commit `f2b5fdf3236bf81a246aedac6a795483308b42ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
$connect (WebSocket data plane)

## Description
One root cause.

Cause 1 of 1: the WebSocket `$connect` authorization path of a deployed apigatewayv2 WEBSOCKET stage never converges, so the check that grades a `$connect` integration refusing the handshake blocks and is cut by the harness cap. This check is listed in the trace ids of open issue 20261023T223918Z-apigatewayv2-dynamodb-lambda-all-three-websocket-duplex-checks.md, but that issue's recorded replay key for it (`...|_poll_until#4|Timeout|time.sleep(interval_s)`) no longer matches this replay — the suite marks the other two checks in that bundle as OWNED and leaves this one unowned, i.e. it now blocks at a different poll site than when that issue was written, so its current failure is not carried by any open issue.

AWS: recorded/documented behavior only — the opening handshake and the `$connect` authorization decision are the execute-api data plane, not a modeled operation. Smithy (apigatewayv2 2018-11-29): `ProtocolType` enum values [WEBSOCKET, HTTP]; `Api.RouteSelectionExpression` is required and selects the route for each frame. On real AWS: the opening handshake against a deployed WEBSOCKET stage runs the `$connect` route's integration BEFORE completing the upgrade; the integration receives requestContext.connectionId/domainName/stage plus queryStringParameters. When that integration answers a non-2xx statusCode, API Gateway refuses the upgrade: the HTTP response line carries that status (403 here), it carries NO `Sec-WebSocket-Accept` header, the response body is the integration's own body (the fixed text here is the integration's literal `denied`; the status code and body are values the integration returns, not constants of the service), and the socket is closed. The refusal is connection-scoped: side effects the integration performed before refusing persist (the item it wrote under `denied#<marker>`, carrying the connection id the service minted), no admitted connection is recorded, a frame written into the refused socket never reaches `$default`, and sockets admitted before and after the refusal still round-trip.

Emulator: none of that is observable. The check reports `Check timed out after 60.0s` — some leg of the sequence (handshake upgrade, the `$connect` integration's DynamoDB write, the 403 refusal with the `denied` body, or the post-refusal round trip) never completes, so the service never produces an observable `$connect` denial at all. The sibling checks in this group fail the same way, and the corroborating status line this emulator returns for such an upgrade, recorded elsewhere in this suite, is `HTTP/1.1 404 Not Found` rather than either 101 or the integration's 403.

Why: a client relying on AWS behavior has no way to implement or test connection admission control. `$connect` authorization is the only hook for refusing a WebSocket client; with it unreachable, an application either cannot deploy its authorization at all or — worse, if the upgrade were later completed without honouring the integration status — would be silently over-permissive, admitting every client the integration meant to deny. Instead of a wrong answer, callers get an indefinite wait or a 404 on wss://.

Fix: the execute-api data-plane front door / WebSocket session layer of apigatewayv2 — recognise the RFC 6455 upgrade on a deployed WEBSOCKET API's stage path, mint a connection id, invoke the `$connect` route integration synchronously, and gate the upgrade on its response: on a non-2xx statusCode answer the handshake with that status, omit `Sec-WebSocket-Accept`, write the integration's body, close the socket, record no admitted connection, and refuse to route any subsequent frame on it to `$default` — while leaving other connections on the same API unaffected. This is a capability gap, not an error-shape bug.

Traces: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake.

## Evidence
Suite: simparity-behavioral-ga-prod-v1. Service group apigatewayv2__dynamodb__lambda 0/6 (0%).

Expected (real AWS), for the deny leg of this check:
  - handshake response line status == 403 (the statusCode the $connect integration returned);
  - the response head contains no `sec-websocket-accept` header;
  - the response body == b'denied' (the integration's own body);
  - a DynamoDB item exists under pk = 'denied#<marker>' with a non-empty connectionId, proving the integration ran and made the decision;
  - no item ever appears under pk = 'conn#<marker>' (not admitted) and none under pk = 'msg#<sha256(refused_marker)>' (a frame pushed into the refused socket never reached $default);
  - the socket admitted before the refusal still completes a duplex round trip, and a fresh un-denied handshake still upgrades.

Actual (emulator):
  [mismatch] apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake
  Error: Check timed out after 60.0s
None of the above assertions is reached; the check blocks in one of its poll loops until the 60 s harness cap. Corroborating status line this emulator returns for a WebSocket upgrade on a deployed stage, observed in the same suite for the same stack shape: `b'HTTP/1.1 404 Not Found'`.

Ownership note: open issue 20261023T223918Z-apigatewayv2-dynamodb-lambda-all-three-websocket-duplex-checks.md lists this check id in Issue-Trace-Ids with Issue-Key `apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake|_poll_until#4|Timeout|time.sleep(interval_s)`. In this replay the suite marks the other two checks of that bundle OWNED and leaves this one unowned, so the key has moved and this failure is unclaimed.

Model facts used: smithy_query shape ProtocolType (com.amazonaws.apigatewayv2#ProtocolType) -> values [WEBSOCKET, HTTP].

## Steps to Reproduce
1. Boot the emulator.
2. Create a DynamoDB table and a Lambda whose handler, on routeKey '$connect', reads queryStringParameters and (a) when deny == '1' writes an item {pk: 'denied#<marker>', connectionId: <requestContext.connectionId>} and returns {'statusCode': 403, 'body': 'denied'}, (b) otherwise writes {pk: 'conn#<marker>', connectionId: ...} and returns {'statusCode': 200}; on '$default' it records the frame digest and pushes it back via apigatewaymanagementapi PostToConnection.
3. create_api(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'); create an AWS_PROXY integration to the Lambda; create routes $connect, $disconnect and $default; lambda add_permission; create_deployment; create_stage.
4. Open an RFC 6455 handshake to the stage URL with query '?marker=<m1>' and confirm the 101 upgrade, the conn#<m1> row, and a duplex round trip.
5. Open a second handshake to the same stage with query '?marker=<m2>&deny=1'.
6. Observe that the emulator never answers that handshake with HTTP 403 + body b'denied' and no Sec-WebSocket-Accept, never writes denied#<m2>, and the check hangs to the 60 s cap instead.
7. Replay apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake.
