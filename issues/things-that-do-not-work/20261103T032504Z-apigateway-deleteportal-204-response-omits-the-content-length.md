Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-portal-multiple-isolation
Issue-Step: teardown#8
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-portal-multiple-isolation
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|DeletePortal|2xx->2xx|$headers.content-length
Issue-Commit: 2d2373ac96719fb51b225d122133a9d558f5af51

# apigateway DeletePortal 204 response omits the Content-Length header

> _Note: this issue was identified against the code base at commit `2d2373ac96719fb51b225d122133a9d558f5af51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePortal

## Description
Cause 1 of 1: DeletePortal (fixture apigatewayv2-portal-multiple-isolation, step teardown#8).
AWS: Smithy DeletePortal — DELETE /v2/portals/{PortalId}, httpResponseCode 204, output Unit (empty body). The recorded AWS response carries a Content-Length header on that empty 204; the fixture asserts the header is present and shaped, not a particular numeral.
Emulator: answers 204 with no Content-Length header at all ('$headers.content-length: missing').
Why: a client or proxy that frames the response from Content-Length sees a message with neither Content-Length nor a terminating framing header for the empty body; strict HTTP clients and intermediaries treat that as an incomplete/unframed response, and header-asserting SDK tests fail even though the delete succeeded.
Fix: the response serializer for the portal delete handler (the shared empty-204 response path) must emit the Content-Length header for the empty body.
Traces: apigatewayv2-portal-multiple-isolation#teardown#8 and #teardown#11 (same cluster).

This step is not owned by an existing issue. The open issues 20261029T220031Z-apigateway-204-responses-omit-the-content-length-0.md and 20261031T154120Z-apigateway-content-length-0-missing-on-204-deletes-for.md describe the same class for OTHER operations (DeleteApiMapping/DeleteAuthorizer/DeleteCorsConfiguration, route settings, models, integrations, route responses, portal products) — DeletePortal is not covered by either, so fixing them must include this handler.

## Evidence
Replay divergence cluster (service=apigateway): DeletePortal, expected_status 204, actual_status 204, detail '$headers.content-length: missing (expected <shaped-not-asserted>, got None)', affected steps apigatewayv2-portal-multiple-isolation#teardown#8 and apigatewayv2-portal-multiple-isolation#teardown#11.

## Steps to Reproduce
1. Replay fixture apigatewayv2-portal-multiple-isolation.
2. At step teardown#8, DELETE /v2/portals/{portalId}.
3. Observe the 204 response headers: no Content-Length is sent. AWS sends Content-Length for the empty body.
