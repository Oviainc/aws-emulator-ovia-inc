Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-documentation-part-resource-and-method-locations
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-documentation-part-resource-and-method-locations apigateway-documentation-parts-multiple-locations-and-filtering apigateway-import-documentation-parts
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetDocumentationParts|2xx->2xx|$.item[id="8ib9n34r0l"]
Issue-Key: apigateway|GetDocumentationParts|2xx->2xx|$.item[id="i5yj7gymiu"]
Issue-Key: apigateway|GetDocumentationParts|2xx->2xx|$.item[id="nya7y5ckrg"]
Issue-Key: apigateway|ImportDocumentationParts|2xx->2xx|$.ids[*]
Issue-Commit: 3971b4aa426682b1b07d577a2cfc2e75e38df2fe

# API Gateway documentation parts: METHOD-location parts dropped from listings, type filter ignored, and merge import mints new ids (3 causes)

> _Note: this issue was identified against the code base at commit `3971b4aa426682b1b07d577a2cfc2e75e38df2fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDocumentationParts

## Description
Three net-new root causes in the API Gateway REST documentation-part subsystem. All three are apigateway and all three concern how documentation parts are stored, filtered and merged.

Cause 1 of 3: GetDocumentationParts omits documentation parts whose location.type is METHOD.
AWS: Smithy: GetDocumentationParts -> DocumentationParts, member `items` (jsonName "item") is a ListOfDocumentationPart; DocumentationPart carries {id, location, properties}; DocumentationPartLocation.type is the required DocumentationPartType enum, whose values include METHOD, and location.method is documented as "The HTTP verb of a method. It is a valid field for the API entity types of METHOD, ...". A documentation part created for a METHOD location (location {type: METHOD, path: <resource path>, method: <verb>}) is therefore a first-class part and MUST appear in the restApi's GetDocumentationParts listing, carrying the id minted at creation, its full location (type/path/method) and the JSON-string `properties` it was created with.
Emulator: for the same restApiId the listing comes back WITHOUT the METHOD-location part: the step's expected `$.item[...]` entry — a part whose location is {method: <verb>, path: <resource path>, type: METHOD} with its `properties` JSON string — is reported `missing (... got None)`, while the rest of the listing matches. Status is 200 either way, so a client sees a successful, silently incomplete listing.
Why: a client that creates documentation for a method and then reads the API's documentation parts back (to render docs, to diff before ImportDocumentationParts, or to drive GetExport/OpenAPI publication) never sees that documentation. The part appears to have been accepted at create time and then to have vanished, so documentation for every method of the API is lost with no error to react to.
Fix: the documentation-part state/index behind GetDocumentationParts for the REST API — METHOD-location parts must be retained in, and enumerated from, the same per-restApi collection as RESOURCE/MODEL parts.
Traces: apigateway-documentation-part-resource-and-method-locations#task#11.

Cause 2 of 3: GetDocumentationParts does not apply its location filter, so a filtered listing returns the wrong set.
AWS: Smithy: GetDocumentationParts input binds `type` (DocumentationPartType, httpQuery "type"), `nameQuery` (httpQuery "name"), `path` (httpQuery "path") and `locationStatus` (httpQuery "locationStatus"). Those query parameters SELECT which parts are returned: a listing filtered to a location must contain exactly the parts matching that filter, and must exclude parts of other location types for the same API.
Emulator: for a restApi carrying parts at several locations, the filtered listing is both short and wrong — the expected METHOD-location part (location {method: <verb>, path: <resource path>, type: METHOD}) is `missing`, while a MODEL-location part (location {type: MODEL, name: <model name>}) that the filter excludes is returned as `extra`. Status 200. The combination shows the filter query parameters being ignored and the result being drawn from an unfiltered collection that is itself missing the METHOD part (cause 1).
Why: a client that uses `type=`/`path=` to fetch the documentation for one entity gets documentation belonging to a different entity and silently loses the documentation it asked for. Any tooling that pages documentation per location (or asserts a location has no documentation) draws the opposite conclusion from the real service.
Fix: the GetDocumentationParts handler's query-parameter filtering, together with the documentation-part state it reads.
Traces: apigateway-documentation-parts-multiple-locations-and-filtering#task#13.

Cause 3 of 3: ImportDocumentationParts does not merge into the existing part, it creates a new one and returns a new id.
AWS: Smithy: ImportDocumentationParts input binds `mode` to the PutMode enum (values `merge`, `overwrite`) and returns DocumentationPartIds {ids: ListOfString, warnings: ListOfString}. A merge import of documentation whose location already has a documentation part updates THAT part: the id reported in `ids` is the id of the already-existing part for that location, not a freshly minted one. (The per-location identity rule is recorded behavior here — the model names the merge mode but does not spell out id reuse — and the fixture asserts the previously minted id by reference, not as a literal.)
Emulator: `$.ids[0]` is a brand-new generated documentation-part id instead of the id of the part that the earlier step of the same fixture created for that location; status 200. The import therefore produced a second part for the same location rather than merging.
Why: a client that re-imports an OpenAPI documentation body (the normal way documentation is kept in sync from source control) accumulates duplicate parts per location on every import instead of updating in place, and cannot correlate the returned ids with the parts it already tracks. Note: the returned id value itself is per-run and is NOT the requirement — the requirement is that it EQUALS the existing part's id for that location.
Why-not-related: the open issue 20261103T120000Z-apigateway-importdocumentationparts-failonwarnings-refusal-sentence.md covers ImportDocumentationParts' failOnWarnings REFUSAL wording only; it does not cover merge-mode part identity, so this cause is net-new.
Fix: the ImportDocumentationParts handler's merge path and the documentation-part state it writes (match on location, update in place, report the existing id).
Traces: apigateway-import-documentation-parts#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture apigateway-documentation-part-resource-and-method-locations, step task#11, GetDocumentationParts, expected 200 / actual 200:
  $.item[<minted-id>]: missing (expected {'id': <minted-id>, 'location': {'method': 'GET', 'path': '/widgets', 'type': 'METHOD'}, 'properties': '{"description": "List widgets"}'}, got None)

Cause 2 — fixture apigateway-documentation-parts-multiple-locations-and-filtering, step task#13, GetDocumentationParts, expected 200 / actual 200:
  $.item[<minted-id-A>]: missing (expected {'id': <minted-id-A>, 'location': {'method': 'GET', 'path': '/pets', 'type': 'METHOD'}, 'properties': '{"description": "list pets"}'}, got None)
  $.item[<minted-id-B>]: extra (expected None, got {'id': <minted-id-B>, 'location': {'type': 'MODEL', 'name': <model name>}, 'properties': '{"description": "pet model"}'})

Cause 3 — fixture apigateway-import-documentation-parts, step task#2, ImportDocumentationParts, expected 200 / actual 200:
  $.ids[0]: value_mismatch (expected the documentation-part id minted by the earlier step for the same location, got a newly generated id)

(The literal id strings in the recordings are per-run values and are not part of the requirement; the requirement in cause 3 is that ids[0] equals the pre-existing part's id, and in causes 1-2 that the METHOD part is present and the location filter is honoured.)

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Cause 1: CreateRestApi, create a resource and a method on it, then CreateDocumentationPart with location {type: METHOD, path: <resource path>, method: <verb>} and a properties JSON string. Call GetDocumentationParts for that restApiId — the METHOD part is absent from `item`.
3. Cause 2: on one restApi create documentation parts at several locations (e.g. a METHOD location and a MODEL location), then call GetDocumentationParts with the location filter query parameters (type=/path=). The response contains the non-matching MODEL part and omits the matching METHOD part.
4. Cause 3: CreateDocumentationPart for a location and note the returned id; then call ImportDocumentationParts with mode=merge and a body describing documentation for that same location. `ids[0]` is a new id instead of the noted one, and the API ends up with two parts for the location.
5. Or replay the three fixture steps directly: apigateway-documentation-part-resource-and-method-locations#task#11, apigateway-documentation-parts-multiple-locations-and-filtering#task#13, apigateway-import-documentation-parts#task#2.
