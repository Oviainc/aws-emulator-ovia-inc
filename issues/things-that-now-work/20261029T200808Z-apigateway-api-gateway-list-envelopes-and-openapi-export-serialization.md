Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-documentation-part-resource-and-method-locations
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-documentation-part-resource-and-method-locations apigateway-documentation-parts-multiple-locations-and-filtering apigw-envelope-api-keys-position-walk apigw-envelope-documentation-parts-position-walk apigateway-create-base-path-mapping-nonexistent-restapi-fails apigateway-put-rest-api-merge-then-overwrite apigateway-rest-api-import-export-and-put apigateway-rest-api-import-export-put
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetDocumentationParts|2xx->2xx|$.item[id="4kdlzkcaaa"]
Issue-Key: apigateway|GetDocumentationParts|2xx->2xx|$.limit
Issue-Key: apigateway|GetDocumentationParts|2xx->2xx|$.position
Issue-Key: apigateway|GetDocumentationParts|2xx->2xx|$.item[id="2gbc2hdaaa"]
Issue-Key: apigateway|GetDocumentationParts|2xx->2xx|$.item[id="3iuyi1caaa"]
Issue-Key: apigateway|GetApiKeys|2xx->2xx|$.limit
Issue-Key: apigateway|GetApiKeys|2xx->2xx|$.position
Issue-Key: apigateway|GetDocumentationParts|2xx->4xx|$.item
Issue-Key: apigateway|GetDocumentationParts|2xx->4xx|status
Issue-Key: apigateway|GetBasePathMappings|2xx->2xx|$
Issue-Key: apigateway|GetExport|2xx->2xx|$.paths./orders.get.consumes
Issue-Key: apigateway|GetExport|2xx->2xx|$.components
Issue-Key: apigateway|GetExport|2xx->2xx|$.paths./pets.get.responses.200.content
Issue-Key: apigateway|GetExport|2xx->2xx|$headers.content-disposition
Issue-Related: 20261029T200808Z-apigateway-api-gateway-sdk-type-catalogue-and-openapi-export.md
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway list envelopes and OpenAPI export serialization (9 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDocumentationParts

## Description
Paged collection responses lose the envelope members AWS returns, an empty collection is serialised with an empty list instead of being omitted, and the OAS30/Swagger exporter omits members AWS always writes. Smithy: GetDocumentationParts output = items (jsonName 'item') + position; GetApiKeys and GetBasePathMappings have the same position/limit envelope; GetExport returns the definition as its payload with a content-disposition header.

Cause 1 of 9 - GetDocumentationParts with `limit`. AWS echoes the requested `limit` in the response and returns `position` as the composite location token of the last item on the page (recorded form `RESOURCE::/widgets::` - type, name, path and statusCode joined by `:`), and the page holds the item AWS's ordering puts there. The emulator omits `limit` entirely and returns the last item's id as `position`. Why: a client paging with the returned position sends an id AWS would never issue, so the next page is wrong or refused. Fix: GetDocumentationParts' pagination envelope (limit echo + composite position token). Trace: apigateway-documentation-part-resource-and-method-locations#task#11.

Cause 2 of 9 - the same envelope defect with a filter applied; the expected position token takes the MODEL form `MODEL:<modelName>:::`. Trace: apigateway-documentation-parts-multiple-locations-and-filtering#task#13.

Cause 3 of 9 - GetApiKeys with limit=1: `limit` is not echoed and `position` has the wrong shape. Fix: the api-keys listing envelope. Trace: apigw-envelope-api-keys-position-walk#task#3.

Cause 4 of 9 - GetDocumentationParts position walk: `limit` not echoed, `position` returned as an item id instead of the composite token. Trace: apigw-envelope-documentation-parts-position-walk#task#7 (2 steps).

Cause 5 of 9 - GetDocumentationParts given a position past the end of the collection. AWS: 200 with `item` = []. Emulator: 400. Why: a client that walks to the end of a collection is handed a client error instead of an empty final page. Fix: position handling in GetDocumentationParts. Trace: apigw-envelope-documentation-parts-position-walk#task#11.

Cause 6 of 9 - GetBasePathMappings for a domain with no mappings. AWS returns an empty body `{}` (the item member is omitted); the emulator returns `{"item": []}`. Why: clients distinguishing 'no member' from 'empty list' misread the result. Fix: omit empty collections in the base-path-mapping serialiser. Trace: apigateway-create-base-path-mapping-nonexistent-restapi-fails#task#4.

Cause 7 of 9 - GetExport (swagger) omits each method's `consumes` list and the document-level `x-amazon-apigateway-security-policy: TLS_1_0`. Fix: the exporter. Trace: apigateway-put-rest-api-merge-then-overwrite#task#6.

Cause 8 of 9 - GetExport (oas30) additionally omits the empty `components: {}` object and each response's empty `content: {}` object that AWS always writes, plus the security policy. Fix: the OAS30 writer. Trace: apigateway-rest-api-import-export-and-put#task#5.

Cause 9 of 9 - the same oas30 omissions, plus the `content-disposition` filename: AWS names the attachment `<exportType>_<the API's version>.json` (recorded `oas30_1.0.json`), while the emulator substitutes a wall-clock timestamp for the version. The timestamp itself is per-run, but deriving the filename from a clock rather than from the API version is the defect. Fix: GetExport's content-disposition header construction. Trace: apigateway-rest-api-import-export-put#task#5.

No open issue owns these clusters.

## Evidence
Cause 1: $.limit missing (expected 1); $.position expected 'RESOURCE::/widgets::', got the last item's id; $.item[...] page content diverges.
Cause 2: $.limit missing (expected 2); $.position expected 'MODEL:sim2realprobeapigatewayPetModelc29f2c71:::', got an item id.
Cause 3/4: $.limit missing (expected 1); $.position shape assertion failed / expected 'MODEL:<name>:::'.
Cause 5: status expected 200 with $.item [], got 400.
Cause 6: body expected {}, got {'item': []}.
Causes 7-9: $.paths./<path>.<method>.consumes, $.components, $.paths./<path>.<method>.responses.200.content and $.x-amazon-apigateway-security-policy missing; $headers.content-disposition expected 'attachment; filename="oas30_1.0.json"', got a clock-stamped filename.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Causes 1-5: GET the documentation-parts / api-keys collections with limit=1 and walk `position` to the end (apigw-envelope-documentation-parts-position-walk, apigw-envelope-api-keys-position-walk).
3. Cause 6: GET /domainnames/{domain}/basepathmappings on a domain with no mappings.
4. Causes 7-9: GET /restapis/{id}/stages/{stage}/exports/oas30 and inspect components, responses[*].content, x-amazon-apigateway-security-policy and the content-disposition header.
