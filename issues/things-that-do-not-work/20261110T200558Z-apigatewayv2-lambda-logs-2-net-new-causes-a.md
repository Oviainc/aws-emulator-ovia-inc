Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies apigatewayv2__lambda__logs.unmatched-action-without-a-default-route-is-rejected-forbidden apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint apigatewayv2__lambda__logs.heartbeat-is-acknowledged-and-advances-the-node-last-seen apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui apigatewayv2__lambda__logs.registration-reply-carries-the-node-id-stored-for-the-connection apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url apigatewayv2__lambda__logs.registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset apigatewayv2__lambda__logs.reply-is-denied-without-the-manage-connections-grant apigatewayv2__lambda__logs.unrouted-ping-action-is-swallowed-while-heartbeat-is-answered
Issue-Key: apigatewayv2__lambda__logs.default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies|run#9|ConnectionClosedError|mgmt.post_to_connection(ConnectionId=cid, Data=b"control-frame")
Issue-Key: apigatewayv2__lambda__logs.unmatched-action-without-a-default-route-is-rejected-forbidden|run#9|ConnectionClosedError|live = ctx.client("apigatewaymanagementapi", endpoint_url=ctx.stage_url(est["api_endpoint"], _STAGE, management=True)) .get_connection(ConnectionId=cid)
Issue-Key: apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint|_log_messages#7|Timeout|time.sleep(4)
Issue-Key: apigatewayv2__lambda__logs.heartbeat-is-acknowledged-and-advances-the-node-last-seen|_exact#3|Timeout|return None
Issue-Key: apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui|_exact#5|Timeout|return None
Issue-Key: apigatewayv2__lambda__logs.registration-reply-carries-the-node-id-stored-for-the-connection|_exact#3|Timeout|return None
Issue-Key: apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url|_exact#5|Timeout|return None
Issue-Key: apigatewayv2__lambda__logs.registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset|_exact#5|Timeout|return None
Issue-Key: apigatewayv2__lambda__logs.reply-is-denied-without-the-manage-connections-grant|_log_messages#5|Timeout|time.sleep(4)
Issue-Key: apigatewayv2__lambda__logs.unrouted-ping-action-is-swallowed-while-heartbeat-is-answered|_exact#3|Timeout|return None
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewayv2__lambda__logs: 2 net-new causes — a @connections PostToConnection on a deployed stage is answered by closing the connection with no HTTP response, and eight WebSocket session checks reach no verdict

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
Two distinct causes on a deployed WEBSOCKET stage.

Cause 1 of 2: PostToConnection against the stage's @connections endpoint closes the TCP connection without sending any HTTP response.
  AWS: Smithy (apigatewaymanagementapi): PostToConnection POST /{stage}/@connections/{ConnectionId}, ConnectionId @httpLabel @required, Data @httpPayload @length min 0 max 131072; success code 200 with an empty body; modeled errors ForbiddenException/403, GoneException/410, LimitExceededException/429, PayloadTooLargeException/413. Every outcome is an HTTP response with a status and, for a refusal, the error code in the body — AWS never closes the socket without answering.
  Emulator: botocore reports ConnectionClosedError — 'Connection was closed before we received a valid response from endpoint URL: "https://<host>/<stage>/@connections/<url-encoded connection id>"'. The host/port and the connection id are per-run values and are not part of the requirement; the defect is that no HTTP status is sent at all.
  Why: a client sees a transport failure instead of a status. botocore classifies a closed connection as retryable and retries a call that may already have had an effect, and the caller can never see the 410 GoneException / 403 ForbiddenException it must branch on — so post-disconnect and unauthorized pushes are indistinguishable from a network fault.
  Fix: the apigatewaymanagementapi @connections request handler reached through the apigatewayv2 execute-api front door — it must always serialize an HTTP response (200 empty, or a modeled 403/410/413/429) instead of dropping the connection. Related open issues 20261110T175447Z-apigatewaymanagementapi-2-net-new-causes-a-connections-call-d8ba2e.md and 20261109T160349Z-apigatewaymanagementapi-2-net-new-causes-a-connections-call.md describe this shape for other stage states and do not carry these checks' replay keys.
  Traces: apigatewayv2__lambda__logs.default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies, apigatewayv2__lambda__logs.unmatched-action-without-a-default-route-is-rejected-forbidden.

Cause 2 of 2: eight WebSocket session/agent-registration checks reach no verdict — each hangs to the 60 s cap.
  AWS: recorded behavior only — in the recordings each leg answers or is refused within seconds: a registration frame is answered with a reply pushed back over @connections (or, with no WebSocket endpoint configured, the write succeeds and demonstrably no reply arrives), a heartbeat is acknowledged and advances the node's last-seen value, a stale connection is pruned on GoneException while the live connection is still notified, a reply attempted without the execute-api:ManageConnections grant is refused, and an unrouted action is dropped while a routed one is answered.
  Emulator: all eight end 'Check timed out after 60.0s'. No frame, no reply, no refusal.
  Why: a client gets neither a response nor an error — the operation is indefinitely outstanding, so timeouts/retries fire blindly and the negative contracts (no reply when the endpoint is unset, denial without the grant, pruning on Gone) are unobservable.
  Fix: the apigatewayv2 WEBSOCKET execute-api data plane that these scenarios drive — route dispatch and the @connections push path must settle each frame promptly (answer, push, or refuse). Related open issue 20261110T153046Z-apigatewayv2-lambda-logs-the-websocket-session-scenario-and.md describes a similar hang and does not carry these checks' replay keys.
  Traces: the eight timed-out checks in trace_ids.

## Evidence
Cause 1 — expected: PostToConnection answers HTTP 200 with an empty body, or a modeled refusal (403 ForbiddenException / 410 GoneException / 413 PayloadTooLargeException / 429 LimitExceededException).
actual, both checks: botocore.exceptions.ConnectionClosedError: Connection was closed before we received a valid response from endpoint URL: "https://<host>/<stage>/@connections/<id>" — no status line is ever sent.
Smithy: PostToConnection POST /@connections/{ConnectionId}, success code 200, Data @length 0..131072, errors 403/410/429/413.

Cause 2 — expected: each registration/heartbeat/prune/denial/drop leg answers or is refused in seconds.
actual, eight checks: Error: Check timed out after 60.0s (agent-registration-reply-is-never-delivered-without-websocket-endpoint, heartbeat-is-acknowledged-and-advances-the-node-last-seen, registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui, registration-reply-carries-the-node-id-stored-for-the-connection, registration-reply-is-delivered-via-the-https-callback-url, registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset, reply-is-denied-without-the-manage-connections-grant, unrouted-ping-action-is-swallowed-while-heartbeat-is-answered).

## Steps to Reproduce
1. Deploy a WEBSOCKET API with $connect/$default/named Lambda integrations that log to CloudWatch Logs, plus a stage and a deployment.
2. Call apigatewaymanagementapi PostToConnection against https://<host>/<stage>/@connections/<connectionId>: the connection is closed with no HTTP response (Cause 1).
3. Run the agent-registration/heartbeat/prune/denial/unrouted-action scenarios over an upgraded socket: each hangs to the 60 s cap (Cause 2).
4. Replay the checks in trace_ids.
