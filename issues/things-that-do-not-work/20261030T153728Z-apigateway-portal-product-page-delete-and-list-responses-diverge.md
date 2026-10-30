Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-multiple-product-pages-isolation
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-multiple-product-pages-isolation apigatewayv2-portal-product-page-errors apigatewayv2-portal-product-page-lifecycle apigatewayv2-product-page-error-paths apigatewayv2-product-page-lifecycle apigatewayv2-portal-product-lifecycle apigatewayv2-portal-product-multi-isolation
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|DeleteProductPage|2xx->2xx|$headers.content-length
Issue-Key: apigateway|ListPortalProducts|2xx->2xx|$.items[*]
Issue-Key: apigateway|ListPortalProducts|2xx->2xx|$.nextToken
Issue-Commit: f3303bdb390cee5b7b5d46972a178183bd5e7548

# Portal product page delete and list responses diverge: missing Content-Length and epoch lastModified

> _Note: this issue was identified against the code base at commit `f3303bdb390cee5b7b5d46972a178183bd5e7548`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteProductPage

## Description
Three net-new wire divergences on the developer-portal product surface of the ApiGatewayV2 API (wire service token `apigateway`). All three are response-serializer faults on portal-product resources: the 204 framing of a product-page delete, and the timestamp encoding of the portal-product LIST envelope. Two existing open issues describe the SAME two serializer faults but each enumerates only ten causes and neither names these operations, so neither covers them: `20261029T220031Z-apigateway-204-responses-omit-the-content-length-0.md` (ten Delete* operations, DeleteProductPage not among them) and `20261029T220031Z-apigateway-portal-product-and-vpc-link-timestamps-serialized.md` (ten single-resource operations, ListPortalProducts not among them). Three causes; all three must be closed.

Cause 1 of 3: DeleteProductPage answers 204 without a Content-Length header.
AWS: recorded behavior only for the header. Smithy (com.amazonaws.apigatewayv2): DeleteProductPage @http DELETE /v2/portalproducts/{PortalProductId}/productpages/{ProductPageId}, code 204, output Unit — the model says nothing about response headers, and the recording shows the 204 carrying an explicit `content-length` response header whose value is `0` (the fixed text is the header name; the value is the body length, which is always 0 for this empty-output operation).
Emulator: the status matches (204) but the header is absent entirely — replay detail `$headers.content-length: missing (expected '0', got None)`.
Why: a client that closes out the message on an explicit zero length, rather than on 204's implicit no-body rule, blocks waiting for a body or treats the response as malformed; proxies that normalise on Content-Length see a different message shape than against AWS.
Fix: the shared REST-JSON response serializer for empty-output (Unit) operations in the apigatewayv2 service crate — it must emit `Content-Length: 0` on a 204, for DeleteProductPage along with the other Delete* operations.
Traces: apigatewayv2-multiple-product-pages-isolation#task#5 (cluster also covers apigatewayv2-multiple-product-pages-isolation#task#6, apigatewayv2-portal-product-page-errors#task#3, apigatewayv2-portal-product-page-lifecycle#task#7, apigatewayv2-product-page-error-paths#task#5, apigatewayv2-product-page-error-paths#task#6, apigatewayv2-product-page-lifecycle#task#7 — 7 recorded steps).

Cause 2 of 3: ListPortalProducts items[].lastModified is serialized as an epoch integer.
AWS: an RFC3339/ISO-8601 date-time STRING with fractional seconds. Smithy: ListPortalProducts output Items targets __listOfPortalProductSummary; PortalProductSummary.LastModified (jsonName `lastModified`) targets __timestampIso8601 with @timestampFormat date-time. The recording shows e.g. a quoted string of the form `<YYYY-MM-DDThh:mm:ss.sssssssssZ>` — the required form is the quoted RFC3339 rendering with sub-second precision; the instant itself comes from the resource's state, never from any one recording.
Emulator: a bare JSON number (epoch seconds), so the whole list element compares unequal — replay detail `$.items[0]: missing (expected {... 'lastModified': '<ISO-8601 with fractional seconds>' ...}, got {... 'lastModified': <epoch integer> ...})`. Status matched (200) and every other member of the summary (description, displayName, portalProductArn, portalProductId, tags) matched.
Why: SDK date-time deserialization of PortalProductSummary.lastModified fails or yields a wrong instant, and the sub-second ordering a client uses to pick the newest revision out of a listing is lost.
Fix: the REST-JSON timestamp writer used by the portal-product LIST response encoder (the summary shape's encoder), so it honours @timestampFormat date-time the same way the single-resource encoders must.
Traces: apigatewayv2-portal-product-lifecycle#task#3.

Cause 3 of 3: the same epoch-integer lastModified on every element of a multi-item ListPortalProducts page.
AWS: as cause 2 — each PortalProductSummary in `items` carries an ISO-8601 date-time string.
Emulator: every element's `lastModified` is an epoch integer, so both listed elements compare unequal — replay detail `$.items[0]: missing (...)` and `$.items[1]: missing (...)`, status 200 matched.
Why: as cause 2, and it affects every element of a page rather than only the first, so no listing of portal products is parseable by a conformant client.
Fix: as cause 2 — the fix must apply per element, not only to the first item.
Traces: apigatewayv2-portal-product-multi-isolation#task#3.

Excluded deliberately: the same two ListPortalProducts clusters also report `$.nextToken` differing (recorded `bnVsbCxTdGFydFBhZ2luYXRpbmdTaGFyZWRSZXNvdXJjZXM=` vs the emulator's own hex token). A pagination continuation token is opaque and implementation-defined — the model only constrains it to __stringMin1Max2048 — so its exact bytes are not a requirement and no cause is filed for it. Both sides do return a non-empty token of a legal length.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up true, bootstrap_signal false; 799 steps, score 46.2%).

Cause 1 — verdict `mismatch`, divergence_key `service=apigateway|DeleteProductPage|status=204->204|keys=$headers.content-length`: expected_status 204, actual_status 204, detail `$headers.content-length: missing (expected '0', got None)`; affected_count 7.

Cause 2 — verdict `mismatch`, divergence_key `service=apigateway|ListPortalProducts|status=200->200|keys=$.items[0],$.nextToken`: expected_status 200, actual_status 200, detail `$.items[0]: missing` — expected element carries `'lastModified': '2026-09-05T10:36:41.957953111Z'` (a quoted RFC3339 string), got `'lastModified': 1793374429` (a JSON number). All other members of the element are identical.

Cause 3 — verdict `mismatch`, divergence_key `service=apigateway|ListPortalProducts|status=200->200|keys=$.items[0],$.items[1],$.nextToken`: expected_status 200, actual_status 200, both `$.items[0]` and `$.items[1]` diverge only in `lastModified` (expected quoted RFC3339 strings, got JSON numbers).

No status code, error code or error message diverges in any of the three clusters — these are 2xx representation/framing faults only. Neither cluster reports a stub_path, and the replay reports no owning open issue for any of the three (`owned_by: []`).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: POST /v2/portalproducts to create a portal product, POST /v2/portalproducts/{portalProductId}/productpages to create a product page, then DELETE /v2/portalproducts/{portalProductId}/productpages/{productPageId} and dump the RAW response headers (fixture apigatewayv2-multiple-product-pages-isolation step task#5). The emulator answers 204 with no `Content-Length` header; AWS answers 204 with `Content-Length: 0`.
3. Cause 2: create one portal product, then GET /v2/portalproducts (fixture apigatewayv2-portal-product-lifecycle step task#3). In the 200 body, `items[0].lastModified` is a JSON number instead of a quoted RFC3339 date-time string with fractional seconds.
4. Cause 3: create two portal products, then GET /v2/portalproducts (fixture apigatewayv2-portal-product-multi-isolation step task#3). Both `items[0].lastModified` and `items[1].lastModified` are JSON numbers.
