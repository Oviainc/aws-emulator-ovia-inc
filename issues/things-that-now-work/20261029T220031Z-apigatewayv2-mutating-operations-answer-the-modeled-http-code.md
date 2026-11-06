Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-api-stage-bogus-child-not-found
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-api-stage-bogus-child-not-found apigatewayv2-create-routing-rule-missing-condition-rejected apigatewayv2-put-routing-rule-nonexistent-not-found apigatewayv2-routing-rule-priority-numeric-precision apigatewayv2-domain-api-mapping-bogus-child-not-found apigatewayv2-domain-routing-rule-bogus-child-and-bounds apigatewayv2-import-reimport-status-codes apigatewayv2-stage-route-settings-bogus-child-not-found apigatewayv2-websocket-deployment-requires-route-targets apigatewayv2-websocket-model-route-response-status-codes
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|UpdateApi|2xx->2xx|$.createdDate
Issue-Key: apigatewayv2|UpdateApi|2xx->2xx|status
Issue-Key: apigatewayv2|DeleteDomainName|2xx->2xx|status
Issue-Key: apigatewayv2|CreateApiMapping|2xx->2xx|$.apiMappingId
Issue-Key: apigatewayv2|CreateApiMapping|2xx->2xx|status
Issue-Key: apigatewayv2|DeleteApiMapping|2xx->2xx|$headers.content-length
Issue-Key: apigatewayv2|DeleteDomainName|2xx->2xx|$headers.content-length
Issue-Key: apigatewayv2|ImportApi|2xx->2xx|$.createdDate
Issue-Key: apigatewayv2|ImportApi|2xx->2xx|status
Issue-Key: apigatewayv2|ReimportApi|2xx->2xx|$.createdDate
Issue-Key: apigatewayv2|ReimportApi|2xx->2xx|status
Issue-Key: apigatewayv2|UpdateRoute|2xx->2xx|status
Issue-Key: apigatewayv2|DeleteRoute|2xx->2xx|$headers.content-length
Issue-Key: apigatewayv2|UpdateRouteResponse|2xx->2xx|status
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigatewayv2 mutating operations answer the modeled HTTP code, not AWS's

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApi

## Description
For ten ApiGatewayV2 mutating operations the emulator answers the status declared by the Smithy `@http` trait while the recorded service answers a different one; two of them additionally omit `Content-Length` on a 204. Ten causes so each can be re-replayed.

Cause 1 of 10: UpdateApi.
AWS: 201. (Smithy: UpdateApi @http PATCH /v2/apis/{ApiId} code 200; the recorded service answers 201, so the recording governs the wire.)
Emulator: 200.
Why: a client or waiter that asserts the recorded status on the PATCH sees an unexpected code.
Fix: the UpdateApi handler's success-status selection.
Traces: apigatewayv2-api-stage-bogus-child-not-found#task#2.

Cause 2 of 10: DeleteDomainName.
AWS: 202 Accepted. (Smithy: DeleteDomainName @http code 204; recorded service answers 202.)
Emulator: 204.
Why: 202 tells the client the teardown is asynchronous; 204 reads as complete, so a client skips the settle it needs.
Fix: the DeleteDomainName handler's success status.
Traces: apigatewayv2-create-routing-rule-missing-condition-rejected#teardown#14.

Cause 3 of 10: CreateApiMapping.
AWS: 200 with the api-mapping body. (Smithy: CreateApiMapping @http code 201; recorded service answers 200.)
Emulator: 201.
Why/Fix: as cause 1, in the CreateApiMapping handler.
Traces: apigatewayv2-domain-api-mapping-bogus-child-not-found#task#5.

Cause 4 of 10: DeleteApiMapping omits Content-Length on its 204.
AWS: recorded behavior only - 204 with a `content-length` response header.
Emulator: 204 with no `content-length`.
Why: a client that relies on an explicit zero length to close out the message can block or treat the response as malformed.
Fix: the REST-JSON empty-output serializer - emit `Content-Length: 0` on a 204.
Traces: apigatewayv2-domain-api-mapping-bogus-child-not-found#task#9.

Cause 5 of 10: DeleteDomainName - wrong status and missing Content-Length.
AWS: 202 with a `content-length` header.
Emulator: 204 with no `content-length`.
Why/Fix: as causes 2 and 4 together.
Traces: apigatewayv2-domain-api-mapping-bogus-child-not-found#task#10.

Cause 6 of 10: ImportApi.
AWS: 200. (Smithy: ImportApi @http PUT /v2/apis code 201; recorded service answers 200.)
Emulator: 201.
Why/Fix: as cause 1, in the ImportApi handler.
Traces: apigatewayv2-import-reimport-status-codes#task#1.

Cause 7 of 10: ReimportApi.
AWS: 200. (Smithy: ReimportApi @http PUT /v2/apis/{ApiId} code 201; recorded service answers 200.)
Emulator: 201.
Why/Fix: as cause 1, in the ReimportApi handler.
Traces: apigatewayv2-import-reimport-status-codes#task#2.

Cause 8 of 10: UpdateRoute.
AWS: 201. (Smithy: UpdateRoute @http PATCH /v2/apis/{ApiId}/routes/{RouteId} code 200; recorded service answers 201.)
Emulator: 200.
Why/Fix: as cause 1, in the UpdateRoute handler.
Traces: apigatewayv2-stage-route-settings-bogus-child-not-found#task#5.

Cause 9 of 10: DeleteRoute omits Content-Length on its 204.
AWS: recorded behavior only - 204 with a `content-length` header.
Emulator: 204 with no `content-length`.
Why/Fix: as cause 4.
Traces: apigatewayv2-websocket-deployment-requires-route-targets#task#7.

Cause 10 of 10: UpdateRouteResponse.
AWS: 201. (Smithy: UpdateRouteResponse @http PATCH .../routeresponses/{RouteResponseId} code 200; recorded service answers 201.)
Emulator: 200.
Why/Fix: as cause 1, in the UpdateRouteResponse handler.
Traces: apigatewayv2-websocket-model-route-response-status-codes#task#6.

## Evidence
Replay verdict `mismatch`, detail `status: expected <aws>, got <emulator>`: UpdateApi 201->200, DeleteDomainName 202->204 (x2), CreateApiMapping 200->201, ImportApi 200->201, ReimportApi 200->201, UpdateRoute 201->200, UpdateRouteResponse 201->200; plus `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)` on DeleteApiMapping, DeleteRoute and one DeleteDomainName cluster.

## Steps to Reproduce
Replay the corpus. Cause 1: PATCH /v2/apis/{apiId} (fixture apigatewayv2-api-stage-bogus-child-not-found step task#2) - 200 instead of the recorded 201. Cause 2: DELETE /v2/domainnames/{domainName} (apigatewayv2-create-routing-rule-missing-condition-rejected#teardown#14) - 204 instead of 202.

## Resolution

Causes 1, 2, 3, 5, 6, 7, 8 and 10 (the statuses): a modelled operation's success status is
now the status the service ANSWERS rather than the status its `@http` trait declares, where
the two differ. The seven statuses that differ are single-homed in
`src/state/wire_status.rs` and named by the handlers they govern — `UpdateApi`,
`UpdateRoute` and `UpdateRouteResponse` answer `201`, `ImportApi`, `ReimportApi` and
`CreateApiMapping` answer `200`, and `DeleteDomainName` answers `202 Accepted`, the status
of the teardown this service already models as accepted-not-finished
(`state::DOMAIN_NAME_DELETION`). Every other operation keeps its modelled `code`.

Causes 4 and 9 (`content-length: 0` on an empty `204`): the service STATES that framing on
every answer it serializes with no body, at whatever status — `src/service.rs` now frames an
empty answer from the BODY rather than from the `204` status, so the `202` teardown states it
too, which `tests/all/mutating_success_status.rs` pins on a real socket. CAVEAT: a client
reached over HTTP/1.1 still does not see the header on a `204`, because the HTTP writer the
vendored transport uses removes `content-length` from a `204` (RFC 9110 §15.3.5 forbids a
body there) — that writer is under the read-only vendored tree, so the wire-level half of
those two causes is a platform limitation rather than something this service can state its
way out of. The same limitation is recorded for `serverlessrepo|DeleteApplication` in
`20261022T011815Z-serverlessrepo-response-serialization-drops-empty-labels-presigned-templateurl.md`.
