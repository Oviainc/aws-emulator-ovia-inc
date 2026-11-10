Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003 apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009 apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch
Issue-Key: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect|run#27|AssertionError|assert json.loads(pushed).get("echo") == text, (
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003|run#10|AssertionError|assert json.loads(pushed).get("sha") == text_sha, (
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake|run#13|AssertionError|assert json.loads(pushed).get("sha") == marker_sha, (
Issue-Key: apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009|run#10|AssertionError|assert pushed.get("len") == len(under), (
Issue-Key: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch|_poll_until#31|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewayv2__dynamodb__lambda: 2 net-new causes — every WebSocket route reply is the gateway's "Internal server error" frame instead of the integration's PostToConnection payload, and a route-selection-expression update never redirects dispatch

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
The handshake now upgrades, so these checks fail on post-upgrade behaviour. Two distinct causes.

Cause 1 of 2: the client socket receives the gateway's generic internal-error envelope in place of the integration's pushed payload.
  AWS: Smithy (apigatewaymanagementapi): PostToConnection POST /@connections/{ConnectionId}, Data @httpPayload @length min 0 max 131072, success code 200 (empty body), modeled errors ForbiddenException/403, GoneException/410, LimitExceededException/429, PayloadTooLargeException/413. On a 200 AWS writes exactly the posted Data bytes to that connection's socket and substitutes nothing. Fragmented client frames are reassembled into ONE message before the route integration sees them, so the integration observes the full message length.
  Emulator: whatever the integration posts, the client reads {"connectionId":"<id>","message":"Internal server error","requestId":"<id>"}. The fixed text of that envelope is the member names connectionId/message/requestId and the string 'Internal server error'; the id values are per-run and are not part of the requirement. Because the same envelope is returned for the echo case, the digest case, the admitted-socket case and the reassembly case, the integration's payload (echo body, sha digest, reassembled length) never reaches the client at all.
  Why: a client doing request/response over a WebSocket API gets an error document where its application payload belongs, and the management call still looks successful, so the caller cannot detect the loss. The fragment-reassembly contract is equally unobservable: the integration never reports the single 114688-byte message AWS would hand it.
  Fix: the apigatewayv2 WEBSOCKET execute-api data plane — the route-dispatch/@connections push path must frame the integration's bytes to the client verbatim (and reassemble continuation frames before dispatch) instead of substituting the gateway error envelope.
  Traces: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect, apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003, apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake, apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009.

Cause 2 of 2: after UpdateApi changes RouteSelectionExpression and the stage is redeployed, dispatch never follows the new expression.
  AWS: recorded behavior only — once a new deployment is associated with the stage, the stage selects routes by the api's current RouteSelectionExpression (here $request.body.meta.kind) and a frame matching the new key reaches that route's integration promptly.
  Emulator: the scenario never converges — 'the redeployed stage selecting routes by $request.body.meta.kind: the run budget ended first; not graded'. No frame is ever dispatched under the new expression.
  Why: a client that re-keys its protocol by updating RouteSelectionExpression and redeploying sees its traffic silently stop being routed, with no refusal to diagnose.
  Fix: the apigatewayv2 WEBSOCKET route-selection/stage-deployment snapshot path — a redeployed stage must pick up the api's updated RouteSelectionExpression for subsequent frames.
  Traces: apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch.

Related open issues (20261110T155738Z-apigatewayv2-dynamodb-lambda-websocket-route-dispatch-never-completes.md, 20261109T154749Z-apigatewayv2-dynamodb-lambda-3-net-new-causes-the.md) describe the earlier no-upgrade shape and do not carry these checks' replay keys.

## Evidence
Cause 1 expected: PostToConnection delivers the posted Data verbatim on the client socket.
actual (websocket-api-connect-message-post-back-disconnect): expected {'echo': 'ws-payload-<id>'}, got '{"connectionId":"<id>","message":"Internal server error","requestId":"<id>"}'.
actual (wsduplex-client-binary-frame-rejected-close-1003): expected the digest frame, got the same 'Internal server error' envelope.
actual (wsduplex-connect-denial-rejects-handshake): the admitted socket got the same envelope instead of the digest push.
actual (wsduplex-oversize-client-message-rejected-close-1009): $default should have seen 7 fragments as ONE 114688-byte message; the integration reported the same envelope.
Smithy: PostToConnection 200/empty body; Data @length 0..131072; errors 403/410/429/413.

Cause 2 expected: after UpdateApi(RouteSelectionExpression=$request.body.meta.kind) + CreateDeployment, a frame with meta.kind reaches the new route.
actual (deep-apigatewayv2-1-wsroute-selection-expression-update-redirects-dispatch): TimeoutError: the redeployed stage selecting routes by $request.body.meta.kind: the run budget ended first; not graded.

## Steps to Reproduce
1. Create a WEBSOCKET API with $connect/$default/named Lambda integrations writing to DynamoDB, a stage and a deployment.
2. Upgrade a client socket, send a text frame, and have the integration call PostToConnection with its payload: the client reads the gateway 'Internal server error' envelope instead (Cause 1). Repeat with 7 continuation frames totalling 114688 bytes: the integration never reports one reassembled message.
3. UpdateApi with a new RouteSelectionExpression, CreateDeployment, then send a frame keyed by the new expression: it is never dispatched (Cause 2).
4. Replay the checks in trace_ids.
