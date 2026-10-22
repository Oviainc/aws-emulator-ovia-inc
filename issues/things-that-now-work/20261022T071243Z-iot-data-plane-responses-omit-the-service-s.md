Issue-Source: trace-sweep-divergence
Issue-Trace: iotdata-classic-and-named-shadow-document-lifecycle
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotdata-classic-and-named-shadow-document-lifecycle iotdata-shadow-update-document-guards iotdata-publish-topic-name-guards iotdata-retained-message-publish-read-and-clear
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data|GetThingShadow|4xx->4xx|$.message
Issue-Key: iot-data|GetThingShadow|4xx->4xx|$.traceId
Issue-Key: iot-data|Publish|2xx->2xx|$
Issue-Key: iot-data|Publish|4xx->4xx|$.message
Issue-Key: iot-data|Publish|4xx->4xx|$.traceId
Issue-Key: iot-data|GetRetainedMessage|4xx->4xx|$.message
Issue-Key: iot-data|GetRetainedMessage|4xx->4xx|$.traceId
Issue-Key: iot-data|UpdateThingShadow|4xx->4xx|$.message
Issue-Key: iot-data|UpdateThingShadow|4xx->4xx|$.traceId
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# IoT Data Plane responses omit the service's message/traceId envelope and rebuild error text from internal state

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateThingShadow

## Description
The IoT Data Plane emulator builds its own response bodies for both failures and the Publish success case. Two things go wrong in every one of them: (a) the `traceId` member that the real data plane returns alongside `message` is never emitted at all, and (b) where AWS returns a fixed, service-authored message string, the emulator substitutes prose rebuilt from its own internal state (thing name, stored version, byte counts, serde parser text). HTTP statuses are correct throughout, so this is a response-serialization defect, not a routing or state-machine defect. This issue bundles SEVEN causes; all seven must be closed.

Cause 1 of 7 — GetThingShadow, missing shadow (404).
AWS: restJson1 ResourceNotFoundException, HTTP 404 (Smithy: GetThingShadow errors ResourceNotFoundException @error client, @httpError 404; members: `message` -> errorMessage). The model declares only `message`, so the `traceId` sibling is recorded behavior only: the recording shows the data plane returning BOTH `message` and a `traceId` string on every 404. The message follows the fixed shadow-not-found template `No shadow exists with name: '<name>'`.
Emulator: status 404 and the error type are right, but the body carries `message` = "no shadow exists for thing '<thingName>'" (lower-case, emulator-authored, keyed on the thing rather than the shadow name) and no `traceId` member at all.
Why: a client that reads `traceId` to correlate a failed shadow read with service-side logs gets `None` and either logs nothing or throws on the missing key; a client that matches the documented shadow error text does not recognise the condition.
Fix: the error serializer for the shadow operations — the shared ResourceNotFoundException rendering path, which must emit the `{message, traceId}` envelope and use the service's fixed template instead of an internally composed sentence.
Traces: iotdata-classic-and-named-shadow-document-lifecycle#task#9 (also iotdata-shadow-update-document-guards#task#9).

Cause 2 of 7 — Publish, success (200).
AWS: Smithy models Publish output as `Unit` (Publish -> output Unit, http POST /topics/{topic}, code 200), so the body is recorded behavior only: the recording shows a 200 carrying a JSON object with `message` (example value "OK") and a `traceId`.
Emulator: 200 with an entirely empty JSON body `{}` — no `message`, no `traceId`.
Why: callers that parse the publish acknowledgement to confirm the broker accepted the message, or that capture `traceId` for correlation, see an empty object and cannot distinguish an accepted publish from a response they failed to parse.
Fix: the Publish handler's success serializer, which must render the acknowledgement envelope rather than an empty object.
Traces: iotdata-publish-topic-name-guards#task#1 (also iotdata-retained-message-publish-read-and-clear#task#1, #task#4).

Cause 3 of 7 — Publish, MQTT wildcard in the publish topic (400).
AWS: InvalidRequestException, HTTP 400 (Smithy: Publish errors include InvalidRequestException @error client, @httpError 400 via the service's common errors). AWS rejects a publish topic containing an MQTT wildcard with the fixed string `Invalid topic filter` plus a `traceId` (recorded behavior for the traceId member).
Emulator: status 400 is correct, but `message` is a generated sentence that echoes the request topic and explains the wildcard rule, and `traceId` is absent.
Why: the rejection reason is no longer the service's stable, matchable string, and the correlation id is gone.
Fix: Publish's topic validation error construction — emit the service's fixed rejection message and the envelope's `traceId`.
Traces: iotdata-publish-topic-name-guards#task#2.

Cause 4 of 7 — GetRetainedMessage, no retained message for topic (404).
AWS: ResourceNotFoundException, HTTP 404 (Smithy: GetRetainedMessage errors ResourceNotFoundException @httpError 404). Fixed message `Retained message not found`, plus a `traceId` (recorded behavior; the recording does not assert the id's value, only its presence and shape).
Emulator: 404 with `message` = "no retained message exists for topic '<topic>'" and no `traceId`.
Why: same as cause 1 — a missing envelope member and a non-service message string on the primary negative path of the retained-message API.
Fix: the GetRetainedMessage handler's not-found error construction / the shared error serializer.
Traces: iotdata-retained-message-publish-read-and-clear#task#5.

Cause 5 of 7 — UpdateThingShadow, version conflict (409).
AWS: ConflictException, HTTP 409 (Smithy: UpdateThingShadow errors ConflictException @error client @httpError 409, doc "The specified version does not match the version of the document"; members: `message`). AWS answers the fixed string `Version conflict` and a `traceId` (recorded behavior).
Emulator: 409 is correct, but `message` discloses internal document state ("the update expects version N but the document is at version M") and `traceId` is absent.
Why: the optimistic-concurrency failure no longer presents AWS's stable conflict text, and the correlation id is missing; a client asserting on the conflict message mis-classifies the retryable conflict.
Fix: UpdateThingShadow's version-check error construction.
Traces: iotdata-shadow-update-document-guards#task#3.

Cause 6 of 7 — UpdateThingShadow, malformed state document (400).
AWS: InvalidRequestException, HTTP 400 (Smithy: UpdateThingShadow.payload -> JsonDocument @httpPayload @required; InvalidRequestException @httpError 400). The document-parse rejection is the fixed string `Payload contains invalid json` plus a `traceId` (recorded behavior).
Emulator: 400 with the underlying parser's diagnostic leaked into `message` ("the state document is not valid JSON: expected value at line 1 column 1") and no `traceId`.
Why: the response exposes the emulator's JSON library rather than the service contract, and omits the correlation id.
Fix: UpdateThingShadow's payload validation — map any parse failure onto the service's fixed invalid-json rejection.
Traces: iotdata-shadow-update-document-guards#task#4 (also #task#5).

Cause 7 of 7 — UpdateThingShadow, state document over the size limit (413).
AWS: RequestEntityTooLargeException, HTTP 413 (Smithy: UpdateThingShadow errors RequestEntityTooLargeException @error client @httpError 413, doc "The payload exceeds the maximum size allowed"). Here the message IS built from the request, by the template `Json document too large.  Size is <actualBytes>, max allowed: 8192` (note the two spaces after the period), and the body also carries a `traceId` (recorded behavior).
Emulator: 413 is correct, and the limit it enforces is 8192, but it answers its own sentence ("the state document is N bytes; the maximum state document size is 8192 bytes") with no `traceId`. The recorded and emulated byte counts also differ for the same fixture document, which suggests the emulator measures a different quantity (e.g. the decoded document rather than the request entity) — Hypothesis: the size that AWS reports is the request payload as received, so the counted span differs.
Why: a client that parses the reported size/limit out of the documented template to decide how much to trim gets no match, and loses the correlation id.
Fix: UpdateThingShadow's size guard — render the documented size template from the request's own byte count and emit the envelope's `traceId`.
Traces: iotdata-shadow-update-document-guards#task#6.

All seven share one root cause in the response-building layer of services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops (the per-operation handlers) and the crate's error rendering: the `{message, traceId}` envelope is never produced, and messages are composed locally instead of taken from the service's fixed templates. No existing open issue covers iot-data; a search of the open issue list for "iot-data" returns nothing.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200). Per-cause expected vs actual, verbatim from the replay divergence details:

1) GetThingShadow 404 -> 404, keys $.message,$.traceId: $.message expected "No shadow exists with name: 'sim2real-probe-iotdata-shadow-f695e9dc'", got "no shadow exists for thing 'sim2real-probe-iotdata-shadow-f695e9dc'"; $.traceId missing (expected 'bfea4c1b-a889-2faf-51d2-3c99f0a21707', got None). Cluster also covers iotdata-shadow-update-document-guards#task#9.

2) Publish 200 -> 200, key $: empty_body — expected {'message': 'OK', 'traceId': 'c34a992b-7225-2be5-cf58-c2513a564c9a'}, got {}. Cluster also covers iotdata-retained-message-publish-read-and-clear#task#1 and #task#4.

3) Publish 400 -> 400, keys $.message,$.traceId: $.message expected 'Invalid topic filter', got "the topic 'sim2real-probe-iotdata-9760ab49/#' contains the MQTT wildcard '#', which a publish topic may not use"; $.traceId missing (expected '056b91d7-6c0f-1ea4-a6d4-a48121340ea4', got None).

4) GetRetainedMessage 404 -> 404, keys $.message,$.traceId: $.message expected 'Retained message not found', got "no retained message exists for topic 'sim2real-probe-iotdata-5f042158/retained'"; $.traceId missing (expected '<shaped-not-asserted>', got None).

5) UpdateThingShadow 409 -> 409, keys $.message,$.traceId: $.message expected 'Version conflict', got 'the update expects version 1 but the document is at version 2'; $.traceId missing (expected '3dee7657-57bd-cbed-b2b9-0c1f1060dd3d', got None).

6) UpdateThingShadow 400 -> 400, keys $.message,$.traceId: $.message expected 'Payload contains invalid json', got 'the state document is not valid JSON: expected value at line 1 column 1'; $.traceId missing (expected '46b04028-4952-5ed8-8548-151072d23f11', got None). Cluster also covers iotdata-shadow-update-document-guards#task#5.

7) UpdateThingShadow 413 -> 413, keys $.message,$.traceId: $.message expected 'Json document too large.  Size is 8311, max allowed: 8192', got 'the state document is 8336 bytes; the maximum state document size is 8192 bytes'; $.traceId missing (expected 'cc5887e4-126a-ceda-c4fe-406107550f97', got None).

The quoted ids, thing names, topics and byte counts are examples from these recordings, not requirements; the requirements are the presence of the `traceId` member in the envelope and the message rule stated per cause. Statuses (404/200/400/404/409/400/413) matched in all seven and are therefore not in dispute.

## Steps to Reproduce
1. git clone the emulator repo and boot the emulator built from it (health: GET /__admin/health -> 200).
2. Replay trace set simparity-wire-aws-v1 against it.
3. Representative step: fixture iotdata-classic-and-named-shadow-document-lifecycle, step task#9 — GET /things/{thingName}/shadow for a thing whose shadow was just deleted. Observe HTTP 404 whose JSON body has no `traceId` and a lower-case emulator-authored `message`.
4. Re-replay the other six causes: iotdata-publish-topic-name-guards#task#1 (POST /topics/{topic} success, body {}), iotdata-publish-topic-name-guards#task#2 (publish topic containing '#'), iotdata-retained-message-publish-read-and-clear#task#5 (GET /retainedMessage/{topic} after a clear), iotdata-shadow-update-document-guards#task#3 (UpdateThingShadow with a stale `version`), #task#4 (UpdateThingShadow with a non-JSON payload), #task#6 (UpdateThingShadow with a state document above 8192 bytes).
5. In each response, diff the JSON body against the fixture: every failure body is missing `traceId`, and each `message` is locally composed rather than the service's template.

## Resolution note

All seven causes are answered in `services/service-com-amazonaws-iotdataplane-iotmoonrakerservice`: the envelope's `traceId` is minted per call (`state::new_trace_id`), and each `message` comes from the service's message catalog in `state.rs` instead of being composed at the throw site.

One choice is worth revisiting if a later recording pins it. Cause 7's size template is rendered from the STATE the document carries — its `desired`/`reported` nodes as canonical JSON (`state::state_sections_bytes`) — and the same measure weighs the state a merge would leave stored. The recorded instance's count matches neither the request entity (25 bytes more) nor that entity canonicalised (22 bytes more), and no documented rule yields that span, so the span above is the one the code can state as a rule rather than the recorded number. The consequence to watch: a request whose ENTITY exceeds 8 KB while the state it carries does not is accepted here, and the real service may refuse it. This emulator still bounds the entity it will read at `state::MAX_SHADOW_REQUEST_ENTITY_BYTES` (8x the quota), refused in the emulator's own words.
