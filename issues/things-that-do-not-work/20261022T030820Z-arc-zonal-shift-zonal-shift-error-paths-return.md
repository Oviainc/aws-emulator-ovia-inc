Issue-Source: trace-sweep-divergence
Issue-Trace: arc-zonal-shift-zonal-shift-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: arc-zonal-shift-zonal-shift-error-paths
Issue-Shape-Id: com.amazonaws.arczonalshift#PercDataPlane
Issue-Key: arc-zonal-shift|StartZonalShift|4xx->4xx|$.Message
Issue-Key: arc-zonal-shift|StartZonalShift|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: arc-zonal-shift|StartZonalShift|4xx->4xx|status
Issue-Key: arc-zonal-shift|CancelZonalShift|4xx->4xx|$
Issue-Key: arc-zonal-shift|CancelZonalShift|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: arc-zonal-shift|CancelZonalShift|4xx->4xx|status
Issue-Key: arc-zonal-shift|UpdateZonalShift|4xx->4xx|$
Issue-Key: arc-zonal-shift|UpdateZonalShift|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: arc-zonal-shift|UpdateZonalShift|4xx->4xx|status
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# arc-zonal-shift: zonal-shift error paths return 404 ResourceNotFoundException instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartZonalShift

## Description
The three resource-scoped zonal-shift write operations all take the same wrong error branch on the unauthorized/unavailable-resource path: the emulator resolves the referenced resource first and answers 404 ResourceNotFoundException, where ARC Zonal Shift answers 403 AccessDeniedException before any resource lookup is reported. This is ONE shared fault (authorization/precedence of the error branch) with THREE causes, numbered below; all three must be closed.

Cause 1 of 3: StartZonalShift (fixture arc-zonal-shift-zonal-shift-error-paths, step task#1).
AWS: AccessDeniedException is a service-common error of PercDataPlane (Smithy: service common_errors = AccessDeniedException, InternalServerException, ThrottlingException; AccessDeniedException @error client, @httpError 403, doc "You do not have sufficient access to perform this action."). The recorded response for a StartZonalShift against a resource the caller may not shift is HTTP 403 with header x-amzn-errortype naming AccessDeniedException and an AccessDeniedException body (the recording's body field is "Message" with the text "Requested resource is not available." — the text is an EXAMPLE, the 403 status and the AccessDeniedException error code are the requirement).
Emulator: HTTP 404 with x-amzn-errortype: ResourceNotFoundException and no Message member.
Why: a client's error handling branches on the error code. It sees a retriable/expected "resource does not exist" (ResourceNotFoundException, modeled @httpError 404) where AWS reports an authorization failure, so permission problems are silently misreported as missing resources and never surfaced to the operator; SDK error-type dispatch on x-amzn-errortype picks the wrong exception class entirely.
Fix: the StartZonalShift handler's error path — the access/authorization check must be evaluated and returned before the managed-resource lookup produces a not-found result.

Cause 2 of 3: CancelZonalShift (fixture arc-zonal-shift-zonal-shift-error-paths, step task#2).
AWS: same service-common AccessDeniedException (403). The recorded response for DELETE /zonalshifts/{zonalShiftId} on an id the caller is not permitted to act on is HTTP 403 with x-amzn-errortype naming AccessDeniedException; the recorded body carried no parsed members (body contents are an example, not a requirement).
Emulator: HTTP 404 with body {"__type": "ResourceNotFoundException", "message": "no zonal shift found for zonalShiftId <id>"} and x-amzn-errortype: ResourceNotFoundException.
Why: as Cause 1 — wrong status class (4xx not-found vs forbidden) and wrong error code, so retry/alarm logic keyed on AccessDeniedException never fires.
Fix: the CancelZonalShift handler's error path (same precedence rule as Cause 1), not the state store's lookup result itself.

Cause 3 of 3: UpdateZonalShift (fixture arc-zonal-shift-zonal-shift-error-paths, step task#3).
AWS: same service-common AccessDeniedException (403). The recorded response for PATCH /zonalshifts/{zonalShiftId} on an id the caller is not permitted to act on is HTTP 403 with x-amzn-errortype naming AccessDeniedException.
Emulator: HTTP 404 with body {"__type": "ResourceNotFoundException", "message": "no zonal shift found for zonalShiftId <id>"} and x-amzn-errortype: ResourceNotFoundException.
Why: as above.
Fix: the UpdateZonalShift handler's error path; the three handlers should share one ordering helper so the branch cannot drift again.

Hypothesis: ARC Zonal Shift performs the caller's authorization decision on the target resource before (or without) disclosing whether the zonal-shift id exists, which is why a non-existent/out-of-scope id yields 403 rather than 404; the emulator has no such pre-check and falls straight through to its state lookup. No open issue covers these; list_existing_issues shows no arc-zonal-shift issues at all.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):

- arc-zonal-shift-zonal-shift-error-paths#task#1 StartZonalShift: status expected 403, got 404; $.Message missing (expected 'Requested resource is not available.', got None); $headers.x-amzn-errortype expected 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/', got 'ResourceNotFoundException'.
- arc-zonal-shift-zonal-shift-error-paths#task#2 CancelZonalShift: status expected 403, got 404; body expected {} , got {'__type': 'ResourceNotFoundException', 'message': 'no zonal shift found for zonalShiftId deadbeef1234'}; $headers.x-amzn-errortype expected 'AccessDeniedException:...coral.service/', got 'ResourceNotFoundException'.
- arc-zonal-shift-zonal-shift-error-paths#task#3 UpdateZonalShift: status expected 403, got 404; body expected {}, got {'__type': 'ResourceNotFoundException', 'message': 'no zonal shift found for zonalShiftId deadbeef1234'}; $headers.x-amzn-errortype expected 'AccessDeniedException:...coral.service/', got 'ResourceNotFoundException'.

Model evidence (services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json): AccessDeniedException is a common error of the service with @httpError 403; ResourceNotFoundException is @httpError 404 and is the error the emulator chose. Expected-vs-actual requirement: 403 + AccessDeniedException error code (and matching x-amzn-errortype) on these paths; the recorded message texts and the Coral shape-id suffix in the header are examples. state_reset_failures = 0, so these three steps ran against clean state.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay fixture arc-zonal-shift-zonal-shift-error-paths against it (replay_wire_corpus over trace set simparity-wire-aws-v1).
3. Step task#1 — StartZonalShift for a resource the caller is not permitted to shift: emulator answers 404/ResourceNotFoundException, recording is 403/AccessDeniedException.
4. Step task#2 — DELETE /zonalshifts/deadbeef1234 (CancelZonalShift): emulator answers 404/ResourceNotFoundException, recording is 403/AccessDeniedException.
5. Step task#3 — PATCH /zonalshifts/deadbeef1234 (UpdateZonalShift): emulator answers 404/ResourceNotFoundException, recording is 403/AccessDeniedException.
6. Confirm a fix by re-replaying all three steps named in Issue-Step/trace_ids.
