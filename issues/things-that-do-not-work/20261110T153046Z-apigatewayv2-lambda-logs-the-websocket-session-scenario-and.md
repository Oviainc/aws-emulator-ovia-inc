Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint apigatewayv2__lambda__logs.connect-without-an-invoke-permission-is-rejected-with-500 apigatewayv2__lambda__logs.default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies apigatewayv2__lambda__logs.disconnect-route-deletes-the-connection-row-and-marks-the-node-disconnected apigatewayv2__lambda__logs.heartbeat-is-acknowledged-and-advances-the-node-last-seen apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui apigatewayv2__lambda__logs.registration-reply-carries-the-node-id-stored-for-the-connection apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url apigatewayv2__lambda__logs.registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset apigatewayv2__lambda__logs.reply-is-denied-without-the-manage-connections-grant apigatewayv2__lambda__logs.unmatched-action-without-a-default-route-is-rejected-forbidden apigatewayv2__lambda__logs.unrouted-ping-action-is-swallowed-while-heartbeat-is-answered
Issue-Key: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id|_poll_until#29|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint|_ws_connect#3|Timeout|status, ws, head = _ws_open(url, ctx.http_verify)
Issue-Key: apigatewayv2__lambda__logs.connect-without-an-invoke-permission-is-rejected-with-500|_ws_connect#7|Timeout|status, ws, head = _ws_open(url)
Issue-Key: apigatewayv2__lambda__logs.default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies|_ws_connect#4|Timeout|status, ws, head = _ws_open(url)
Issue-Key: apigatewayv2__lambda__logs.disconnect-route-deletes-the-connection-row-and-marks-the-node-disconnected|_ws_connect#2|Timeout|status, ws, head = _ws_open(url)
Issue-Key: apigatewayv2__lambda__logs.heartbeat-is-acknowledged-and-advances-the-node-last-seen|_ws_connect#2|Timeout|status, ws, head = _ws_open(url)
Issue-Key: apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui|_ws_connect#4|Timeout|status, ws, head = _ws_open(url)
Issue-Key: apigatewayv2__lambda__logs.registration-reply-carries-the-node-id-stored-for-the-connection|_ws_connect#2|Timeout|status, ws, head = _ws_open(url)
Issue-Key: apigatewayv2__lambda__logs.registration-reply-is-delivered-via-the-https-callback-url|_ws_connect#4|Timeout|status, ws, head = _ws_open(url)
Issue-Key: apigatewayv2__lambda__logs.registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset|_ws_connect#3|Timeout|status, ws, head = _ws_open(url)
Issue-Key: apigatewayv2__lambda__logs.reply-is-denied-without-the-manage-connections-grant|_ws_connect#3|Timeout|status, ws, head = _ws_open(url)
Issue-Key: apigatewayv2__lambda__logs.unmatched-action-without-a-default-route-is-rejected-forbidden|_ws_connect#2|Timeout|status, ws, head = _ws_open(url)
Issue-Key: apigatewayv2__lambda__logs.unrouted-ping-action-is-swallowed-while-heartbeat-is-answered|_ws_connect#2|Timeout|status, ws, head = _ws_open(url)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# apigatewayv2__lambda__logs: the WebSocket session scenario and the HTTP-API access-log/request-id correlation reach no verdict — every check hangs to the 60 s cap (2 causes)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two root causes, both leaving the check with no verdict at all (the harness kills it at its 60 s cap). The open issue 20261109T143515Z-apigatewayv2-lambda-logs-apigatewayv2-the-api-endpoint-is-published-as-an.md describes the published-ApiEndpoint and PostToConnection-close defects for two of these check ids, but it does not carry the replay key of the thirteen failures below (their signature is now a hang, not PublishedUrlRejected / ConnectionClosedError), so it does not cover them.

Cause 1 of 2: a deployed WEBSOCKET stage never carries a client session to completion, so none of the twelve agent-session checks can grade anything.
AWS: a client opens an RFC 6455 connection to the stage's wss endpoint; $connect, the route-selection-expression routes ($default, register, register_ui, agent_connected, heartbeat), $disconnect and ApiGatewayManagementApi PostToConnection/@connections all answer within seconds — a handshake is answered 101 Switching Protocols, a routed action is delivered to the integration, an unrouted action with no $default route is dropped, a PostToConnection to a gone connection answers 410 GoneException and one without the execute-api:ManageConnections grant answers 403 ForbiddenException. Recorded behavior only for the scenario's own assertions (connection-row writes, reply delivery, node last-seen).
Emulator: every one of these twelve checks produces no response of any kind within the cap — the run ends 'Error: Check timed out after 60.0s' with no AWS call having returned the awaited state. Nothing is refused and nothing is served; the session simply never progresses.
Why: a client cannot open, route, reply on or close a WebSocket session, and cannot observe a refusal either, so no WebSocket-backed application can be exercised against the emulator and no error-handling path around connections can be written.
Fix: the apigatewayv2 execute-api WEBSOCKET data plane (handshake/upgrade, route-selection dispatch to the integration, $disconnect) and the ApiGatewayManagementApi @connections handler it is reached through.
Traces: apigatewayv2__lambda__logs.agent-registration-reply-is-never-delivered-without-websocket-endpoint, .connect-without-an-invoke-permission-is-rejected-with-500, .default-route-response-is-silent-until-redeployed-then-frames-the-handler-error-bodies, .disconnect-route-deletes-the-connection-row-and-marks-the-node-disconnected, .heartbeat-is-acknowledged-and-advances-the-node-last-seen, .registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui, .registration-reply-carries-the-node-id-stored-for-the-connection, .registration-reply-is-delivered-via-the-https-callback-url, .registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset, .reply-is-denied-without-the-manage-connections-grant, .unmatched-action-without-a-default-route-is-rejected-forbidden, .unrouted-ping-action-is-swallowed-while-heartbeat-is-answered.

Cause 2 of 2: an HTTP API request and its access-log record never join, so the request-id correlation check never completes.
AWS: a request to the stage is answered by the integration and the stage's access-log record for that request is delivered to the configured log group within seconds, carrying the same gateway request id that the response's apigw-requestid header reports; FilterLogEvents then returns it.
Emulator: the check reaches no verdict inside the cap — 'Error: Check timed out after 60.0s'. Either the request never completes or the access-log record is never delivered, so the correlation is unobservable.
Why: a client cannot correlate a response to its access-log record, which is the only way to attribute latency, status and identity for a request.
Fix: the apigatewayv2 HTTP-API execute-api request path and the stage access-log emitter that writes to CloudWatch Logs.
Traces: apigatewayv2__lambda__logs.access-log-record-joins-the-gateway-request-id.

## Evidence
All thirteen checks end with the same detail and nothing else: 'Error: Check timed out after 60.0s'. Expected: each AWS call in the scenario returns (a 101 handshake, a routed frame delivered to the integration, a 410/403 refusal from PostToConnection, an access-log record retrievable via FilterLogEvents) within seconds, so the check reaches a pass/fail verdict. Actual: no awaited state ever materialises and the harness kills the check at 60 s. The 60 s figure is the harness cap, not a requirement; the divergence is that the operations never complete.

## Steps to Reproduce
Replay the apigatewayv2__lambda__logs checks listed in trace_ids. Cause 1: create a WEBSOCKET API with a route-selection expression and $connect/$default/register/heartbeat/$disconnect routes on an AWS_PROXY Lambda integration, deploy a stage, open an RFC 6455 connection to the stage endpoint and send a routed action; observe no 101 and no frame delivery. Cause 2: create an HTTP API stage with AccessLogSettings, issue a request, then FilterLogEvents on the configured log group for the gateway request id.
