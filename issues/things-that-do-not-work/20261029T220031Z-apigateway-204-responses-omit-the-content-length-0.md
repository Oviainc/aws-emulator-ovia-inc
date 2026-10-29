Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-api-mapping-lifecycle-crud
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-api-mapping-lifecycle-crud apigatewayv2-authorizer-lifecycle apigatewayv2-cors-configuration-lifecycle apigatewayv2-http-api-cors-accesslog-cache-cleanup apigatewayv2-deployment-error-paths apigatewayv2-deployment-lifecycle apigatewayv2-stage-access-log-settings-lifecycle apigatewayv2-integration-aws-proxy-crud apigatewayv2-integration-error-paths apigatewayv2-portal-product-sharing-policy apigatewayv2-portal-product-sharing-policy-roundtrip apigatewayv2-product-rest-endpoint-page-lifecycle apigatewayv2-quick-create-managed-resources apigatewayv2-route-error-paths apigatewayv2-route-with-response-selection-expression-crud apigatewayv2-routes-pagination-max-results-one apigatewayv2-websocket-api-lifecycle apigatewayv2-stage-create-with-tags
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|DeleteApiMapping|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeleteAuthorizer|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeleteCorsConfiguration|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeleteDeployment|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeleteAccessLogSettings|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeleteIntegration|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeletePortalProductSharingPolicy|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeleteProductRestEndpointPage|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeleteRoute|2xx->2xx|$headers.content-length
Issue-Key: apigateway|DeleteStage|2xx->2xx|$headers.content-length
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway 204 responses omit the Content-Length: 0 header

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApiMapping

## Description
Every ApiGatewayV2 delete-style operation that answers 204 No Content emits the status line and body framing but never sets `Content-Length: 0`. The recorded AWS responses always carry that header on these 204s. This is one serializer-level fault shared by 10 operations; each is listed below as its own cause so all ten can be re-replayed.

Cause 1 of 10: DeleteApiMapping.
AWS: recorded behavior only - the model (DeleteApiMapping @http code 204, output Unit) says nothing about headers; the recording shows a 204 with an explicit `content-length` response header.
Emulator: 204 with no `content-length` header at all ($headers.content-length: missing).
Why: an HTTP client that relies on an explicit zero length to close out the message (rather than on 204's implicit no-body rule) can block waiting for a body or treat the response as malformed.
Fix: the shared REST-JSON response serializer for empty-output operations - it must emit `Content-Length: 0` on a 204.
Traces: apigatewayv2-api-mapping-lifecycle-crud#task#10.

Cause 2 of 10: DeleteAuthorizer.
AWS: recorded behavior only - 204 with `content-length: 0`.
Emulator: 204, `content-length` absent.
Why/Fix: as cause 1.
Traces: apigatewayv2-authorizer-lifecycle#task#6.

Cause 3 of 10: DeleteCorsConfiguration.
AWS: recorded behavior only - 204 with `content-length: 0`.
Emulator: 204, `content-length` absent.
Why/Fix: as cause 1.
Traces: apigatewayv2-cors-configuration-lifecycle#task#3.

Cause 4 of 10: DeleteDeployment.
AWS: recorded behavior only - 204 with `content-length: 0`.
Emulator: 204, `content-length` absent.
Why/Fix: as cause 1.
Traces: apigatewayv2-deployment-error-paths#task#5.

Cause 5 of 10: DeleteAccessLogSettings.
AWS: recorded behavior only - 204 with a `content-length` header.
Emulator: 204, `content-length` absent.
Why/Fix: as cause 1.
Traces: apigatewayv2-http-api-cors-accesslog-cache-cleanup#task#6.

Cause 6 of 10: DeleteIntegration.
AWS: recorded behavior only - 204 with a `content-length` header.
Emulator: 204, `content-length` absent.
Why/Fix: as cause 1.
Traces: apigatewayv2-integration-aws-proxy-crud#task#5.

Cause 7 of 10: DeletePortalProductSharingPolicy.
AWS: recorded behavior only - 204 with `content-length: 0`.
Emulator: 204, `content-length` absent.
Why/Fix: as cause 1.
Traces: apigatewayv2-portal-product-sharing-policy#task#5.

Cause 8 of 10: DeleteProductRestEndpointPage.
AWS: recorded behavior only - 204 with `content-length: 0`.
Emulator: 204, `content-length` absent.
Why/Fix: as cause 1.
Traces: apigatewayv2-product-rest-endpoint-page-lifecycle#task#10.

Cause 9 of 10: DeleteRoute.
AWS: recorded behavior only - 204 with a `content-length` header.
Emulator: 204, `content-length` absent.
Why/Fix: as cause 1.
Traces: apigatewayv2-quick-create-managed-resources#task#5.

Cause 10 of 10: DeleteStage.
AWS: recorded behavior only - 204 with a `content-length` header.
Emulator: 204, `content-length` absent.
Why/Fix: as cause 1.
Traces: apigatewayv2-stage-create-with-tags#task#8.

## Evidence
Replay verdict `mismatch` on all ten clusters, detail `$headers.content-length: missing (expected '0' | '<shaped-not-asserted>', got None)`, expected_status == actual_status == 204 in every case. 29 recorded steps are affected across the ten clusters.

## Steps to Reproduce
Replay the wire corpus and inspect any of the listed fixture steps, e.g. apigatewayv2-api-mapping-lifecycle-crud#task#10: create a domain name, create an API mapping, then DELETE /v2/domainnames/{domainName}/apimappings/{apiMappingId} and dump the raw response headers. The emulator answers 204 with no Content-Length header.
