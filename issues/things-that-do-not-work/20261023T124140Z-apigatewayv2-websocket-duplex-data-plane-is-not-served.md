Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda.websocket-api-connect-message-post-back-disconnect apigatewayv2__dynamodb__lambda.wsduplex-client-binary-frame-rejected-close-1003 apigatewayv2__dynamodb__lambda.wsduplex-connect-denial-rejects-handshake apigatewayv2__dynamodb__lambda.wsduplex-oversize-client-message-rejected-close-1009

# apigatewayv2: a deployed WEBSOCKET api serves no data plane — the vendored listener refuses every WebSocket upgrade, so no handshake, route dispatch or close code can be observed (1 cause)

> _Filed by the fixer of
> `20261023T124140Z-apigatewayv2-dynamodb-lambda-websocket-duplex-checks-all-abort.md`,
> whose single cause (CreateDeployment answering the unmodelled `NotImplemented`) is
> fixed: the control plane now mints deployments and releases them to stages. What that
> uncovered is the cause below, which the four duplex traces reach only once deploy
> works. It is recorded here so the gap is not lost behind a deploy-time error._

## Operation
(data plane) `wss://{api-id}.execute-api.{region}.amazonaws.com/{stage}`

## Description
Cause 1 of 1: the apigatewayv2 emulator serves the CONTROL plane only, and the vendored
HTTP front door cannot serve the WebSocket transport at all. A WEBSOCKET api can be
created, given an `AWS_PROXY` Lambda integration and `$connect`/`$default`/`$disconnect`
routes, deployed and released to a stage — `GetStage` reports the `DeploymentId` and
`CreateApi` reports `ApiEndpoint` `wss://{api-id}.execute-api.{region}.amazonaws.com` —
and then nothing accepts a WebSocket upgrade on that endpoint.

AWS: a released stage of a WEBSOCKET api accepts the RFC 6455 upgrade at
`wss://{api-id}.execute-api.{region}.amazonaws.com/{stage}`; it invokes the `$connect`
route's integration (and its authorizer, when the route has one) before completing the
handshake and REFUSES the handshake when that answers Deny or a non-2xx; it dispatches a
client frame to the route named by the api's `RouteSelectionExpression`, falling back to
`$default`; it closes with the protocol's own status codes — 1003 for a frame whose type
the api does not accept, 1009 for a message over the frame limit; and it invokes
`$disconnect` when the socket closes. The `@connections` side of that conversation is the
Amazon API Gateway Management API (`PostToConnection`, `GetConnection`,
`DeleteConnection`), whose connection ids the data plane mints.

Emulator: every WebSocket upgrade is refused by the vendored listener before any service
is reached — `vendor/server/src/listeners.rs:505-507` answers
`501 "websocket listeners are not supported"` for any request that
`is_websocket_upgrade(connection, upgrade, :protocol)` matches
(`vendor/server/src/listener_guard.rs:126`). On top of that the front door serves one
BUFFERED request per call (`vendor/server/src/lib.rs`), the router strips the hop-by-hop
`connection`/`upgrade` response headers a handshake needs
(`vendor/smithy-server-runtime/src/multi_service_router.rs`), and an operation is handed
the request parts by shared reference so it can never take hyper's upgrade handle
(`vendor/smithy-server-runtime/src/operation.rs`). The same wall is documented, with the
same citations, by `transcribestreaming`, which answers the WebSocket spelling of its
streaming operations with a modelled in-band error for exactly this reason
(`services/service-com-amazonaws-transcribestreaming-transcribe/src/ops/types/event_stream.rs`,
`NO_WEBSOCKET_TRANSPORT`). The apigatewayv2 crate also holds no connection state
(`services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/state.rs` keys apis,
routes, integrations, stages, deployments, models and authorizers only) and nothing
shares connection ids with the apigatewaymanagementapi crate.

Why: every two-way behaviour a WebSocket api exists for is unobservable — the handshake
refusal a `$connect` Deny is supposed to produce, the close codes a client grades, and
the post-back a server sends through the management API.

Fix: this is a VENDOR blocker as the tree stands, not an emulator gap a service crate can
close — `vendor/` is read-only here, so a fixer who confirms the citations above should
close this with `mark_wont_fix(category="vendor_blocker")`,
`evidence_path=vendor/server/src/listeners.rs`, naming the required core change: the
listener must stop refusing `is_websocket_upgrade` requests and instead hand a claiming
service the upgrade (hyper's `OnUpgrade`) plus a bidirectional frame channel, and the
router must stop stripping the hop-by-hop handshake headers. Once the core offers that,
the apigatewayv2 side is implementable and is what this issue then asks for: accept the
upgrade only for a released stage of a WEBSOCKET api; a connection registry in this
crate's state, reachable by the apigatewaymanagementapi crate (so a shared crate under
`services/`) and keyed by the connection id its `@connections/{ConnectionId}` path names;
`$connect`/`$default`/`$disconnect` dispatch through each route's integration, which for
`AWS_PROXY` is a real Lambda invocation; RFC 6455 framing from a maintained crate rather
than hand-written; and the close codes above derived from the frame that caused them,
never from a recorded expectation.

## Evidence
With deploy working, the four traces in `Issue-Trace-Ids` get past
`CreateApi`/`CreateIntegration`/`CreateRoute`/`CreateStage`/`CreateDeployment` and then
have nothing to connect to: the vendored listener answers the upgrade with
`501 websocket listeners are not supported` (`vendor/server/src/listeners.rs:505-507`),
so the `$connect` denial, the 1003 and 1009 closes and the `PostToConnection` post-back
are never reached.

## Steps to Reproduce
1. `CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action')`,
   `CreateIntegration(IntegrationType='AWS_PROXY', IntegrationUri=<lambda arn>)`,
   `CreateRoute` for `$connect`, `$default` and `$disconnect`,
   `CreateStage(StageName='dev')`, `CreateDeployment(StageName='dev')` — all succeed, and
   `GetStage` reports the `DeploymentId`.
2. Open `wss://<api-id>.execute-api.<region>.amazonaws.com/dev` against the emulator (or
   send a `GET` carrying `Connection: Upgrade` / `Upgrade: websocket` to its data port) —
   the listener answers 501 `websocket listeners are not supported`, so the handshake
   refusal, the close codes (1003, 1009) and the `$disconnect` invocation cannot be
   observed.
