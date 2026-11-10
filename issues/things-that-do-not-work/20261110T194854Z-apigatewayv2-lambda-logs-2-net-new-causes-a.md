Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies apigatewayv2__lambda__logs.unmatched-action-without-a-default-route-is-rejected-forbidden apigatewayv2__lambda__logs.registration-reply-carries-the-node-id-stored-for-the-connection apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui
Issue-Key: apigatewayv2__lambda__logs.default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies|run#9|ConnectionClosedError|mgmt.post_to_connection(ConnectionId=cid, Data=b"control-frame")
Issue-Key: apigatewayv2__lambda__logs.unmatched-action-without-a-default-route-is-rejected-forbidden|run#9|ConnectionClosedError|live = ctx.client("apigatewaymanagementapi", endpoint_url=ctx.stage_url(est["api_endpoint"], _STAGE, management=True)) .get_connection(ConnectionId=cid)
Issue-Key: apigatewayv2__lambda__logs.registration-reply-carries-the-node-id-stored-for-the-connection|_exact#3|Timeout|return None
Issue-Key: apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui|_exact#5|Timeout|return None
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewayv2__lambda__logs: 2 net-new causes — a @connections call on a deployed WebSocket stage is answered by closing the connection with no HTTP response, and the two registration scenarios reach no verdict (4 checks)

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
TWO net-new root causes on the apigatewayv2__lambda__logs service token, taken from the 4 failures of this replay that the report marks neither ALREADY FILED nor OWNED (the fifth, post-to-a-malformed-connection-id-is-rejected-bad-request, is OWNED). They are numbered below and each carries its own failing check ids; BOTH must be closed before this issue is submitted.

=== Cause 1 of 2: a @connections request against the deployed WebSocket stage produces NO HTTP response — the connection is aborted before any status line ===
AWS: Smithy com.amazonaws.apigatewaymanagementapi#ApiGatewayManagementApi (restJson1, sigv4): PostToConnection POST /@connections/{ConnectionId} -> 200 with Data @httpPayload blob @length{min 0,max 131072} and an empty body; GetConnection GET /@connections/{ConnectionId} -> 200 {connectedAt, lastActiveAt, identity}; DeleteConnection DELETE -> 204; all declare ForbiddenException (403), GoneException (410, memberless) and LimitExceededException (429), PostToConnection also PayloadTooLargeException (413). Recorded: a live id is served 200 and the payload is delivered as a TEXT frame; an id no live socket owns is HTTP 410 with error code GoneException and an EMPTY message. Whatever the verdict, AWS always ANSWERS with a complete HTTP response.
Emulator: botocore raises `ConnectionClosedError: Connection was closed before we received a valid response from endpoint URL: "https://<host>:<port>/prod/@connections/<connection-id>"` — no status line, no headers, no body — for the id the stage itself minted ('EzCyjTUad5uklZ0=' this run, the same value the emulator echoes inside WebSocket frames elsewhere in this replay). Host, port and id are per-run; the requirement is that an HTTP response arrives at all.
Why: the SDK sees a retryable transport reset instead of a modelled result, so a backend pushing to its clients can neither confirm a delivery (200) nor prune a dead id (410 GoneException); the two checks that depend on pushing a frame to a known connection cannot get past their first management call, and the whole server-push surface is unusable even on its negative paths.
Fix: the @connections data-plane front door that resolves {api-id, stage} from the request path and dispatches into the connection store, plus the three handlers' response/error serialization — always emit a complete HTTP response (200/204 for a live id, 410 GoneException with an empty message once it is gone) instead of terminating the connection.
Related, does NOT cover these two checks: 20261109T143515Z-apigatewayv2-lambda-logs-apigatewayv2-the-api-endpoint-is-published-as-an.md records the published execute-api host plus the malformed-connection-id variant of this behavior and owns only that check's replay key; the replay reports no owner for the two listed here.
Traces: apigatewayv2__lambda__logs.default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies, apigatewayv2__lambda__logs.unmatched-action-without-a-default-route-is-rejected-forbidden.

=== Cause 2 of 2: the two connection-registration scenarios reach no verdict — both checks consume the whole 60 s harness budget ===
AWS: recorded behavior only. A client frame selecting a registration route is dispatched to its AWS_PROXY Lambda, which stores the connection (with its node id) and replies on the socket; the reply carries the node id stored for that connection. When a registered connection has gone away, the backend's PostToConnection to it is refused 410 GoneException (empty message), the handler prunes that id from its store, and the still-live UI connection is notified — all within the request's lifetime.
Emulator: both checks are killed by the harness — `Error: Check timed out after 60.0s` — so neither the registration reply nor the prune-on-gone fan-out is ever produced. No reply frame and no refusal arrives; the scenarios' poll loops run to the cap. AWS answers promptly in both cases.
Why: a WebSocket application that registers connections and prunes stale ones cannot run at all, and a hang gives client code nothing to branch on — no reply, no 410, no error — so neither the happy path nor the pruning path can be developed against the emulator.
Fix: the apigatewayv2 WEBSOCKET session/route dispatch path that invokes the route integration and relays its reply on the socket, together with the @connections push path from Cause 1 that must answer 410 GoneException for a departed connection so the handler's prune branch can run.
Traces: apigatewayv2__lambda__logs.registration-reply-carries-the-node-id-stored-for-the-connection, apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui.

## Evidence
Suite: simparity-behavioral-aws-v1. apigatewayv2__lambda__logs 0/5 (0%), 5 failed; one is OWNED, the 4 below are not.

Cause 1 — two [error] checks, no HTTP response from the deployed stage's @connections endpoint:
[error] apigatewayv2__lambda__logs.default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies
  File "endpoint.py", line 385, in _send -> File "httpsession.py", line 549, in send
  botocore.exceptions.ConnectionClosedError: Connection was closed before we received a valid response from endpoint URL: "https://127.0.0.1:45273/prod/@connections/EzCyjTUad5uklZ0%3D".
[error] apigatewayv2__lambda__logs.unmatched-action-without-a-default-route-is-rejected-forbidden
  botocore.exceptions.ConnectionClosedError: Connection was closed before we received a valid response from endpoint URL: "https://127.0.0.1:41111/prod/@connections/EzCyjTUad5uklZ0%3D".
  Expected (real AWS): a complete HTTP response — 200 (empty body) for a live id with the payload delivered as a TEXT frame, or 410 with error code GoneException and an empty message once the connection is gone. Actual: the connection is closed before any status line.

Cause 2 — two [mismatch] checks with no verdict:
  apigatewayv2__lambda__logs.registration-reply-carries-the-node-id-stored-for-the-connection -> Error: Check timed out after 60.0s
  apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui -> Error: Check timed out after 60.0s
  Expected: the registration route's reply frame carries the node id stored for that connection; a PostToConnection to a departed connection is refused 410 GoneException (empty message), the handler prunes it and notifies the live UI connection. Actual: nothing within the 60 s budget.

Model facts used (smithy_query query=operation on services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/smithy-model/com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json): PostToConnection POST /@connections/{ConnectionId} code 200, Data @httpPayload blob @length{min 0,max 131072}, errors [ForbiddenException 403, GoneException 410, LimitExceededException 429, PayloadTooLargeException 413]; GetConnection GET code 200 with connectedAt/lastActiveAt/identity; DeleteConnection DELETE code 204. GoneException is memberless, so its message has no modelled variable part.

## Steps to Reproduce
1. CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action') with AWS_PROXY Lambda integrations and a registration route whose handler stores the connection id plus a node id and replies through apigatewaymanagementapi PostToConnection; grant lambda:InvokeFunction to apigateway.amazonaws.com; CreateDeployment + CreateStage('prod'); attach the stage's CloudWatch Logs destination.
2. Open an RFC 6455 handshake to the stage URL (it upgrades) and learn the minted connection id.
3. From a client built on the stage's https endpoint + '/prod', call PostToConnection with that id: botocore raises ConnectionClosedError('Connection was closed before we received a valid response from endpoint URL: "https://<host>:<port>/prod/@connections/<id>"') where AWS answers 200 with an empty body and delivers the payload as a TEXT frame — and 410 GoneException with an empty message once the connection is gone.
4. Send a registration frame and wait for the reply frame carrying the stored node id, then close one connection and push to it so the handler's prune-on-GoneException branch runs and the live UI connection is notified: neither completes within the 60 s budget.
5. After fixing, re-replay the 4 ids in trace_ids.
