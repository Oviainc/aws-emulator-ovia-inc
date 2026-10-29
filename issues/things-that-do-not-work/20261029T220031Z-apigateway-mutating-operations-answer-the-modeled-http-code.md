Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-api-mapping-lifecycle-crud
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-api-mapping-lifecycle-crud apigatewayv2-routing-rule-missing-condition-rejected apigatewayv2-domain-name-create-server-defaults apigatewayv2-http-api-lifecycle apigatewayv2-models-and-cors apigatewayv2-import-api-basepath-split apigatewayv2-import-export-reimport-api apigatewayv2-import-export-reimport-api-round-trip apigatewayv2-route-integration-workflow apigatewayv2-route-response-lifecycle apigatewayv2-vpc-link-tags-unicode-round-trip
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateApiMapping|2xx->2xx|status
Issue-Key: apigateway|DeleteDomainName|2xx->2xx|status
Issue-Key: apigateway|DeleteDomainName|2xx->2xx|$headers.content-length
Issue-Key: apigateway|UpdateApi|2xx->2xx|$.createdDate
Issue-Key: apigateway|UpdateApi|2xx->2xx|status
Issue-Key: apigateway|ImportApi|2xx->2xx|$.createdDate
Issue-Key: apigateway|ImportApi|2xx->2xx|status
Issue-Key: apigateway|ReimportApi|2xx->2xx|$.createdDate
Issue-Key: apigateway|ReimportApi|2xx->2xx|status
Issue-Key: apigateway|UpdateRoute|2xx->2xx|status
Issue-Key: apigateway|UpdateRouteResponse|2xx->2xx|status
Issue-Key: apigateway|DeleteVpcLink|2xx->2xx|status
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway mutating operations answer the modeled HTTP code, not AWS's

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApiMapping

## Description
For ten ApiGatewayV2 mutating operations the emulator answers the HTTP status declared by the Smithy `@http` trait, but real AWS answers a different status on the wire. The emulator is consistently following the model where the recording shows the service does not. Each operation is a separate cause so all ten can be re-replayed.

Cause 1 of 10: CreateApiMapping.
AWS: 200 OK with the api-mapping body. (Smithy: CreateApiMapping @http POST /v2/domainnames/{DomainName}/apimappings code 201 - the model says 201, the recorded service answers 200, so the recording governs the wire.)
Emulator: 201 Created.
Why: a client that branches on the status (or an SDK/waiter asserting 200) treats a successful mapping creation as an unexpected response.
Fix: the CreateApiMapping handler's success-status selection.
Traces: apigatewayv2-api-mapping-lifecycle-crud#task#6.

Cause 2 of 10: DeleteDomainName (no-body delete).
AWS: 202 Accepted. (Smithy: DeleteDomainName @http DELETE /v2/domainnames/{DomainName} code 204; recorded service answers 202.)
Emulator: 204 No Content.
Why: the 202 tells a client the domain teardown is asynchronous; 204 reads as already-complete, so a client skips the settle/poll it needs.
Fix: the DeleteDomainName handler's success status.
Traces: apigatewayv2-api-mapping-lifecycle-crud#teardown#15.

Cause 3 of 10: DeleteDomainName, second recorded shape (also omits Content-Length).
AWS: 202 Accepted with a `content-length` header.
Emulator: 204 and no `content-length`.
Why/Fix: as cause 2, plus the empty-output serializer.
Traces: apigatewayv2-domain-name-create-server-defaults#task#6.

Cause 4 of 10: UpdateApi.
AWS: 201. (Smithy: UpdateApi @http PATCH /v2/apis/{ApiId} code 200; recorded service answers 201.)
Emulator: 200.
Why: clients and waiters that assert the recorded status on a PATCH see an unexpected code.
Fix: the UpdateApi handler's success status.
Traces: apigatewayv2-http-api-lifecycle#task#3.

Cause 5 of 10: ImportApi.
AWS: 200. (Smithy: ImportApi @http PUT /v2/apis code 201; recorded service answers 200.)
Emulator: 201.
Why/Fix: as cause 4, in the ImportApi handler.
Traces: apigatewayv2-import-api-basepath-split#task#1.

Cause 6 of 10: ReimportApi.
AWS: 200. (Smithy: ReimportApi @http PUT /v2/apis/{ApiId} code 201; recorded service answers 200.)
Emulator: 201.
Why/Fix: as cause 4, in the ReimportApi handler.
Traces: apigatewayv2-import-export-reimport-api#task#4.

Cause 7 of 10: ImportApi, status-only cluster (no body divergence).
AWS: 200.
Emulator: 201.
Why/Fix: as cause 5.
Traces: apigatewayv2-import-export-reimport-api-round-trip#setup#1.

Cause 8 of 10: UpdateRoute.
AWS: 201. (Smithy: UpdateRoute @http PATCH /v2/apis/{ApiId}/routes/{RouteId} code 200; recorded service answers 201.)
Emulator: 200.
Why/Fix: as cause 4, in the UpdateRoute handler.
Traces: apigatewayv2-route-integration-workflow#task#8.

Cause 9 of 10: UpdateRouteResponse.
AWS: 201. (Smithy: UpdateRouteResponse @http PATCH .../routeresponses/{RouteResponseId} code 200; recorded service answers 201.)
Emulator: 200.
Why/Fix: as cause 4, in the UpdateRouteResponse handler.
Traces: apigatewayv2-route-response-lifecycle#task#7.

Cause 10 of 10: DeleteVpcLink.
AWS: 204. (Smithy: DeleteVpcLink @http DELETE /v2/vpclinks/{VpcLinkId} code 202; recorded service answers 204.)
Emulator: 202.
Why: the inverse of cause 2 - a client told 202 waits for an asynchronous completion that never reports.
Fix: the DeleteVpcLink handler's success status.
Traces: apigatewayv2-vpc-link-tags-unicode-round-trip#teardown#3.

## Evidence
Replay verdict `mismatch`, detail `status: expected <aws>, got <emulator>` on each cluster: CreateApiMapping 200->201, DeleteDomainName 202->204 (x2), UpdateApi 201->200, ImportApi 200->201 (x2), ReimportApi 200->201, UpdateRoute 201->200, UpdateRouteResponse 201->200, DeleteVpcLink 204->202.

## Steps to Reproduce
Replay the corpus. For cause 1: POST /v2/domainnames/{domainName}/apimappings against the emulator (fixture apigatewayv2-api-mapping-lifecycle-crud step task#6) and read the status line - 201 instead of the recorded 200. Repeat for each listed fixture#step.
