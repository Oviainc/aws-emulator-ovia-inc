Issue-Source: trace-sweep-divergence
Issue-Trace: iot-managed-integrations-managed-thing-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-managed-integrations-managed-thing-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations|UpdateManagedThing|2xx->2xx|$headers.content-length
Issue-Commit: dd79a542b09a035cf711a2d0c52670e9a3e4e70a

# UpdateManagedThing 204 responses omit the Content-Length: 0 header

> _Note: this issue was identified against the code base at commit `dd79a542b09a035cf711a2d0c52670e9a3e4e70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateManagedThing

## Description
Cause 1 of 1: the empty 204 success from UpdateManagedThing is emitted with no Content-Length header, so the bodyless response is not framed the way the service frames it.

  AWS: Smithy: UpdateManagedThing is PUT /managed-things/{Identifier} with http code 204 and output shape Unit (output members: none), i.e. an always-empty success body; errors are the service-common set (ValidationException 400, ResourceNotFoundException 404, ...). The model does not state response headers, so this is recorded behavior only: the recorded 204 for the empty-body success carries `content-length: 0`. The same shape applies to the service's other Unit-output/204 operations (e.g. DeleteManagedThing, DELETE /managed-things/{Identifier}, code 204, output Unit).
  Emulator: returns the correct status 204, but emits no content-length header at all (replay detail: "$headers.content-length: missing (expected '0', got None)").
  Why: on a response with no body, Content-Length: 0 is what tells a client the message is complete. HTTP stacks and proxies that key framing off Content-Length on a bodyless, non-chunked response can block waiting for a body or treat the framing as indeterminate, so an update that succeeded can surface to the caller as a read timeout or a connection-reuse failure rather than a success. Callers byte-comparing or header-asserting a 204 also see a response that is not shaped like the service's for an otherwise identical success.
  Fix: UpdateManagedThing's response serializer — set Content-Length: 0 when serializing an empty (Unit-output) 204; apply it to the service's other Unit-output/204 operations rather than only this one handler.
  Traces: iot-managed-integrations-managed-thing-lifecycle#task#4 (cluster size 1).

Note on prior art: resolved issue 20261022T071104Z-iot-managed-integrations-managedthing-responses-drop-stored-state-and-required-headers bundled this header as its "Cause 3 of 3" together with two state-layer causes. The state causes no longer reproduce (GetManagedThing/ListManagedThings now return the stored attributes and the summary entry), but this header cause still does at the same fixture step, so it is re-filed here as a standalone defect; the resolved issue does not cover it.

## Evidence
Fixture iot-managed-integrations-managed-thing-lifecycle, step task#4 (UpdateManagedThing, PUT /managed-things/{Identifier}).
Expected: HTTP 204, header content-length: 0, empty body.
Actual:   HTTP 204, no content-length header at all, empty body.
Replay verdict: mismatch; detail "$headers.content-length: missing (expected '0', got None)". Status matches (204 -> 204); the only divergence is the absent header, affected_count 1.
Model citations (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json): UpdateManagedThing http {method PUT, uri /managed-things/{Identifier}, code 204}, output_shape Unit, idempotent; DeleteManagedThing http {method DELETE, uri /managed-things/{Identifier}, code 204}, output_shape Unit.
The header value 0 is the only value a bodyless response can carry, so it is environment-independent; no generated id, host or timestamp is being asserted here.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture iot-managed-integrations-managed-thing-lifecycle.
2. setup#1 creates a managed thing; note the returned Id.
3. Step task#4 issues PUT /managed-things/{Id} with an update body.
4. Inspect the response headers: the emulator answers 204 with no Content-Length header; the recorded service response carries Content-Length: 0.
5. Equivalent check outside the corpus: PUT /managed-things/{Id} (or DELETE /managed-things/{Id}) against the emulator and dump raw response headers — no content-length is present on the empty 204.
