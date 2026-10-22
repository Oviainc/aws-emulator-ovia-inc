Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-discover-schema-from-events
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-discover-schema-from-events schemas-schema-search-and-list schemas-registry-resource-policy schemas-schema-lifecycle
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|GetDiscoveredSchema|2xx->2xx|$.Content
Issue-Key: schemas|SearchSchemas|2xx->2xx|$.Schemas[*]
Issue-Key: schemas|GetResourcePolicy|2xx->2xx|$.Policy
Issue-Key: schemas|DeleteSchema|2xx->2xx|$headers.content-length
Issue-Commit: e556c3079ace9bb74a6c235df6f1f92b13f370bd

# schemas discovery, search and policy reads return bodies and headers unlike AWS

> _Note: this issue was identified against the code base at commit `e556c3079ace9bb74a6c235df6f1f92b13f370bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDiscoveredSchema

## Description
Four smaller payload/header divergences on otherwise-successful responses. This bundle carries FOUR causes and all four must be closed.

Cause 1 of 4 — GetDiscoveredSchema infers a different OpenAPI document (fixture schemas-discover-schema-from-events, step task#1).
  AWS: recorded behavior (the inference algorithm is not modelled; the model only says output.Content is a string). For an event whose `detail` is a nested object, AWS hoists the nested object into its own named component and references it: components.schemas.Event.properties.detail = {"$ref": "#/components/schemas/Detail"} with a sibling `Detail` component, Event.required listed as ["name","detail","id"], and an integral JSON value typed `"type":"number"`.
  Emulator: inlines the nested object inside Event.properties.detail (no `Detail` component, no $ref) and types the integral value as `"type":"integer"`.
  Why: the document is the product of the operation — clients feed it to CreateSchema and to code-binding generation, so a client gets different generated types (and no reusable Detail type) than it would from AWS.
  Fix: the GetDiscoveredSchema inference/serialization logic (hoist nested objects to named components with $ref; map JSON numbers to `number`).

Cause 2 of 4 — SearchSchemas returns a schema AWS does not match (fixture schemas-schema-search-and-list, step task#6).
  AWS: recorded behavior — for the recorded keyword, 200 with an empty Schemas list. Smithy: SearchSchemas GET .../schemas/search, Keywords required, output __listOfSearchSchemaSummary, paginated.
  Emulator: 200 with one SearchSchemaSummary (the probe schema in the searched registry), i.e. the keyword match is broader than AWS's (it matches the schema even though AWS's keyword search does not).
  Why: search results are the client's selection step; extra matches make keyword search non-discriminating and any assertion on result-set size wrong.
  Fix: the SearchSchemas handler's keyword-matching predicate.

Cause 3 of 4 — GetResourcePolicy re-serializes the policy document instead of returning it as stored (fixture schemas-registry-resource-policy, step task#4).
  AWS: returns the policy document as it was supplied to PutResourcePolicy — recorded as the compact form `{"Version":"2012-10-17","Statement":[{...}]}`. Smithy: GetResourcePolicyResponse.Policy targets SynthesizedJson__string with @mediaType("application/json"), i.e. an opaque JSON document, not a re-encoded structure.
  Emulator: returns a re-encoded document with spaces after `:` and `,` (`{"Version": "2012-10-17", "Statement": [{"Sid": ...`). Same semantics, different bytes.
  Why: callers that hash, diff or byte-compare a policy round-trip (a standard way to detect drift) see a change on every read.
  Fix: the resource-policy state / GetResourcePolicy serializer — store and return the document verbatim rather than parsing and re-emitting it.

Cause 4 of 4 — DeleteSchema's 204 omits Content-Length (fixture schemas-schema-lifecycle, step task#10).
  AWS: 204 with `content-length: 0`. Smithy: DeleteSchema -> Unit, http code 204.
  Emulator: 204 with no Content-Length header at all.
  Why: clients and proxies that require a framing header on an empty response see a differently framed reply than AWS sends.
  Fix: the DeleteSchema response serializer / the empty-body path of the HTTP layer for this operation.

Traces: schemas-discover-schema-from-events#task#1 (representative), schemas-schema-search-and-list#task#6, schemas-registry-resource-policy#task#4, schemas-schema-lifecycle#task#10.

## Evidence
Replay of simparity-wire-aws-v1:
- schemas-discover-schema-from-events#task#1 GetDiscoveredSchema 200->200: $.Content value_mismatch — expected ..."Event":{..."required":["name","detail","id"],"properties":{"detail":{"$ref":"#/components/schemas/Detail"}...}},"Detail":{..."count":{"type":"number"}...} ; got ..."Event":{..."properties":{"detail":{"type":"object",..."count":{"type":"integer"}...}}} with no Detail component
- schemas-schema-search-and-list#task#6 SearchSchemas 200->200: $.Schemas[0] extra (expected None, got {'RegistryName': 'sim2real-probe-schemas-searchreg-...', 'SchemaName': 'sim2real-probe-schemas-apple-...', 'SchemaVersions': [{'SchemaVersion': '1', 'Type': 'OpenApi3'}]})
- schemas-registry-resource-policy#task#4 GetResourcePolicy 200->200: $.Policy value_mismatch — expected '{"Version":"2012-10-17","Statement":[{"Sid":"AllowDescribe",...}]}', got '{"Version": "2012-10-17", "Statement": [{"Sid": "AllowDescribe", ...}]}'
- schemas-schema-lifecycle#task#10 DeleteSchema 204->204: $headers.content-length missing (expected '0')
Model evidence: smithy_query(query="operation", operation="GetDiscoveredSchema,SearchSchemas,GetResourcePolicy,DeleteSchema") — GetDiscoveredSchema output is an opaque Content string, GetResourcePolicy.Policy is SynthesizedJson__string @mediaType application/json, DeleteSchema returns Unit at 204.

## Steps to Reproduce
1. Replay fixture schemas-discover-schema-from-events step task#1: POST /v1/discover with an event whose detail is a nested object and compare $.Content — the emulator inlines the nested object and types the integral field as integer.
2. Replay fixture schemas-schema-search-and-list step task#6: SearchSchemas with the recorded keyword returns one schema where AWS returns none.
3. Replay fixture schemas-registry-resource-policy step task#4: GetResourcePolicy returns the policy re-serialized with inserted whitespace rather than the bytes supplied to PutResourcePolicy.
4. Replay fixture schemas-schema-lifecycle step task#10: DeleteSchema's 204 response carries no content-length header.
