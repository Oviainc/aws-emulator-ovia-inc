Issue-Source: trace-sweep-divergence
Issue-Trace: arc-zonal-shift-autoshift-observer-notification-toggle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: arc-zonal-shift-autoshift-observer-notification-toggle arc-zonal-shift-list-operations-validation-errors
Issue-Shape-Id: com.amazonaws.arczonalshift#PercDataPlane
Issue-Key: arc-zonal-shift|UpdateAutoshiftObserverNotificationStatus|4xx->4xx|$.message
Issue-Key: arc-zonal-shift|ListZonalShifts|4xx->4xx|$.message
Issue-Key: arc-zonal-shift|ListAutoshifts|4xx->4xx|$.message
Issue-Commit: c9a86881ef96f093104e2802f866de6afbf03a24

# arc-zonal-shift ValidationException messages interpolate the rejected value where the recorded wire omits it

> _Note: this issue was identified against the code base at commit `c9a86881ef96f093104e2802f866de6afbf03a24`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListZonalShifts

## Description
Three net-new causes in the arc-zonal-shift input-validation message builder, found by replaying the wire corpus simparity-wire-aws-v1. This bundle has 3 causes; ALL THREE must be closed before submitting. They share one fault: on every constraint path exercised by this corpus the emulator splices the rejected input value into the ValidationException message as the segment "Value '<x>' at '<member>'", where the recorded service writes "Value at '<member>'" with no quoted value. Status (400) and error code (ValidationException) are correct on all three; the sole divergence is the message text.

NOT already covered, and READ THE COORDINATION NOTE BELOW. The open issue 20261102T015300Z-arc-zonal-shift-cancelzonalshift-validationexception-for-a-pattern.md reports exactly this shape (value interpolated where the recording omits it) for a DIFFERENT operation/member/constraint — CancelZonalShift.zonalShiftId, @pattern — and explicitly excludes these three paths from its scope, telling a fixer to KEEP interpolating the value for them. The open issue 20261029T193235Z-arc-zonal-shift-accessdenied-message-on-unknown-shift.md carries a different cause on one of these messages (the ORDER of the enum value set for ListZonalShifts.status) and not the value segment. The replay report marks none of the three clusters below as OWNED or stub-blocked.

COORDINATION NOTE (important): the FIXED issue 20261029T170702Z-arc-zonal-shift-validationexception-messages-omit-the-rejected.md required the OPPOSITE for these very three paths — it had the emulator start interpolating the rejected value for ListZonalShifts.maxResults (@range), ListAutoshifts.status (enum) and UpdateAutoshiftObserverNotificationStatus.status (enum). The recorded wire fixtures replayed here carry NO quoted value on any of those three paths. So this is not a new feature request: the current behavior is that earlier change applied to paths whose recordings do not show it, and the fix must make the formatter follow the recorded shape PER operation/member/constraint rather than applying one rule service-wide (the same conclusion 20261102T015300Z reached from the other direction). Do not simply invert the rule for the whole service: CreatePracticeRunConfiguration.outcomeAlarms[].type is not exercised by this corpus and is not claimed here.

Cause 1 of 3 — UpdateAutoshiftObserverNotificationStatus with a `status` outside the AutoshiftObserverNotificationStatus enum. AWS: Smithy (services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json): UpdateAutoshiftObserverNotificationStatus is PUT /autoshift-observer-notification, code 200; its sole input member `status` is @required and targets AutoshiftObserverNotificationStatus (enum values ENABLED, DISABLED); the operation models ValidationException (errorKind client, httpError 400) whose `message` is @required. Recorded behavior only for the message composition (the model does not state it): the recording answers 400 ValidationException with "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]". Fixed text of this message class: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]"; variable parts: the member path taken from the request (here `status`) and the enum value set taken from the model, in the order the recording lists it. Emulator: same status and code, but the message is "1 validation error detected: Value 'BOGUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]" — an extra "Value '<x>' at '" segment. (The literal BOGUS is the fixture's own input filling the template; the defect is the presence of the quoted-value segment, not that string.) Why: a client or contract test pinned to AWS's wording for this refusal sees a message production never emits, so message assertions fail though status and code are right; it also echoes the caller's rejected input back in a message AWS keeps value-free here. Fix: the shared arc-zonal-shift ValidationException message formatter in the centralized deserialize -> validate step, enum-constraint path for a body member — the operation handler is never entered. Traces: arc-zonal-shift-autoshift-observer-notification-toggle#task#4.

Cause 2 of 3 — ListZonalShifts with `maxResults` above the MaxResults @range maximum. AWS: Smithy: ListZonalShifts is GET /zonalshifts, code 200; `maxResults` is @httpQuery("maxResults") targeting MaxResults (integer, @range min 1 max 100); the operation models ValidationException (client, 400). Recorded behavior only for the message composition: the recording answers 400 ValidationException "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100". Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must have value less than or equal to "; variable parts: the member path from the request (`maxResults`) and the bound from the model (100). Emulator: "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100" — the same extra quoted-value segment, on the @range path and on a query-bound member. Why: as Cause 1. Fix: the same shared formatter, @range-constraint path for an httpQuery-bound member. Traces: arc-zonal-shift-list-operations-validation-errors#task#3 (its cluster also covers arc-zonal-shift-list-operations-validation-errors#task#5, the same refusal reached a second time in that fixture).

Cause 3 of 3 — ListAutoshifts with a `status` outside the AutoshiftExecutionStatus enum. AWS: Smithy: ListAutoshifts is GET /autoshifts, code 200; `status` is @httpQuery("status") targeting AutoshiftExecutionStatus (enum values ACTIVE, COMPLETED); models ValidationException (client, 400). Recorded behavior only for the message composition: 400 ValidationException "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, COMPLETED]" — fixed text as in Cause 1, with the member path from the request and the enum set from the model. Emulator: "1 validation error detected: Value 'BOGUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, COMPLETED]". Why: as Cause 1. Fix: the same shared formatter, enum-constraint path for an httpQuery-bound member. Traces: arc-zonal-shift-list-operations-validation-errors#task#4.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (branch issues/finder-wi-8fae382f324c4061b982e2f609e662b6-1794234963681): emulator_up=true, 6 fixtures replayed, 34 steps, 19 passed, 15 mismatched, score 55.9%, 10 divergence clusters — all arc-zonal-shift. Three clusters are OWNED by 20261029T170702Z-arc-zonal-shift-accessdeniedexception-403-body-does-not.md and excluded; four more are the azGroupId clusters filed separately. The three below are this bundle.

[mismatch] UpdateAutoshiftObserverNotificationStatus — fixture arc-zonal-shift-autoshift-observer-notification-toggle, step task#4
  expected status 400, actual status 400 (match); expected code ValidationException, actual ValidationException (match)
  $.message: value_mismatch
    expected: "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]"
    actual:   "1 validation error detected: Value 'BOGUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]"
  divergence_key: service=arc-zonal-shift|UpdateAutoshiftObserverNotificationStatus|status=400->400|keys=$.message

[mismatch] ListZonalShifts — fixture arc-zonal-shift-list-operations-validation-errors, step task#3 (affected: task#3, task#5)
  expected status 400, actual status 400 (match)
  $.message: value_mismatch
    expected: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"
    actual:   "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"
  divergence_key: service=arc-zonal-shift|ListZonalShifts|status=400->400|keys=$.message

[mismatch] ListAutoshifts — fixture arc-zonal-shift-list-operations-validation-errors, step task#4
  expected status 400, actual status 400 (match)
  $.message: value_mismatch
    expected: "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, COMPLETED]"
    actual:   "1 validation error detected: Value 'BOGUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, COMPLETED]"
  divergence_key: service=arc-zonal-shift|ListAutoshifts|status=400->400|keys=$.message

In all three the enum value set / numeric bound and the member path already match the recording; the ONLY differing substring is the leading "Value '<rejected value>' at '" versus "Value at '".

Model evidence (smithy_query, model_path services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json):
  UpdateAutoshiftObserverNotificationStatus: PUT /autoshift-observer-notification (code 200); input {status: AutoshiftObserverNotificationStatus, required}; errors include ValidationException (client, httpError 400).
  ListZonalShifts: GET /zonalshifts (code 200); input {maxResults: MaxResults @httpQuery maxResults, range 1..100; status: ZonalShiftStatus @httpQuery; nextToken; resourceIdentifier}; errors include ValidationException (client, 400).
  ListAutoshifts: GET /autoshifts (code 200); input {status: AutoshiftExecutionStatus @httpQuery status; maxResults: MaxResults range 1..100; nextToken}; errors include ValidationException (client, 400).
  Enum shapes: AutoshiftObserverNotificationStatus = [ENABLED, DISABLED]; AutoshiftExecutionStatus = [ACTIVE, COMPLETED]; MaxResults = integer range 1..100.
  The model states nothing about ValidationException message composition, hence "recorded behavior only" for the absence of the quoted value.

Related filed work: 20261102T015300Z-arc-zonal-shift-cancelzonalshift-validationexception-for-a-pattern.md (open) is the same fault on CancelZonalShift.zonalShiftId/@pattern and names these three paths as ones that should KEEP the value; 20261029T170702Z-arc-zonal-shift-validationexception-messages-omit-the-rejected.md (fixed) is what introduced the interpolation on these three paths; 20261029T193235Z-arc-zonal-shift-accessdenied-message-on-unknown-shift.md (open) owns the enum-set ORDER divergence for ListZonalShifts.status, which is not part of this bundle.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, then replay the wire corpus simparity-wire-aws-v1.
2. Cause 1: PUT /autoshift-observer-notification with body {"status":"BOGUS"} (botocore: arc.update_autoshift_observer_notification_status(status="BOGUS")). Expect 400 ValidationException with message "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]" — member path from the request, enum set from the model, NO quoted rejected value. Observe the emulator prefixes "Value 'BOGUS' at '".
3. Cause 2: GET /zonalshifts?maxResults=101 (botocore: arc.list_zonal_shifts(maxResults=101)). Expect 400 ValidationException "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100". Observe "Value '101' at 'maxResults' ...".
4. Cause 3: GET /autoshifts?status=BOGUS (botocore: arc.list_autoshifts(status="BOGUS")). Expect 400 ValidationException "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, COMPLETED]". Observe "Value 'BOGUS' at 'status' ...".
5. Re-replay to confirm: arc-zonal-shift-autoshift-observer-notification-toggle#task#4, arc-zonal-shift-list-operations-validation-errors#task#3 (and #task#5), arc-zonal-shift-list-operations-validation-errors#task#4. All three causes must pass before submitting.
6. Before changing the shared formatter, read 20261102T015300Z-arc-zonal-shift-cancelzonalshift-validationexception-for-a-pattern.md: the formatter has to decide per operation/member/constraint, not service-wide, and that issue's CancelZonalShift.zonalShiftId path must also end up value-free.
