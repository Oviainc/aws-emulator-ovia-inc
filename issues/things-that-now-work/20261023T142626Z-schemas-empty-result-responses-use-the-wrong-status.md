Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-registry-resource-policy
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-registry-resource-policy schemas-schema-lifecycle
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|DeleteResourcePolicy|2xx->2xx|$headers.content-length
Issue-Key: schemas|DeleteResourcePolicy|2xx->2xx|status
Issue-Key: schemas|DeleteSchema|2xx->2xx|$headers.content-length
Issue-Commit: 17c220c4f82dc0c00315f2111baf55aa4ec61156

# schemas empty-result responses use the wrong status and omit Content-Length

> _Note: this issue was identified against the code base at commit `17c220c4f82dc0c00315f2111baf55aa4ec61156`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteResourcePolicy

## Description
Two operations that return no modelled output frame the empty response differently from AWS. This issue bundles TWO causes; both must be closed.

Cause 1 of 2: DeleteResourcePolicy answers 204 where AWS answers 200.
AWS: recorded behavior only, and it contradicts the model — Smithy declares DeleteResourcePolicy DELETE /v1/policy with output Unit and `@http(code: 204)`, but the recorded response for a successful delete is status 200 carrying a body and a Content-Length header. The wire recording is what a client's HTTP stack sees, so 200 is the requirement here.
Emulator: status 204 with no body and no Content-Length (`status: expected 200, got 204`; `$headers.content-length: missing`).
Why: an HTTP status is part of the contract. A client, proxy or test harness that distinguishes 200-with-body from 204-no-content (anything that reads the response body, caches on status, or asserts on it) takes a different path than it does against AWS, and 204 forbids the body AWS sends.
Fix: the DeleteResourcePolicy handler's success response — emit the recorded 200 framing rather than the model's declared 204 code.
Traces: schemas-registry-resource-policy#task#7.

Cause 2 of 2: DeleteSchema's 204 omits Content-Length: 0.
AWS: Smithy: DeleteSchema DELETE /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}, output Unit, `@http(code: 204)`. Recorded: status 204 with an explicit `Content-Length: 0` response header.
Emulator: status 204 is correct, but the response carries no Content-Length header at all.
Why: a client that reads Content-Length to decide the body is complete (rather than relying on connection framing) sees an unframed response, which some HTTP clients surface as a protocol error on an otherwise successful delete.
Fix: the empty-body response serializer shared by the service's no-output operations — set Content-Length: 0 on a 204.
Traces: schemas-schema-lifecycle#task#10.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, 13 fixtures, 289 steps).

Cause 1 — schemas-registry-resource-policy#task#7, DeleteResourcePolicy, expected 200 / actual 204:
  status: expected 200, got 204
  $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  divergence_key service=schemas|DeleteResourcePolicy|status=200->204|keys=$headers.content-length,status
  (Smithy model declares @http(code: 204) for this operation; the recording shows AWS answering 200 with a body, so the model and the wire disagree and the wire wins.)

Cause 2 — schemas-schema-lifecycle#task#10, DeleteSchema, expected 204 / actual 204:
  $headers.content-length: missing (expected '0', got None)
  divergence_key service=schemas|DeleteSchema|status=204->204|keys=$headers.content-length

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: fixture schemas-registry-resource-policy, step task#7 — after PutResourcePolicy has attached a registry policy, call DELETE /v1/policy?registryName=<registry> and inspect the status line and Content-Length header: emulator 204/no header, recording 200 with Content-Length.
3. Cause 2: fixture schemas-schema-lifecycle, step task#10 — call DELETE /v1/registries/name/<registry>/schemas/name/<schema> and inspect response headers: Content-Length: 0 is absent.
