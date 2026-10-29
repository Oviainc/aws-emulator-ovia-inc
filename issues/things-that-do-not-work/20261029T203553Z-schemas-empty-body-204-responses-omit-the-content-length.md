Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-registry-tagging
Issue-Step: task#5
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-registry-tagging schemas-schema-lifecycle
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|UntagResource|2xx->2xx|$headers.content-length
Issue-Key: schemas|DeleteSchemaVersion|2xx->2xx|$headers.content-length
Issue-Key: schemas|DeleteSchema|2xx->2xx|$headers.content-length
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# Empty-body 204 responses omit the Content-Length: 0 header AWS sends

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UntagResource

## Description
Every operation this service models with an empty output and a 204 status answers without a Content-Length header, where AWS sends 'Content-Length: 0'. The same framing gap shows up on three operations in this corpus; it is one defect in how an empty 204 response is framed, and each of the three must be re-replayed.

AWS: Smithy models these operations with output_shape Unit and http code 204 - UntagResource DELETE /tags/{ResourceArn} (204), DeleteSchemaVersion DELETE /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}/version/{SchemaVersion} (204), DeleteSchema DELETE /v1/registries/name/{RegistryName}/schemas/name/{SchemaName} (204). The recording shows each 204 carrying the header content-length with the fixed value '0'; there is no variable part to this value.

Emulator: the status is right (204 expected, 204 actual) and the body is empty, but the response carries no content-length header at all - the replay reports $headers.content-length: missing (expected '0', got None) for all three.

Why: an HTTP client that frames a response by its declared length, or a proxy that normalises or counts bytes, sees an unframed empty response rather than an explicitly zero-length one. Clients differ in how they treat a 204 with neither Content-Length nor Transfer-Encoding, and some log it or retry it as a truncated response, so the emulator's responses are not byte-interchangeable with AWS's for the whole class of empty 204 replies - including the ones this corpus does not reach (DeleteRegistry, DeleteDiscoverer, DeleteResourcePolicy).

Fix: the response serializer for an operation whose output shape is Unit - it must emit Content-Length: 0 on the empty 204 rather than leaving the header off. One edit in the shared empty-response path closes all three causes.

Cause 1 of 3: UntagResource - fixture schemas-registry-tagging, step task#5.
Cause 2 of 3: DeleteSchemaVersion - fixture schemas-schema-lifecycle, step task#8.
Cause 3 of 3: DeleteSchema - fixture schemas-schema-lifecycle, step task#10.

## Evidence
Replay results, service=schemas, all 'expected 204 / actual 204' with detail '$headers.content-length: missing (expected '0', got None)':
- UntagResource, fixture schemas-registry-tagging step task#5 (divergence_key service=schemas|UntagResource|status=204->204|keys=$headers.content-length).
- DeleteSchemaVersion, fixture schemas-schema-lifecycle step task#8 (divergence_key service=schemas|DeleteSchemaVersion|status=204->204|keys=$headers.content-length).
- DeleteSchema, fixture schemas-schema-lifecycle step task#10 (divergence_key service=schemas|DeleteSchema|status=204->204|keys=$headers.content-length).

Smithy (services/service-com-amazonaws-schemas-schemas/smithy-model/com-amazonaws-schemas-schemas.json): UntagResource output_shape Unit, http {method: DELETE, uri: /tags/{ResourceArn}, code: 204}; DeleteSchemaVersion output_shape Unit, http code 204; DeleteSchema output_shape Unit, http code 204. No member is bound to a header on any of the three, so the header is the framing of the empty body and not a modelled output member.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture schemas-registry-tagging step task#5: DELETE /tags/<registry ARN>?tagKeys=<key>. Inspect the response headers: status is 204 and no content-length header is present; AWS sends content-length: 0.
3. Replay fixture schemas-schema-lifecycle step task#8 (DELETE .../schemas/name/<name>/version/<n>) and step task#10 (DELETE .../schemas/name/<name>) and observe the same missing header.
4. After the fix, re-replay all three steps - they are served by three different handlers over one empty-204 response path.
