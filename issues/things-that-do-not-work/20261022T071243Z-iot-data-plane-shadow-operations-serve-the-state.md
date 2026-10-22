Issue-Source: trace-sweep-divergence
Issue-Trace: iotdata-classic-and-named-shadow-document-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotdata-classic-and-named-shadow-document-lifecycle iotdata-shadow-update-document-guards
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data|UpdateThingShadow|2xx->2xx|$headers.content-type
Issue-Key: iot-data|GetThingShadow|2xx->2xx|$headers.content-type
Issue-Key: iot-data|DeleteThingShadow|2xx->2xx|$headers.content-type
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# IoT Data Plane shadow operations serve the state document as application/octet-stream instead of application/json

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetThingShadow

## Description
All three shadow-document operations return the correct HTTP 200 and the correct state document, but label it with the wrong media type: `content-type: application/octet-stream` instead of `application/json`. This is a single defect in how the shadow payload is serialized, reproduced once per operation. This issue bundles THREE causes; all three must be closed.

Shared AWS behavior: the shadow operations bind their output to `payload`, a `@httpPayload` blob of type `JsonDocument` (Smithy: UpdateThingShadow / GetThingShadow / DeleteThingShadow -> output member `payload` target JsonDocument, @httpPayload, type blob; JsonDocument is `blob` with no `@mediaType`). Because the blob has no mediaType trait, restJson1's default for a bound blob payload is `application/octet-stream` — but the real data plane returns the shadow document as `application/json`, which is what every recording in the corpus shows. So the media type here is recorded behavior, and the recording is unanimous across all three operations and both classic and named shadows.

Cause 1 of 3 — UpdateThingShadow (200).
AWS: 200 with `content-type: application/json` carrying the merged state document.
Emulator: 200 with `content-type: application/octet-stream`; body matches.
Why: see shared impact below.
Fix: the UpdateThingShadow response serializer (the payload-blob content-type decision).
Traces: iotdata-classic-and-named-shadow-document-lifecycle#task#1 (also #task#3, iotdata-shadow-update-document-guards#task#1, #task#2).

Cause 2 of 3 — GetThingShadow (200).
AWS: 200 with `content-type: application/json` carrying the stored state document.
Emulator: 200 with `content-type: application/octet-stream`.
Fix: the GetThingShadow response serializer.
Traces: iotdata-classic-and-named-shadow-document-lifecycle#task#2 (also #task#7, iotdata-shadow-update-document-guards#task#7).

Cause 3 of 3 — DeleteThingShadow (200).
AWS: 200 with `content-type: application/json` carrying the delete acknowledgement document.
Emulator: 200 with `content-type: application/octet-stream`.
Fix: the DeleteThingShadow response serializer.
Traces: iotdata-classic-and-named-shadow-document-lifecycle#task#5 (also #task#8, iotdata-shadow-update-document-guards#task#8).

Shared Why: clients and proxies dispatch on content-type. An HTTP client, test harness or gateway that decodes `application/json` automatically will hand back raw bytes for a shadow read, and anything that content-negotiates or logs/transforms JSON bodies treats the shadow document as opaque binary. The defect is uniform, so it affects every shadow interaction rather than one edge case.

Fix location: the shared payload-blob response path for the shadow operations in services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops — it must pin `application/json` for JsonDocument payloads rather than fall back to the protocol's generic blob default. No existing open issue covers iot-data.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up). Three divergence clusters, identical detail, statuses all correct (200 expected, 200 actual):

1) service=iot-data | UpdateThingShadow | status=200->200 | keys=$headers.content-type — "$headers.content-type: value_mismatch (expected 'application/json', got 'application/octet-stream')". Cluster members: iotdata-classic-and-named-shadow-document-lifecycle#task#1, #task#3, iotdata-shadow-update-document-guards#task#1, #task#2 (4 affected).

2) service=iot-data | GetThingShadow | status=200->200 | keys=$headers.content-type — same expected/actual. Cluster members: iotdata-classic-and-named-shadow-document-lifecycle#task#2, #task#7, iotdata-shadow-update-document-guards#task#7 (3 affected).

3) service=iot-data | DeleteThingShadow | status=200->200 | keys=$headers.content-type — same expected/actual. Cluster members: iotdata-classic-and-named-shadow-document-lifecycle#task#5, #task#8, iotdata-shadow-update-document-guards#task#8 (3 affected).

Only the content-type header diverges; the response bodies and statuses matched, so the finding is the media type alone.

## Steps to Reproduce
1. git clone the emulator repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. Representative step: fixture iotdata-classic-and-named-shadow-document-lifecycle, step task#1 — POST /things/{thingName}/shadow with a state document. Inspect the 200 response headers: `content-type: application/octet-stream`.
4. Re-replay iotdata-classic-and-named-shadow-document-lifecycle#task#2 (GET /things/{thingName}/shadow) and #task#5 (DELETE /things/{thingName}/shadow) and inspect the same header; both are also application/octet-stream where the fixture records application/json. The named-shadow variants (same operations with ?name=<shadowName>) behave identically.
