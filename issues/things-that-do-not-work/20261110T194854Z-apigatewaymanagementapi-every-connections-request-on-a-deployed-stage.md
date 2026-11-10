Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewaymanagementapi.agwmgmt-cross-api-connection-id-rejected apigatewaymanagementapi.agwmgmt-identity-members-and-response-envelopes apigatewaymanagementapi.agwmgmt-live-connection-get-post-delete-lifecycle apigatewaymanagementapi.agwmgmt-post-payload-byte-ceiling-and-unread-socket-refusal apigatewaymanagementapi.client-initiated-close-converges-to-gone-with-live-twin apigatewaymanagementapi.concurrent-post-to-connection-all-delivered apigatewaymanagementapi.connection-id-resolves-across-sibling-deployed-stages apigatewaymanagementapi.connection-operations-after-delete-return-gone-rejected apigatewaymanagementapi.cross-api-connection-id-returns-gone apigatewaymanagementapi.delete-connection-forces-client-disconnect apigatewaymanagementapi.delete-connection-triggers-disconnect-route apigatewaymanagementapi.forbidden-without-manage-connections-permission-rejected apigatewaymanagementapi.get-connection-last-active-at-advances-on-traffic-not-on-reads apigatewaymanagementapi.get-connection-last-active-at-not-before-connected-at apigatewaymanagementapi.get-connection-returns-identity-metadata apigatewaymanagementapi.get-connection-user-agent-matches-handshake-header apigatewaymanagementapi.manage-connections-denial-precedes-connection-lookup apigatewaymanagementapi.multiple-connections-message-isolation apigatewaymanagementapi.payload-too-large-rejected-at-boundary apigatewaymanagementapi.post-to-connection-binary-payload-roundtrip apigatewaymanagementapi.post-to-connection-delivers-text-payload apigatewaymanagementapi.post-to-connection-empty-payload-delivers apigatewaymanagementapi.post-to-connection-fragments-large-payload-across-frames apigatewaymanagementapi.reconnect-produces-distinct-connection-and-later-connected-at apigatewaymanagementapi.sequential-posts-preserve-message-order
Issue-Shape-Id: com.amazonaws.apigatewaymanagementapi#ApiGatewayManagementApi
Issue-Key: apigatewaymanagementapi.agwmgmt-cross-api-connection-id-rejected|run#17|ConnectionClosedError|live = mgmt_a.get_connection(ConnectionId=connection_id)
Issue-Key: apigatewaymanagementapi.agwmgmt-identity-members-and-response-envelopes|<lambda>#15|ConnectionClosedError|lambda: mgmt.get_connection(ConnectionId=bare_id),
Issue-Key: apigatewaymanagementapi.agwmgmt-live-connection-get-post-delete-lifecycle|run#15|ConnectionClosedError|described = mgmt.get_connection(ConnectionId=connection_id)
Issue-Key: apigatewaymanagementapi.agwmgmt-post-payload-byte-ceiling-and-unread-socket-refusal|run#13|ConnectionClosedError|mgmt.post_to_connection(ConnectionId=boundary_id, Data=at_limit.encode())
Issue-Key: apigatewaymanagementapi.client-initiated-close-converges-to-gone-with-live-twin|run#9|ConnectionClosedError|described_doomed = mgmt.get_connection(ConnectionId=cid_doomed)
Issue-Key: apigatewaymanagementapi.concurrent-post-to-connection-all-delivered|_send#11|ConnectionClosedError|mgmt.post_to_connection(ConnectionId=cid, Data=payload.encode("utf-8"))
Issue-Key: apigatewaymanagementapi.connection-id-resolves-across-sibling-deployed-stages|run#13|ConnectionClosedError|via_minting = mgmt_minting.get_connection(ConnectionId=connection_id)
Issue-Key: apigatewaymanagementapi.connection-operations-after-delete-return-gone-rejected|run#10|ConnectionClosedError|mgmt.delete_connection(ConnectionId=cid_dead)
Issue-Key: apigatewaymanagementapi.cross-api-connection-id-returns-gone|run#8|ConnectionClosedError|live = mgmt_a.get_connection(ConnectionId=cid_a)
Issue-Key: apigatewaymanagementapi.delete-connection-forces-client-disconnect|run#7|ConnectionClosedError|mgmt.delete_connection(ConnectionId=cid)
Issue-Key: apigatewaymanagementapi.delete-connection-triggers-disconnect-route|run#7|ConnectionClosedError|mgmt.delete_connection(ConnectionId=cid)
Issue-Key: apigatewaymanagementapi.forbidden-without-manage-connections-permission-rejected|<lambda>#8|ConnectionClosedError|code, message = _error(lambda: mgmt_unprivileged.post_to_connection(ConnectionId=cid, Data=b"denied"))
Issue-Key: apigatewaymanagementapi.get-connection-last-active-at-advances-on-traffic-not-on-reads|run#7|ConnectionClosedError|baseline = mgmt.get_connection(ConnectionId=connection_id)
Issue-Key: apigatewaymanagementapi.get-connection-last-active-at-not-before-connected-at|run#8|ConnectionClosedError|resp = mgmt.get_connection(ConnectionId=cid)
Issue-Key: apigatewaymanagementapi.get-connection-returns-identity-metadata|run#7|ConnectionClosedError|resp = mgmt.get_connection(ConnectionId=cid)
Issue-Key: apigatewaymanagementapi.get-connection-user-agent-matches-handshake-header|run#8|ConnectionClosedError|resp = mgmt.get_connection(ConnectionId=cid)
Issue-Key: apigatewaymanagementapi.manage-connections-denial-precedes-connection-lookup|_resolved_connection#9|ConnectionClosedError|return mgmt.get_connection(ConnectionId=connection_id)
Issue-Key: apigatewaymanagementapi.multiple-connections-message-isolation|run#12|ConnectionClosedError|mgmt.post_to_connection(ConnectionId=cid_a, Data=payload.encode("utf-8"))
Issue-Key: apigatewaymanagementapi.payload-too-large-rejected-at-boundary|run#8|ConnectionClosedError|mgmt.post_to_connection(ConnectionId=cid, Data=max_payload)
Issue-Key: apigatewaymanagementapi.post-to-connection-binary-payload-roundtrip|run#9|ConnectionClosedError|mgmt.post_to_connection(ConnectionId=cid, Data=raw)
Issue-Key: apigatewaymanagementapi.post-to-connection-delivers-text-payload|run#8|ConnectionClosedError|mgmt.post_to_connection(ConnectionId=cid, Data=payload.encode("utf-8"))
Issue-Key: apigatewaymanagementapi.post-to-connection-empty-payload-delivers|run#7|ConnectionClosedError|mgmt.post_to_connection(ConnectionId=cid, Data=b"")
Issue-Key: apigatewaymanagementapi.post-to-connection-fragments-large-payload-across-frames|run#9|ConnectionClosedError|mgmt.post_to_connection(ConnectionId=connection_id, Data=small)
Issue-Key: apigatewaymanagementapi.reconnect-produces-distinct-connection-and-later-connected-at|run#6|ConnectionClosedError|connected_at_a = mgmt.get_connection(ConnectionId=cid_a)["ConnectedAt"]
Issue-Key: apigatewaymanagementapi.sequential-posts-preserve-message-order|run#9|ConnectionClosedError|mgmt.post_to_connection(ConnectionId=cid, Data=payload.encode("utf-8"))
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewaymanagementapi: every @connections request on a deployed stage — including on a LIVE connection id — is answered by closing the connection with no HTTP response (25 checks)

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
ONE net-new root cause on the apigatewaymanagementapi service token, covering the 25 failures of this replay that the report marks neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: GetConnection / PostToConnection / DeleteConnection against a deployed WEBSOCKET stage produce NO HTTP response at all — the TCP/TLS connection is aborted before any status line — for every id, including the live connection id the stage itself just minted ===

AWS: Smithy model com.amazonaws.apigatewaymanagementapi#ApiGatewayManagementApi (restJson1, sigv4). GetConnection GET /@connections/{ConnectionId} -> 200 with output members ConnectedAt (jsonName connectedAt, __timestampIso8601, date-time), LastActiveAt (jsonName lastActiveAt, date-time) and Identity (jsonName identity, SourceIp/UserAgent); PostToConnection POST /@connections/{ConnectionId} -> 200 with Data @httpPayload blob @length{min 0, max 131072} and an empty body; DeleteConnection DELETE /@connections/{ConnectionId} -> 204. All three declare ForbiddenException (@httpError 403), GoneException (@httpError 410, memberless) and LimitExceededException (@httpError 429); PostToConnection additionally PayloadTooLargeException (@httpError 413). Recorded behavior for the refusals the checks grade: a well-formed id no live socket owns is HTTP 410, error code GoneException, message EMPTY (the recordings assert exactly the tuple ('GoneException', '') — the code is fixed text and the message is the empty string, nothing in it comes from the request); a caller without execute-api:ManageConnections is HTTP 403, code ForbiddenException, message 'Forbidden' (fixed text, no variable parts); a connection whose client is not draining its socket is HTTP 429, code LimitExceededException, message 'Too Many Requests. WebSocket client buffer full' (fixed text); an oversize PostToConnection payload (131073 bytes) is a bare HTTP 413 with no x-amzn-errortype header, which botocore surfaces as the literal error Code '413'. Whatever the verdict, AWS ALWAYS answers a @connections request with a complete HTTP response; it never drops the connection.

Emulator: botocore raises `ConnectionClosedError: Connection was closed before we received a valid response from endpoint URL: "https://<host>:<port>/<stage>/@connections/<connection-id>"` on the first @connections call of each of the 25 checks — on the /dev and /test and freshly created stages alike, on GetConnection and on PostToConnection. No status line, no headers, no body is ever produced. Crucially this is NOT only the unknown-id path any more: in 23 of the 25 checks the id in the aborted request is the live id the check learned from its own successful handshake (the same id the emulator serves back inside WebSocket frames in the sibling apigatewayv2 checks of this replay), so the RFC 6455 upgrade and connection minting now work and it is the @connections HTTP surface in front of the connection store that drops the request. Host, port, stage name and the connection id are per-run values; the requirement is that a complete HTTP response arrives at all.

Why: a client gets a transport-level reset in place of a modelled response. Every AWS SDK classifies an aborted connection as retryable and non-terminal, so the single most common server-push flow — push a message, prune the id when AWS says GoneException — becomes a retry storm followed by a hang, and the application can no longer distinguish 'this connection is gone' (410), 'I am not allowed' (403), 'the client buffer is full' (429) and 'the emulator crashed'. Because nothing is answered, the whole @connections surface is unusable for positive paths (delivering text/binary/empty payloads, reading ConnectedAt/LastActiveAt/Identity, forcing a disconnect) as well as for every negative one.

Fix: the @connections data-plane front door that resolves {api-id, stage} from the request path and dispatches to the three handlers, plus those handlers' response/error serialization in the apigatewaymanagementapi crate — the request must always leave with a complete HTTP response (200 + connectedAt/lastActiveAt/identity for a live id on GetConnection, 200 empty body plus a TEXT frame delivery on PostToConnection, 204 on DeleteConnection, 410 GoneException with an empty message once the connection is gone, 403 ForbiddenException 'Forbidden' without the ManageConnections grant, 429 LimitExceededException 'Too Many Requests. WebSocket client buffer full' for an unread socket) rather than by terminating the connection.

Related, does NOT cover these 25 checks (the replay reports no owner for them; it marks only 4 other apigatewaymanagementapi checks as owned): 20261110T175447Z-apigatewaymanagementapi-2-net-new-causes-a-connections-call-d8ba2e.md and 20261109T160349Z-apigatewaymanagementapi-2-net-new-causes-a-connections-call.md record the same no-response behavior for an UNDEPLOYED stage and for the unknown-id readiness probe only, and their Cause 2 ('the deployed WEBSOCKET stage mints no connection') is contradicted here: the handshake now succeeds and the live id is what gets dropped.

Traces: the 25 failing check ids in trace_ids, all with the identical ConnectionClosedError on a /@connections/ URL.

## Evidence
Suite: simparity-behavioral-aws-v1. Totals: 3/48 passed (6.2%); Mismatched 12 | Errored 33 | Skipped 0 | Unloadable 0. Per-service: apigatewaymanagementapi 0/29 (0%), 29 failed — 4 of them marked OWNED by existing issues (agwmgmt-malformed-connection-id-rejected, agwmgmt-post-oversize-payload-rejected-before-lookup, agwmgmt-undeployed-stage-endpoint-forbidden, backend-route-pushes-describes-and-disconnects-live-clients), the 25 below owned by nobody.

Every one of the 25 ends identically:
  File "endpoint.py", line 385, in _send -> File "httpsession.py", line 549, in send
  botocore.exceptions.ConnectionClosedError: Connection was closed before we received a valid response from endpoint URL: "https://127.0.0.1:<port>/<stage>/@connections/<id>"
Samples verbatim from the report (ports and ids are per-run):
  agwmgmt-cross-api-connection-id-rejected        -> https://127.0.0.1:45705/dev/@connections/EzCyjTUad5uklZ0%3D
  agwmgmt-identity-members-and-response-envelopes -> https://127.0.0.1:41503/dev/@connections/CRJZTo0Q1OSV9mw%3D
  agwmgmt-live-connection-get-post-delete-lifecycle -> https://127.0.0.1:38625/dev/@connections/EzCyjTUad5uklZ0%3D
  post-to-connection-delivers-text-payload        -> https://127.0.0.1:41931/test/@connections/EzCyjTUad5uklZ0%3D
  delete-connection-forces-client-disconnect      -> https://127.0.0.1:46001/test/@connections/EzCyjTUad5uklZ0%3D
  payload-too-large-rejected-at-boundary          -> https://127.0.0.1:40747/test/@connections/EzCyjTUad5uklZ0%3D
  sequential-posts-preserve-message-order         -> https://127.0.0.1:43407/test/@connections/EzCyjTUad5uklZ0%3D
The id 'EzCyjTUad5uklZ0=' is the connection id the emulator minted for the socket the check had just opened — the same value the emulator serves back inside WebSocket frames in this replay's apigatewayv2 checks ({"connectionId":"EzCyjTUad5uklZ0=",...}) — so the upgrade succeeded and a LIVE id is being dropped, not just an unknown one.

Expected (real AWS), as graded by these checks once a response is served again:
  GetConnection(live id)   -> 200 {connectedAt, lastActiveAt, identity{sourceIp, userAgent}} with LastActiveAt >= ConnectedAt, userAgent exactly the handshake's User-Agent header;
  PostToConnection(live id)-> 200 empty body, payload delivered as ONE TEXT frame to exactly that connection (empty Data delivers an empty frame; >125-byte payloads may be fragmented but reassemble to the exact bytes), order preserved across sequential posts and nothing lost under concurrency;
  DeleteConnection(live id)-> 204, socket closed with WebSocket code 1000, $disconnect fired, every later call on that id -> 410 GoneException with message '';
  unknown/after-delete/cross-API id -> 410 ('GoneException', '');
  no execute-api:ManageConnections   -> 403 ('ForbiddenException', 'Forbidden'), decided BEFORE the connection lookup;
  131072-byte Data -> accepted; 131073-byte Data -> bare HTTP 413, botocore Code '413';
  unread client socket -> 429 ('LimitExceededException', 'Too Many Requests. WebSocket client buffer full') and the connection stays live.
Actual (emulator): no HTTP response of any kind for any of these.

Model facts used (smithy_query query=operation on services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/smithy-model/com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json): GetConnection GET /@connections/{ConnectionId} code 200, output {ConnectedAt __timestampIso8601 jsonName connectedAt, LastActiveAt jsonName lastActiveAt, Identity jsonName identity}, errors [ForbiddenException 403, GoneException 410, LimitExceededException 429]; PostToConnection POST /@connections/{ConnectionId} code 200, Data @httpPayload blob @length{min 0,max 131072}, errors [+PayloadTooLargeException 413]; DeleteConnection DELETE /@connections/{ConnectionId} code 204, same three errors. GoneException is memberless, so its message has no modelled variable part.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, and replay the 25 apigatewaymanagementapi check ids in trace_ids from simparity-behavioral-aws-v1.
2. Create an apigatewayv2 API with ProtocolType=WEBSOCKET and RouteSelectionExpression='$request.body.action', an AWS_PROXY Lambda (or MOCK) integration on $connect/$disconnect/$default, a deployment and a stage (the checks use 'dev' and 'test').
3. Open an RFC 6455 handshake to the stage's wss URL and learn the minted connection id (the $default integration echoes requestContext.connectionId, or read it from the $connect record).
4. Build an apigatewaymanagementapi client whose endpoint_url is the API's published endpoint with wss->https plus '/<stage>' and retries capped (max_attempts=1), then call GetConnection / PostToConnection / DeleteConnection with that LIVE id.
5. Observe that no HTTP response arrives at all — botocore raises ConnectionClosedError('Connection was closed before we received a valid response from endpoint URL: "https://<host>:<port>/<stage>/@connections/<id>"') — where AWS answers 200 (with connectedAt/lastActiveAt/identity), 200 + a TEXT frame on the socket, and 204 respectively.
6. Repeat with a well-formed id owned by no socket (e.g. base64 of 11 bytes), with an id deleted a moment earlier, with an id minted by a DIFFERENT API, without the execute-api:ManageConnections grant, with 131072- and 131073-byte payloads, and against a connection that is not reading its socket; each must answer the status/code/message listed in Evidence rather than closing the connection.
7. After fixing, re-replay exactly the 25 ids in trace_ids.
