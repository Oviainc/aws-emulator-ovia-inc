Issue-Source: trace-sweep-divergence
Issue-Trace: arc-zonal-shift-zonal-shift-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: arc-zonal-shift-zonal-shift-error-paths
Issue-Shape-Id: com.amazonaws.arczonalshift#PercDataPlane
Issue-Key: arc-zonal-shift|StartZonalShift|4xx->4xx|$.Message
Issue-Key: arc-zonal-shift|CancelZonalShift|4xx->4xx|$
Issue-Key: arc-zonal-shift|UpdateZonalShift|4xx->4xx|$
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# arc-zonal-shift AccessDeniedException 403 body does not match the recorded wire shape

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartZonalShift

## Description
Three net-new divergences on the arc-zonal-shift 403 AccessDeniedException path. The HTTP status is right (403) on all three, but the emulator serializes a different error document than the recorded service: on StartZonalShift the message is not where the recording puts it, and on CancelZonalShift/UpdateZonalShift the emulator emits an error document where the recording has an empty JSON body. This bundle has 3 causes; all three must be closed.

Cause 1 of 3: StartZonalShift 403 body member name (fixture arc-zonal-shift-zonal-shift-error-paths, step task#1).
AWS: Smithy: StartZonalShift (POST /zonalshifts, success 201) models AccessDeniedException (errorKind client, httpError 403) with a single member `message`. The recorded 403 response carries the text at top-level member `Message` (capital M) - recorded behavior only for the casing - with the fixed text "Requested resource is not available." for this error class (no variable part), and does not carry an `__type` discriminator member.
Emulator: returns 403 but with no `Message` member at all; the body is {"__type": "AccessDeniedException", "message": "Requested resource is not available."}, i.e. the text is under a lowercase `message` plus an added `__type` member.
Why: a client that reads the access-denied text out of the field the service actually populates (`Message`) gets nothing - the error surfaces as a 403 with an empty reason - and a client comparing the body against the service's shape sees an extra `__type` member the service does not send.
Fix: the arc-zonal-shift error serializer for AccessDeniedException (the shared error-response serialization used by the operation handlers), so the 403 document matches the recorded member name and member set.

Cause 2 of 3: CancelZonalShift 403 body (fixture arc-zonal-shift-zonal-shift-error-paths, step task#2).
AWS: Smithy: CancelZonalShift (DELETE /zonalshifts/{zonalShiftId}) models AccessDeniedException, httpError 403. Recorded behavior only: on this path the service answers 403 with an empty JSON body ({}) - the error class is conveyed by the status (and the error-type header), with no message document.
Emulator: answers 403 with the body {"__type": "AccessDeniedException", "message": "Requested resource is not available."} where the recording has {}.
Why: the emulator's 403 document differs in shape from the service's on this operation, so clients and contract tests keyed on the service's response body see members that never appear in production.
Fix: the same AccessDeniedException serialization path, made per-operation consistent with the recording for the DELETE /zonalshifts/{zonalShiftId} handler.

Cause 3 of 3: UpdateZonalShift 403 body (fixture arc-zonal-shift-zonal-shift-error-paths, step task#3).
AWS: Smithy: UpdateZonalShift (PATCH /zonalshifts/{zonalShiftId}) models AccessDeniedException, httpError 403. Recorded behavior only: 403 with an empty JSON body ({}).
Emulator: 403 with {"__type": "AccessDeniedException", "message": "Requested resource is not available."}.
Why: same as cause 2.
Fix: the same AccessDeniedException serialization path for the PATCH /zonalshifts/{zonalShiftId} handler.

No open issue covers any of these (no existing arc-zonal-shift / zonal issues are filed).

Traces: arc-zonal-shift-zonal-shift-error-paths#task#1, #task#2, #task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up):

1) StartZonalShift - arc-zonal-shift-zonal-shift-error-paths#task#1: status 403 expected, 403 actual. Divergence key $.Message: missing - expected 'Requested resource is not available.' at $.Message, got None. Emulator body instead carries $.__type = "AccessDeniedException" and $.message. The message text "Requested resource is not available." is the fixed text of this error class; it has no request- or state-derived parts.

2) CancelZonalShift - arc-zonal-shift-zonal-shift-error-paths#task#2: 403 expected, 403 actual. $ value_mismatch - expected {}, got {'__type': 'AccessDeniedException', 'message': 'Requested resource is not available.'}.

3) UpdateZonalShift - arc-zonal-shift-zonal-shift-error-paths#task#3: 403 expected, 403 actual. $ value_mismatch - expected {}, got {'__type': 'AccessDeniedException', 'message': 'Requested resource is not available.'}.

Model evidence (services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json): AccessDeniedException is a client error with httpError 403 and one member `message`; it is bound to StartZonalShift, CancelZonalShift and UpdateZonalShift.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: drive the StartZonalShift access-denied path (POST /zonalshifts) - fixture arc-zonal-shift-zonal-shift-error-paths step task#1. Inspect the 403 body: it must carry the text under the member the recording uses (`Message`) and must not add `__type`.
3. Cause 2: drive the CancelZonalShift access-denied path (DELETE /zonalshifts/{zonalShiftId}) - same fixture, step task#2. The recorded 403 body is {}; the emulator returns an error document.
4. Cause 3: drive the UpdateZonalShift access-denied path (PATCH /zonalshifts/{zonalShiftId}) - same fixture, step task#3. Same divergence as cause 2.
5. All three steps must match their recorded 403 bodies for this issue to be closed.
