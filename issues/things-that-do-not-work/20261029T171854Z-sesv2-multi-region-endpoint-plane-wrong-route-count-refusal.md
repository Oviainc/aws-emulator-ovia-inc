Issue-Source: trace-sweep-divergence
Issue-Trace: sesv2-multi-region-endpoint-invalid-routes-error
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sesv2-multi-region-endpoint-invalid-routes-error sesv2-multi-region-endpoint-delete-unknown-error sesv2-multi-region-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|CreateMultiRegionEndpoint|4xx->4xx|$.message
Issue-Key: sesv2|DeleteMultiRegionEndpoint|4xx->4xx|$headers.content-type
Issue-Key: sesv2|CreateMultiRegionEndpoint|2xx->2xx|$.EndpointId
Issue-Key: sesv2|GetMultiRegionEndpoint|2xx->2xx|$headers.content-type
Issue-Key: sesv2|ListMultiRegionEndpoints|2xx->2xx|$headers.content-type
Issue-Key: sesv2|GetMultiRegionEndpoint|4xx->4xx|$headers.content-type
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Multi-region endpoint plane: wrong route-count refusal, malformed EndpointId, application/json

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMultiRegionEndpoint

## Description
Six net-new causes in the SESv2 multi-region-endpoint plane.

Cause 1 of 6: CreateMultiRegionEndpoint route-count refusal message. AWS: 400 BadRequestException with the fixed sentence 'Number of routes must be equal to 1.' — no variable part; AWS neither names the member nor reports how many routes were supplied. Emulator: an emulator-authored sentence 'Details.RoutesDetails must contain exactly one route configuration, but N were provided.', which names the member path and interpolates the supplied count. Why: a client matching AWS's refusal text does not recognise the error class. Fix: the CreateMultiRegionEndpoint handler's route-count validation message. Traces: sesv2-multi-region-endpoint-invalid-routes-error#task#1.

Cause 2 of 6: CreateMultiRegionEndpoint returns an EndpointId of the wrong form. AWS: EndpointId is a SES multi-region endpoint identifier whose form the fixture asserts by shape (the value itself is per-run and not a requirement). Emulator: returns a value that does not satisfy that shape — a 36-character dashed string whose third group starts 'cee2', i.e. not the UUID form AWS emits. Why: a client that validates or round-trips the endpoint id against AWS's documented form rejects the emulator's id, and the id appears in the ARNs the emulator then hands out. Fix: the EndpointId generator behind CreateMultiRegionEndpoint (emit the same identifier form AWS does). Traces: sesv2-multi-region-endpoint-lifecycle#task#1.

Cause 3 of 6: content-type on the GetMultiRegionEndpoint 404 (NotFoundException) response for an unknown endpoint. AWS: recorded behavior only — SESv2 responses carry content-type application/x-amz-json-1.1 (the model declares restJson1, whose default media type is application/json). Emulator: application/json; status and error code match. Fix: the service's response content-type default. Traces: sesv2-multi-region-endpoint-delete-unknown-error#task#1 (DeleteMultiRegionEndpoint 404).

Cause 4 of 6: same content-type defect on the successful GetMultiRegionEndpoint response (200). Traces: sesv2-multi-region-endpoint-lifecycle#task#2.

Cause 5 of 6: same content-type defect on the successful ListMultiRegionEndpoints response (200). Traces: sesv2-multi-region-endpoint-lifecycle#task#3.

Cause 6 of 6: same content-type defect on the GetMultiRegionEndpoint 404 after the endpoint is deleted. Traces: sesv2-multi-region-endpoint-lifecycle#teardown#5.

The open issue 20261026T041000Z-sesv2-getmultiregionendpoint-maintainability.md is a refactoring request about the error-document wrapper and does not cover any of these.

## Evidence
Replay of trace set simparity-wire-aws-v1.
Cause 1 (sesv2-multi-region-endpoint-invalid-routes-error#task#1, 400/400): $.message expected 'Number of routes must be equal to 1.', got 'Details.RoutesDetails must contain exactly one route configuration, but 2 were provided.'
Cause 2 (sesv2-multi-region-endpoint-lifecycle#task#1, 200/200): $.EndpointId failed the fixture's shape assertion ('<shaped-not-asserted>'), got '6dfa78b9-89f6-cee2-ba26-f281536eb3f5'.
Cause 3 (sesv2-multi-region-endpoint-delete-unknown-error#task#1, 404/404): $headers.content-type expected 'application/x-amz-json-1.1', got 'application/json'.
Causes 4,5 (sesv2-multi-region-endpoint-lifecycle#task#2, #task#3; both 200/200): same content-type divergence.
Cause 6 (sesv2-multi-region-endpoint-lifecycle#teardown#5, 404/404): same content-type divergence.

## Steps to Reproduce
1. Replay fixtures sesv2-multi-region-endpoint-invalid-routes-error (task#1), sesv2-multi-region-endpoint-delete-unknown-error (task#1) and sesv2-multi-region-endpoint-lifecycle (task#1, task#2, task#3, teardown#5).
2. Cause 1: CreateMultiRegionEndpoint with two entries in Details.RoutesDetails and compare the 400 message.
3. Cause 2: CreateMultiRegionEndpoint with one route and check the form of the returned EndpointId.
4. Causes 3-6: inspect the content-type response header on GetMultiRegionEndpoint (200 and 404), ListMultiRegionEndpoints and DeleteMultiRegionEndpoint for an unknown endpoint.
