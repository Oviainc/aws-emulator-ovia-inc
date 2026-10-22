Issue-Source: trace-sweep-divergence
Issue-Trace: braket-search-and-inspect-devices
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: braket-search-and-inspect-devices
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket|GetDevice|2xx->2xx|$.deviceCapabilities
Issue-Commit: f95a296744f71657ae3614221270df79d0b4a393

# Braket GetDevice deviceCapabilities omits the published device-schema properties clients read

> _Note: this issue was identified against the code base at commit `f95a296744f71657ae3614221270df79d0b4a393`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDevice

## Description
One cause. Cause 1 of 1: GetDevice on a published device returns a capability document that is structurally thinner than the one AWS publishes for that device.
- AWS: Smithy: GetDevice, GET /device/{deviceArn}, 200; output member `deviceCapabilities` (JsonValue, @mediaType("application/json"), required) documented as "Details about the capabilities of the device". The operation's own documentation states that the capability document is versioned by BraketSchemas and that OpenQASM information appears in it for recent schema versions — i.e. the document's CONTENT is part of the contract, not an opaque blob. The recording for a published simulator returns a full Braket device-capability document: a `service` section carrying `braketSchemaHeader` braket.device_schema.device_service_properties, `executionWindows`, `shotsRange`, `deviceCost` (price/unit), `deviceDocumentation` with summary plus imageUrl and externalDocumentationUrl, and `updatedAt`; and an `action` map keyed by action type (e.g. braket.ir.openqasm.program) whose entry carries `actionType`, `version` ["1.0"], `supportedOperations`, `supportedPragmas`, `forbiddenPragmas`, `maximumQubitArrays`, `maximumClassicalArrays`, `forbiddenArrayOperations` and similar. Specific prices, urls and the updatedAt timestamp are catalog values and only examples; the REQUIREMENT is that the published properties clients read — notably the per-action supported/forbidden operation and pragma lists, the action `version`, and the service-section cost/documentation properties — are present and carry the schema's property names.
- Emulator: serves a synthetic document built in services/service-com-amazonaws-braket-braket/src/state.rs (capabilities_document) from a fixed seed row. It has the schema header, `service` with executionWindows/shotsRange and a one-line generated `deviceDocumentation.summary`, an `action` map whose entries carry only `actionType` and `version` ["1"], plus a `paradigm` with `qubitCount` and a hard-coded three-gate `nativeGateSet`, and an empty `deviceParameters`. Every supported/forbidden operation and pragma list, the action version string, `deviceCost` and the documentation urls are absent; the response is 200 so the divergence is in the body only.
- Why: the Braket SDK and any client that introspects a device reads this document to decide what it may submit — which gates and pragmas are legal, which action types are accepted and at what schema version, and what a run costs. Against the emulator those lookups return nothing, so client-side program validation is silently skipped or fails to construct the device, and a program the emulator's catalog cannot vet is submitted anyway; conversely clients that key off `version` see "1" where AWS publishes "1.0" and may reject the device as an unsupported schema version.
- Fix: the seeded device catalog / state for this service — extend the capability document builder so each seeded device publishes the device-schema properties above (per-action supportedOperations/supportedPragmas/forbiddenPragmas and the action version AWS publishes, and the service-section cost and documentation properties) instead of the abbreviated stand-in.
- Traces: braket-search-and-inspect-devices#task#2 (status 200 on both sides; `$.deviceCapabilities` value_mismatch). The document is built at services/service-com-amazonaws-braket-braket/src/state.rs:540-582 from the seeds at :405-512 and returned unchanged by services/service-com-amazonaws-braket-braket/src/ops/get_device.rs:197-205.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline:

fixture braket-search-and-inspect-devices, step task#2, GetDevice on a published simulator
  status: expected 200, actual 200 (match)
  $.deviceCapabilities: value_mismatch
    expected (abridged): {"service": {"braketSchemaHeader": {"name": "braket.device_schema.device_service_properties", "version": "1"}, "executionWindows": [...], "shotsRange": [0, 100000], "deviceCost": {"price": 0.075, "unit": "minute"}, "deviceDocumentation": {"imageUrl": ..., "summary": "Amazon Braket state vector simulator", "externalDocumentationUrl": ...}, "updatedAt": ...}, "action": {"braket.ir.openqasm.program": {"version": ["1.0"], "actionType": "braket.ir.openqasm.program", "supportedOperations": ["barrier", "ccnot", "cnot", ... "ms"], "supportedPragmas": [...], "forbiddenPragmas": [...], "maximumQubitArrays": 1, "maximumClassicalArrays": 1, "forbiddenArrayOperations": [...]}}}
    actual: the emulator's seeded stand-in, which carries only braketSchemaHeader, service{executionWindows, shotsRange, deviceDocumentation.summary}, action{<type>:{actionType, version:["1"]}}, paradigm{qubitCount, nativeGateSet:["rx","rz","cz"]}, deviceParameters{} — no supportedOperations/supportedPragmas/forbiddenPragmas, no deviceCost, no documentation urls.
  divergence_key: service=braket|GetDevice|status=200->200|keys=$.deviceCapabilities

Code evidence: services/service-com-amazonaws-braket-braket/src/state.rs:540-582 (capabilities_document) is the whole document the emulator can ever serve; get_device.rs:203 returns it verbatim.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. GET /device/arn:aws:braket:::device/quantum-simulator/amazon/sv1 (fixture braket-search-and-inspect-devices, step task#2).
3. Parse the returned `deviceCapabilities` JSON string and look up action['braket.ir.openqasm.program'].supportedOperations, .supportedPragmas and .version, and service.deviceCost: the first three are missing or carry version "1" instead of "1.0", and deviceCost is absent.
4. After the fix, re-replay braket-search-and-inspect-devices#task#2; $.deviceCapabilities must carry the published device-schema properties.
