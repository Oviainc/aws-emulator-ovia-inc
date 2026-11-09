Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatchomni-space-and-access-control-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatchomni-space-and-access-control-errors cloudwatchomni-view-alert-dashboard-notfound-and-validation
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|GetSpace|4xx->4xx|$.__type
Issue-Key: cloudwatchomni|GetSpace|4xx->4xx|$.message
Issue-Key: cloudwatchomni|GetSpace|4xx->4xx|status
Issue-Key: cloudwatchomni|DeleteSpace|4xx->2xx|$
Issue-Key: cloudwatchomni|DeleteSpace|4xx->2xx|status
Issue-Key: cloudwatchomni|GetAccessProfile|4xx->4xx|$.__type
Issue-Key: cloudwatchomni|GetAccessProfile|4xx->4xx|$.message
Issue-Key: cloudwatchomni|GetAccessProfile|4xx->4xx|status
Issue-Key: cloudwatchomni|CreateAccessProfile|4xx->4xx|$.__type
Issue-Key: cloudwatchomni|CreateAccessProfile|4xx->4xx|$.message
Issue-Key: cloudwatchomni|CreateAccessProfile|4xx->4xx|status
Issue-Key: cloudwatchomni|GetOmniDashboard|4xx->4xx|$.__type
Issue-Key: cloudwatchomni|GetOmniDashboard|4xx->4xx|$.message
Issue-Key: cloudwatchomni|GetOmniDashboard|4xx->4xx|status
Issue-Key: cloudwatchomni|CreateOmniDashboard|4xx->4xx|$.__type
Issue-Key: cloudwatchomni|CreateOmniDashboard|4xx->4xx|$.message
Issue-Key: cloudwatchomni|CreateOmniDashboard|4xx->4xx|status
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# cloudwatchomni authorization not enforced AccessDeniedException never returned

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSpace

## Description
The cloudwatchomni emulator has no authorization gate. Six space-scoped operations that AWS refuses with AccessDeniedException (HTTP 403) instead run straight into resource lookup, body-schema validation, or — worst — the mutation itself. This is ONE shared fault (the authorization check is missing from the request pipeline and therefore never precedes existence lookup or input validation) with SIX causes, all of which must be closed.

AWS (common to all six causes): Smithy model com-amazonaws-cloudwatchomni-cloudwatchomnifrontend, error com.amazonaws.cloudwatchomni#AccessDeniedException, errorKind client, @httpError 403, member `message` required, doc "The caller is not authorized to perform this action.". AccessDeniedException is a modeled error on GetSpace, DeleteSpace, GetAccessProfile, CreateAccessProfile, GetOmniDashboard and CreateOmniDashboard (per the service digest). The recorded responses show the wire shape for the rpcv2Cbor protocol as `__type` = "com.amazon.cloudwatchomnifrontend#AccessDeniedException" with `message` = "Authorization failed" — that message is entirely FIXED text with no variable parts (it interpolates neither the resource id, nor the principal, nor the action). Recorded behavior also establishes the ORDERING requirement: the authorization decision is taken BEFORE the resource is looked up and BEFORE the request body is validated, so an unauthorized caller gets 403 and the service does not disclose whether the target resource exists.

Cause 1 of 6 — GetSpace (fixture cloudwatchomni-space-and-access-control-errors, step task#2).
  AWS: 403, `__type` "com.amazon.cloudwatchomnifrontend#AccessDeniedException", `message` "Authorization failed".
  Emulator: 404, `__type` "ResourceNotFoundException", `message` "Space '<spaceId>' does not exist." (recorded run: spaceId 00000000-0000-4000-8000-51f488049a3e; the id is a per-run value, the defect is the status/code/message class).
  Why: a client cannot distinguish "you may not see this space" from "this space is gone". SDK retry/err-handling branches on the error code, so callers that catch AccessDeniedException to re-auth instead see a not-found and give up; and the emulator leaks resource existence/non-existence to unauthorized callers.
  Fix: the authorization gate in the GetSpace handler, evaluated before the space lookup.

Cause 2 of 6 — DeleteSpace (fixture cloudwatchomni-space-and-access-control-errors, step task#3). MOST SEVERE.
  AWS: 403, `__type` "com.amazon.cloudwatchomnifrontend#AccessDeniedException", `message` "Authorization failed" — the delete does NOT happen.
  Emulator: 200 with an empty body; the request is accepted as a successful (idempotent) delete.
  Why: an unauthorized delete is reported to the client as a success. Any client flow that relies on AWS refusing the destructive call proceeds as though the space were removed, and a test or guard asserting that an unauthorized principal cannot delete a space passes against the emulator while failing against AWS.
  Fix: the authorization gate in the DeleteSpace handler, evaluated before the state mutation.

Cause 3 of 6 — GetAccessProfile (fixture cloudwatchomni-space-and-access-control-errors, step task#4).
  AWS: 403, AccessDeniedException, `message` "Authorization failed".
  Emulator: 404, `__type` "ResourceNotFoundException", `message` "Access profile '<profileName>' does not exist in space '<spaceId>'."
  Why: same as cause 1 — wrong error class and status, and the response discloses that the profile is absent from a space the caller is not entitled to read.
  Fix: the authorization gate in the GetAccessProfile handler, before the profile lookup.

Cause 4 of 6 — CreateAccessProfile (fixture cloudwatchomni-space-and-access-control-errors, step task#5).
  AWS: 403, AccessDeniedException, `message` "Authorization failed".
  Emulator: 404, `__type` "ResourceNotFoundException", `message` "Space '<spaceId>' does not exist."
  Why: a create that AWS refuses on authorization is reported as a missing parent resource, so clients retry against a different domain/space instead of fixing credentials or grants.
  Fix: the authorization gate in the CreateAccessProfile handler, before the parent-space lookup.

Cause 5 of 6 — GetOmniDashboard (fixture cloudwatchomni-view-alert-dashboard-notfound-and-validation, step task#6).
  AWS: 403, AccessDeniedException, `message` "Authorization failed".
  Emulator: 404, `__type` "ResourceNotFoundException", `message` "Dashboard '<dashboardId>' does not exist in space '<spaceId>'."
  Why: same as cause 1, for the dashboard resource.
  Fix: the authorization gate in the GetOmniDashboard handler, before the dashboard lookup.

Cause 6 of 6 — CreateOmniDashboard (fixture cloudwatchomni-view-alert-dashboard-notfound-and-validation, step task#7).
  AWS: 403, AccessDeniedException, `message` "Authorization failed" — the dashboard body is never validated, because authorization is decided first.
  Emulator: 400, `__type` "ValidationException", `message` "panels: does not satisfy the 'required' constraint of the dashboard body schema; $: does not satisfy the 'additionalProperties' constraint of the dashboard body schema".
  Why: this is an ORDERING defect as well as a missing check — the emulator validates the request body before deciding authorization, so an unauthorized caller gets a 400 that invites it to "fix" its payload rather than the 403 that tells it to fix its permissions.
  Fix: the authorization gate in the CreateOmniDashboard handler, sequenced ahead of dashboard-body schema validation.

Not covered by any existing open issue: the two open cloudwatchomni issues are unimplemented-stub reports for CreateIntegration and ListDomains and have nothing to do with the authorization pipeline.

Traces: cloudwatchomni-space-and-access-control-errors#task#2 (GetSpace), #task#3 (DeleteSpace), #task#4 (GetAccessProfile), #task#5 (CreateAccessProfile); cloudwatchomni-view-alert-dashboard-notfound-and-validation#task#6 (GetOmniDashboard), #task#7 (CreateOmniDashboard).

## Evidence
Cause 1 GetSpace — expected status 403, actual 404; $.__type expected 'com.amazon.cloudwatchomnifrontend#AccessDeniedException', got 'ResourceNotFoundException'; $.message expected 'Authorization failed', got "Space '00000000-0000-4000-8000-51f488049a3e' does not exist."
Cause 2 DeleteSpace — expected status 403 with body {'__type': 'com.amazon.cloudwatchomnifrontend#AccessDeniedException', 'message': 'Authorization failed'}, actual 200 with empty body {}.
Cause 3 GetAccessProfile — expected 403, actual 404; __type expected AccessDeniedException, got 'ResourceNotFoundException'; message expected 'Authorization failed', got "Access profile 'sim2real-probe-cwo-profile-missing-677bde2f' does not exist in space '00000000-0000-4000-8000-51f488049a3e'."
Cause 4 CreateAccessProfile — expected 403, actual 404; __type got 'ResourceNotFoundException'; message got "Space '00000000-0000-4000-8000-51f488049a3e' does not exist."
Cause 5 GetOmniDashboard — expected 403, actual 404; __type got 'ResourceNotFoundException'; message got "Dashboard '00000000-0000-4000-8000-c631c01ae1f1' does not exist in space '00000000-0000-4000-8000-c5a4f0a9ee2f'."
Cause 6 CreateOmniDashboard — expected 403, actual 400; __type got 'ValidationException'; message got "panels: does not satisfy the 'required' constraint of the dashboard body schema; $: does not satisfy the 'additionalProperties' constraint of the dashboard body schema".
In every cause the recorded AWS response is the identical fixed pair (403, AccessDeniedException, 'Authorization failed'), confirming a single missing pipeline stage rather than six per-handler messages. Smithy: AccessDeniedException @httpError 403, required member `message`; it is a modeled error on all six operations.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Inspect fixture cloudwatchomni-space-and-access-control-errors steps task#2..task#5 and fixture cloudwatchomni-view-alert-dashboard-notfound-and-validation steps task#6..task#7.
4. Each step issues a cloudwatchomni rpcv2Cbor request for a space/profile/dashboard the caller holds no access grant for. AWS answers 403 AccessDeniedException with message 'Authorization failed'; the emulator instead answers 404 ResourceNotFoundException (task#2, #4, #5, view#6), 200 with an empty body (task#3, the delete silently succeeds), or 400 ValidationException from body-schema validation (view#7).
