Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-domain-api-mapping-bogus-child-not-found
Issue-Step: task#9
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-domain-api-mapping-bogus-child-not-found apigatewayv2-websocket-deployment-requires-route-targets
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|DeleteApiMapping|2xx->2xx|$headers.content-length
Issue-Key: apigatewayv2|DeleteRoute|2xx->2xx|$headers.content-length
Issue-Commit: 15ef1adb3fb7a9db5ce7069e30860cdce3c8580a

# apigatewayv2 204 deletes omit the Content-Length: 0 header (DeleteApiMapping, DeleteRoute)

> _Note: this issue was identified against the code base at commit `15ef1adb3fb7a9db5ce7069e30860cdce3c8580a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApiMapping

## Description
Two more empty-body delete operations answer the modeled 204 but omit the `Content-Length: 0` response header the recorded AWS responses carry. The open issues `20261029T220031Z-apigateway-204-responses-omit-the-content-length-0.md` and `20261031T154120Z-apigateway-content-length-0-missing-on-204-deletes-for.md` report the same fault on other operations and under the `apigateway` service token, and own those replay keys; the two clusters below carry their own replay keys (service token `apigatewayv2`) and are therefore filed here. BOTH causes must be closed.

Cause 1 of 2: DeleteApiMapping answers 204 with no Content-Length header.
AWS: Smithy (com.amazonaws.apigatewayv2#DeleteApiMapping): http {method DELETE, uri /v2/domainnames/{DomainName}/apimappings/{ApiMappingId}, code 204}, output shape Unit (no body). The header itself is recorded behavior only: the recorded 204 carries an explicit zero-length body framing, `Content-Length: 0`, on the empty success response.
Emulator: the status 204 is correct, but the `content-length` header is absent from the response entirely.
Why: a client or intermediary that frames the response body from Content-Length (and SDKs/proxies that treat a missing Content-Length on a non-chunked response as an unframed body) must fall back to connection-close semantics or report a malformed response, so a successful delete can surface as a transport error and keep-alive connection reuse is lost.
Fix: the shared REST-JSON response serializer for empty-body (204) responses — emit `Content-Length: 0` instead of omitting the header; this is not a per-handler fix.
Traces: apigatewayv2-domain-api-mapping-bogus-child-not-found#task#9.

Cause 2 of 2: DeleteRoute answers 204 with no Content-Length header.
AWS: Smithy (com.amazonaws.apigatewayv2#DeleteRoute): http {method DELETE, uri /v2/apis/{ApiId}/routes/{RouteId}, code 204}, output shape Unit (no body); the header is recorded behavior only — the recorded 204 carries `Content-Length: 0`.
Emulator: status 204 with the `content-length` header absent.
Why: as cause 1.
Fix: as cause 1.
Traces: apigatewayv2-websocket-deployment-requires-route-targets#task#7.

## Evidence
Replay verdict `mismatch` on both clusters, status matching on both sides and only the header diverging.

Cause 1: cluster key `service=apigatewayv2|DeleteApiMapping|status=204->204|keys=$headers.content-length`, detail `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`; expected status 204, actual status 204.
Cause 2: cluster key `service=apigatewayv2|DeleteRoute|status=204->204|keys=$headers.content-length`, detail `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`; expected status 204, actual status 204.

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1).
Cause 1: fixture apigatewayv2-domain-api-mapping-bogus-child-not-found, step task#9 — create a custom domain name and an API mapping on it, then `DELETE /v2/domainnames/{domainName}/apimappings/{apiMappingId}` and inspect the raw response headers: 204 arrives with no content-length.
Cause 2: fixture apigatewayv2-websocket-deployment-requires-route-targets, step task#7 — create a WEBSOCKET API with a route, then `DELETE /v2/apis/{apiId}/routes/{routeId}` and inspect the raw response headers: 204 arrives with no content-length.
