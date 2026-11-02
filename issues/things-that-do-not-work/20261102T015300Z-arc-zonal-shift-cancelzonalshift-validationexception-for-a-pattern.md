Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-zonal-shift.cancel-zonal-shift-malformed-id-rejected
Issue-Shape-Id: com.amazonaws.arczonalshift#PercDataPlane
Issue-Key: arc-zonal-shift.cancel-zonal-shift-malformed-id-rejected|run#5|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: e84aec71ffc201933e2dfa47c6e564cbd39c2a62

# arc-zonal-shift: CancelZonalShift ValidationException for a pattern-violating zonalShiftId interpolates the rejected value, which AWS omits for this member

> _Note: this issue was identified against the code base at commit `e84aec71ffc201933e2dfa47c6e564cbd39c2a62`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelZonalShift

## Description
One net-new root cause in the arc-zonal-shift input-validation message builder, found by replaying the behavioral suite (simparity-behavioral-aws-v1). This bundle has 1 cause; it must be closed before submitting.

NOT already covered. The open issue 20261029T193235Z-arc-zonal-shift-accessdenied-message-on-unknown-shift.md lists this same check in its Issue-Trace-Ids, but its replay key for the check is the LATER assertion in the same check body (`assert other_message == ""`, run#9 — the empty-message AccessDeniedException on a well-formed but unknown id). That issue even states that "the earlier assertion in the same check — the ValidationException for a pattern-violating id — passes, so the pattern-rejection path is NOT part of this cause." That earlier assertion is exactly what now fails, at line 29, before the owned assertion is ever reached. The replay report accordingly marks this check as a plain [mismatch], not OWNED. This issue is that earlier, un-owned assertion only.

Cause 1 of 1 — CancelZonalShift's 400 ValidationException for a zonalShiftId that violates the @pattern constraint renders the rejected value inside the message, where the recorded service renders the member path alone (check arc-zonal-shift.cancel-zonal-shift-malformed-id-rejected).

AWS: Smithy (services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json): CancelZonalShift is DELETE /zonalshifts/{zonalShiftId}; its sole input member zonalShiftId is @required, @httpLabel, and targets ZonalShiftId (@length min 6 max 36, @pattern ^[A-Za-z0-9-]+$). The operation models ValidationException (errorKind client, httpError 400) whose `message` member is @required. For a value that satisfies the length bound (so no client-side botocore check intercepts it) but violates the pattern, the recorded service answers 400 ValidationException with the message "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <the member's pattern, without anchors>". Fixed text of this message class: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: "; variable parts: the member path taken from the request (here `zonalShiftId`) and the pattern taken from the model (here `[A-Za-z0-9-]+`). Recorded behavior only for the ABSENCE of the quoted value on this operation/member: the model does not state message composition, and the recording for this path carries no "Value '<x>' at" segment at all.

Emulator: returns the right status (400) and the right code (ValidationException), and the right pattern and member path, but splices the rejected input value into the message: "1 validation error detected: Value 'bad_id_1' at 'zonalShiftId' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9-]+". (The literal `bad_id_1` is the check's own input filling the template — the defect is the presence of the `Value '<x>' at` form on this path, not that one string.)

Why: the message is the only machine-readable part of a constraint rejection, and a client or contract test pinned to AWS's wording for this operation sees an extra quoted segment that production never emits, so byte-exact message assertions fail even though status and code are right. It also leaks the caller's rejected identifier into a message AWS deliberately keeps value-free on this path, which is the opposite direction from the service's own disclosure behavior for this input (its ownership refusal carries no text at all).

Fix: the arc-zonal-shift shared ValidationException message formatter invoked by the centralized deserialize → validate step, on the @pattern-constraint path for httpLabel-bound members — CancelZonalShift's handler body is never entered (services/service-com-amazonaws-arczonalshift-percdataplane/src/ops/cancel_zonal_shift.rs, handle() at line 173, is unreached), so the text is produced entirely in the pre-handler validation layer. COORDINATION NOTE for the fixer: this looks like over-generalisation of the fix for the now-closed issue 20261029T170702Z-arc-zonal-shift-validationexception-messages-omit-the-rejected.md, which required the quoted value to be interpolated for ListZonalShifts.maxResults (@range), ListAutoshifts.status, UpdateAutoshiftObserverNotificationStatus.status and CreatePracticeRunConfiguration.outcomeAlarms[].type (enum). Those recordings DO carry the value; this one does not. Do not revert that fix service-wide and do not generalise this one service-wide either — the shape is per-operation/per-constraint, so the formatter needs to distinguish the paths rather than apply a single rule. Likewise the open issue 20261029T193235Z-… carries the enum-set ORDER divergence for ListZonalShifts.status, which is a different cause and is not part of this bundle.

Traces: arc-zonal-shift.cancel-zonal-shift-malformed-id-rejected (first assertion, line 29). Its later assertions — the empty-message AccessDeniedException for a well-formed unknown id, and the ListZonalShifts positive control — are unreached while this one fails; the empty-message one is owned by 20261029T193235Z-arc-zonal-shift-accessdenied-message-on-unknown-shift.md and is not claimed here.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 3/21 passed (14.3%); all 21 checks are arc-zonal-shift. 15 of the 18 failures are blocked by the already-filed unimplemented EC2 CreateInternetGateway scaffold stub (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_internet_gateway.rs) and are excluded. Of the remaining 3 mismatches, 2 are marked OWNED by 20261029T193235Z-arc-zonal-shift-accessdenied-message-on-unknown-shift.md (list-zonal-shifts-invalid-status-filter-rejected, update-zonal-shift-extends-expiry) and are excluded. This issue is the one surviving failure.

[mismatch] arc-zonal-shift.cancel-zonal-shift-malformed-id-rejected
  check_arc_zonal_shift_cancel_zonal_shift_malformed_id_rejected.py, line 29:
  assert message == expected_message
  AssertionError: expected message "1 validation error detected: Value at 'zonalShiftId' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9-]+", got "1 validation error detected: Value 'bad_id_1' at 'zonalShiftId' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9-]+"

  Request: CancelZonalShift(zonalShiftId="bad_id_1") -> DELETE /zonalshifts/bad_id_1. The value is 8 characters, so it satisfies ZonalShiftId's @length 6..36 and is not rejected client-side; it contains `_`, so it violates @pattern ^[A-Za-z0-9-]+$.
  Status: 400 matches. Error code: ValidationException matches (the preceding assertion on the code passes). Sole divergence: the emulator's message contains the extra segment "Value 'bad_id_1' at '" where the recording has "Value at '". Fixed text = everything outside the member path and the pattern; variable parts = the member path from the request and the pattern from the model.

  Model evidence (smithy_query on services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json): CancelZonalShift input {zonalShiftId: target ZonalShiftId, httpLabel, required, length 6..36, pattern ^[A-Za-z0-9-]+$}; http {method DELETE, uri /zonalshifts/{zonalShiftId}, code 200}; errors include ValidationException (errorKind client, httpError 400, members message @required + reason @required).

  Code evidence: services/service-com-amazonaws-arczonalshift-percdataplane/src/ops/cancel_zonal_shift.rs lines 173-196 — handle() first reaches the store and then the ownership check (store.authorized_shift_mut), with no pattern check of its own, confirming the diverging text is emitted by the shared pre-handler validation layer and not by this operation's handler.

  Related filed work: 20261029T170702Z-arc-zonal-shift-validationexception-messages-omit-the-rejected.md is marked FIXED and required the opposite shape (value interpolated) for four OTHER operation/member/constraint combinations; 20261029T193235Z-arc-zonal-shift-accessdenied-message-on-unknown-shift.md is open and owns the later assertion of this same check plus the ListZonalShifts enum-set ordering. Neither covers the pattern-constraint message on CancelZonalShift.zonalShiftId.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, and replay the behavioral suite simparity-behavioral-aws-v1 (all 21 checks are arc-zonal-shift).
2. Call CancelZonalShift with a zonalShiftId that satisfies ZonalShiftId's @length 6..36 but violates its @pattern ^[A-Za-z0-9-]+$ — e.g. DELETE /zonalshifts/bad_id_1 (botocore: arc.cancel_zonal_shift(zonalShiftId="bad_id_1")).
3. Expect HTTP 400, code ValidationException, and message exactly "1 validation error detected: Value at 'zonalShiftId' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9-]+" — the member path from the request and the pattern from the model, with NO quoted rejected value.
4. Observe the emulator instead answers "1 validation error detected: Value 'bad_id_1' at 'zonalShiftId' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9-]+".
5. Regression guard while fixing: the four paths closed by 20261029T170702Z-arc-zonal-shift-validationexception-messages-omit-the-rejected.md must KEEP interpolating the value — GET /zonalshifts?maxResults=101, GET /autoshifts?status=BOGUS, PUT /autoshift-observer-notification {"status":"BOGUS"}, and CreatePracticeRunConfiguration with an unmodeled outcomeAlarms[0].type.
6. Re-replay arc-zonal-shift.cancel-zonal-shift-malformed-id-rejected to confirm. Note its later assertions (empty-message AccessDeniedException for a well-formed unknown id) are owned by 20261029T193235Z-arc-zonal-shift-accessdenied-message-on-unknown-shift.md, so this check can still fail on those until that issue is closed; this cause is closed when the line-29 message assertion passes.
