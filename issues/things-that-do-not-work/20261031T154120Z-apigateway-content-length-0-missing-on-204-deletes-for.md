Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-http-api-route-config-cleanup
Issue-Step: task#9
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-http-api-route-config-cleanup apigatewayv2-route-settings-lifecycle apigatewayv2-integration-response-errors apigatewayv2-integration-response-lifecycle apigatewayv2-model-error-paths apigatewayv2-model-lifecycle apigatewayv2-portal-product-error-paths apigatewayv2-portal-product-lifecycle apigatewayv2-route-response-errors apigatewayv2-route-response-lifecycle
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|DeleteRouteSettings|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeleteIntegrationResponse|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeleteModel|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeletePortalProduct|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeleteRouteResponse|2xx->2xx|$headers.content-length
Issue-Commit: 418fa7a6b82984f3652fde3052f2e7dd1ee98ae8

# Content-Length 0 missing on 204 deletes for route settings, models, integration and route responses and portal products

> _Note: this issue was identified against the code base at commit `418fa7a6b82984f3652fde3052f2e7dd1ee98ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRouteSettings

## Description
Five more delete operations answer the modeled 204 but omit the `Content-Length: 0` response header that the recorded AWS responses carry. The open issue `20261029T220031Z-apigateway-204-responses-omit-the-content-length-0.md` reports the same fault on DeleteApiMapping, DeleteAuthorizer and DeleteCorsConfiguration and owns those replay keys; the five clusters below are different operations, carry their own replay keys and are therefore filed here. All five must be closed.

Cause 1 of 5: DeleteRouteSettings answers 204 with no Content-Length header.
AWS: Smithy: DeleteRouteSettings http {method DELETE, uri /v2/apis/{ApiId}/stages/{StageName}/routesettings/{RouteKey}, code 204}, output shape Unit (no body). The header itself is recorded behavior only: the recorded 204 carries `Content-Length: 0`, i.e. an explicit zero-length body framing on every empty success response.
Emulator: status 204 is correct, but the `content-length` header is absent from the response entirely.
Why: a client or intermediary that frames the body from Content-Length (and SDKs/proxies that treat a missing Content-Length on a non-chunked response as an unframed body) must fall back to connection-close semantics or report a malformed response, so an otherwise successful delete can surface as a transport error and keep-alive reuse is lost.
Fix: the shared REST-JSON response serializer for empty-body (204) responses — emit `Content-Length: 0` rather than omitting the header; this is not a per-handler fix.
Traces: apigatewayv2-http-api-route-config-cleanup#task#9 (also apigatewayv2-route-settings-lifecycle#task#5).

Cause 2 of 5: DeleteIntegrationResponse answers 204 with no Content-Length header.
AWS: Smithy: DeleteIntegrationResponse http {method DELETE, uri /v2/apis/{ApiId}/integrations/{IntegrationId}/integrationresponses/{IntegrationResponseId}, code 204}, output Unit; recorded behavior only for the header — recorded response carries `Content-Length: 0`.
Emulator: 204 with the header absent.
Why: as cause 1.
Fix: as cause 1.
Traces: apigatewayv2-integration-response-errors#task#8 (also apigatewayv2-integration-response-lifecycle#task#8).

Cause 3 of 5: DeleteModel answers 204 with no Content-Length header.
AWS: Smithy: DeleteModel http {method DELETE, uri /v2/apis/{ApiId}/models/{ModelId}, code 204}, output Unit; recorded behavior only for the header — `Content-Length: 0`.
Emulator: 204 with the header absent.
Why: as cause 1.
Fix: as cause 1.
Traces: apigatewayv2-model-error-paths#task#4 (also apigatewayv2-model-lifecycle#task#8).

Cause 4 of 5: DeletePortalProduct answers 204 with no Content-Length header.
AWS: Smithy: DeletePortalProduct http {method DELETE, uri /v2/portalproducts/{PortalProductId}, code 204}, output Unit; recorded behavior only for the header — `Content-Length: 0`.
Emulator: 204 with the header absent.
Why: as cause 1.
Fix: as cause 1.
Traces: apigatewayv2-portal-product-error-paths#task#7 (also apigatewayv2-portal-product-lifecycle#task#6).

Cause 5 of 5: DeleteRouteResponse answers 204 with no Content-Length header.
AWS: Smithy: DeleteRouteResponse http {method DELETE, uri /v2/apis/{ApiId}/routes/{RouteId}/routeresponses/{RouteResponseId}, code 204}, output Unit; recorded behavior only for the header — `Content-Length: 0`.
Emulator: 204 with the header absent.
Why: as cause 1.
Fix: as cause 1.
Traces: apigatewayv2-route-response-errors#task#8 (also apigatewayv2-route-response-lifecycle#task#9).

## Evidence
Replay verdict `mismatch` on all five clusters, each with the same detail shape: `$headers.content-length: missing (expected '0', got None)` (DeleteIntegrationResponse, DeleteModel, DeletePortalProduct, DeleteRouteResponse) and `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)` (DeleteRouteSettings). Expected status 204, actual status 204 in every case — only the header diverges.

## Steps to Reproduce
Replay the corpus. Cause 1: create an HTTP API with a stage carrying per-route settings, then DELETE /v2/apis/{apiId}/stages/{stageName}/routesettings/{routeKey} (fixture apigatewayv2-http-api-route-config-cleanup step task#9) and inspect the response headers — 204 arrives with no content-length. Cause 2: DELETE an integration response (apigatewayv2-integration-response-errors#task#8). Cause 3: DELETE a model (apigatewayv2-model-error-paths#task#4). Cause 4: DELETE a portal product (apigatewayv2-portal-product-error-paths#task#7). Cause 5: DELETE a route response (apigatewayv2-route-response-errors#task#8).
