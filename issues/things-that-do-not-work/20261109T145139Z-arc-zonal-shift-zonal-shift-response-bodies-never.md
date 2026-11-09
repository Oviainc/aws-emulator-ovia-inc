Issue-Source: trace-sweep-divergence
Issue-Trace: arc-zonal-shift-nlb-zone-weight-shift-and-cancel
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: arc-zonal-shift-nlb-zone-weight-shift-and-cancel
Issue-Shape-Id: com.amazonaws.arczonalshift#PercDataPlane
Issue-Key: arc-zonal-shift|StartZonalShift|2xx->2xx|$.azGroupId
Issue-Key: arc-zonal-shift|StartZonalShift|2xx->2xx|$.expiryTime
Issue-Key: arc-zonal-shift|StartZonalShift|2xx->2xx|$.startTime
Issue-Key: arc-zonal-shift|StartZonalShift|2xx->2xx|$.zonalShiftId
Issue-Key: arc-zonal-shift|GetManagedResource|2xx->2xx|$.zonalShifts[*].azGroupId
Issue-Key: arc-zonal-shift|GetManagedResource|2xx->2xx|$.zonalShifts[*].expiryTime
Issue-Key: arc-zonal-shift|GetManagedResource|2xx->2xx|$.zonalShifts[*].startTime
Issue-Key: arc-zonal-shift|ListZonalShifts|2xx->2xx|$.items[*].azGroupId
Issue-Key: arc-zonal-shift|ListZonalShifts|2xx->2xx|$.items[*].expiryTime
Issue-Key: arc-zonal-shift|ListZonalShifts|2xx->2xx|$.items[*].startTime
Issue-Key: arc-zonal-shift|CancelZonalShift|2xx->2xx|$.azGroupId
Issue-Key: arc-zonal-shift|CancelZonalShift|2xx->2xx|$.expiryTime
Issue-Key: arc-zonal-shift|CancelZonalShift|2xx->2xx|$.startTime
Issue-Commit: c9a86881ef96f093104e2802f866de6afbf03a24

# arc-zonal-shift zonal-shift response bodies never carry azGroupId

> _Note: this issue was identified against the code base at commit `c9a86881ef96f093104e2802f866de6afbf03a24`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartZonalShift

## Description
Four net-new causes, one per serialization site, found by replaying the wire corpus simparity-wire-aws-v1. This bundle has 4 causes; ALL FOUR must be closed before submitting. They share one fault: every zonal-shift object the recorded service returns carries an `azGroupId` member naming the Availability Zone group of the shifted resource's Region, and the emulator omits that member from all four response shapes — the create, the two reads and the cancel. Every other asserted member of these bodies, and the HTTP status of each (201/200/200/200), matches.

NOT already covered. The open issues 20261029T193235Z-arc-zonal-shift-accessdenied-message-on-unknown-shift.md and 20261104T064145Z-arc-zonal-shift-an-elbv2-load-balancer-opted.md both report that a load balancer opted into zonal shift never becomes an ARC managed resource; in this fixture the shift IS created (201) and IS read back, so that registration path works here and those issues do not cover the missing member. 20261029T170702Z-arc-zonal-shift-accessdeniedexception-403-body-does-not.md owns three unrelated 403-body clusters in the same corpus. The replay report marks none of the four clusters below as OWNED or stub-blocked.

Cause 1 of 4 — StartZonalShift's 201 body omits azGroupId. AWS: the on-disk Smithy model (services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json) declares StartZonalShift as POST /zonalshifts code 201 with output shape ZonalShift {zonalShiftId, resourceIdentifier, awayFrom, expiryTime, startTime, status, comment} and declares NO azGroupId member, so this is recorded behavior only: the recorded 201 body for a zonal shift started on a Network Load Balancer carries, in addition to those members, "azGroupId" whose value is the Availability Zone group identifier of the Region the shifted resource lives in (the recording of a us-west-2 resource carries "us-west-2-zg-1"; the literal string is the recording's own Region filling the template, not a requirement). Hypothesis: azGroupId is an API member added after this checkout's model snapshot and is derived from the Region (and AZ group) of the resource named by resourceIdentifier, not from anything the caller sends. Emulator: the 201 body contains no azGroupId at all ($.azGroupId missing, got None); crates/service-com-amazonaws-arczonalshift-percdataplane has no such field anywhere — StartZonalShiftOutput is built without it. Why: a client that reads azGroupId off a StartZonalShift response — to label or correlate the shift with the AZ group it moved traffic away from — gets a null/absent member against real AWS's always-present one, so SDK models that type it as non-optional or code that dereferences it breaks on the emulator only. Fix: the StartZonalShift output construction in services/service-com-amazonaws-arczonalshift-percdataplane/src/ops/start_zonal_shift.rs (the Ok(StartZonalShiftOutput{..}) at the end of handle()), plus the member itself — the generated output type has no azGroupId, so the service's model/types surface has to carry it before the handler can populate it, and the value belongs beside the shift in crate::state (derived from the managed resource's Region/AZ group) so all four sites report the same thing. Traces: arc-zonal-shift-nlb-zone-weight-shift-and-cancel#task#8.

Cause 2 of 4 — GetManagedResource's zonalShifts[] entries omit azGroupId. AWS: Smithy: GetManagedResource is GET /managedresources/{resourceIdentifier} code 200; its output's @required `zonalShifts` is a list of ZonalShiftInResource {appliedStatus, zonalShiftId, resourceIdentifier, awayFrom, expiryTime, startTime, comment, shiftType, practiceRunOutcome} — again no azGroupId in this snapshot, so recorded behavior only: the recorded 200 body's zonalShifts[0] carries azGroupId, the same AZ-group identifier as the create response for that shift. Emulator: $.zonalShifts[0].azGroupId missing (got None); every other asserted member of that entry matches. Why: as Cause 1, on the read path a client uses to discover the shifts in effect for a resource. Fix: the ZonalShiftInResource construction in services/service-com-amazonaws-arczonalshift-percdataplane/src/state.rs (zonal_shifts_in_resource(), the map at ~line 517), reading the value stored with the shift. Traces: arc-zonal-shift-nlb-zone-weight-shift-and-cancel#task#9.

Cause 3 of 4 — ListZonalShifts' items[] entries omit azGroupId. AWS: Smithy: ListZonalShifts is GET /zonalshifts code 200, output {items: ZonalShiftSummaries, nextToken}; ZonalShiftSummary is {zonalShiftId, resourceIdentifier, awayFrom, expiryTime, startTime, status, comment, shiftType, practiceRunOutcome} with no azGroupId in this snapshot — recorded behavior only: the recorded 200 page's items[0] carries azGroupId for the shift just started. Emulator: $.items[0].azGroupId missing (got None). Why: as Cause 1, on the list/paginated read. Fix: the ZonalShiftSummary construction behind ListZonalShifts (services/service-com-amazonaws-arczonalshift-percdataplane/src/ops/list_zonal_shifts.rs and the shared summary builder in src/state.rs). Traces: arc-zonal-shift-nlb-zone-weight-shift-and-cancel#task#10.

Cause 4 of 4 — CancelZonalShift's 200 body omits azGroupId. AWS: Smithy: CancelZonalShift is DELETE /zonalshifts/{zonalShiftId} code 200 with the same ZonalShift output shape as StartZonalShift — recorded behavior only: the recorded 200 body for the canceled shift carries azGroupId, the same value the create response carried for that shift, alongside status CANCELED. Emulator: $.azGroupId missing (got None). Why: as Cause 1; a client that correlates the cancel response with the create response on azGroupId has nothing to correlate on. Fix: the output construction in services/service-com-amazonaws-arczonalshift-percdataplane/src/ops/cancel_zonal_shift.rs, from the same stored value. Traces: arc-zonal-shift-nlb-zone-weight-shift-and-cancel#task#11.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout: emulator_up=true, 6 fixtures replayed, 34 steps, 19 passed, 15 mismatched, score 55.9%, 10 divergence clusters — all arc-zonal-shift. Three clusters are OWNED by 20261029T170702Z-arc-zonal-shift-accessdeniedexception-403-body-does-not.md; three message-text clusters are filed as a separate bundle. The four below are this bundle, all from one fixture that starts a zonal shift on a Network Load Balancer, reads it back two ways and cancels it.

[mismatch] StartZonalShift — fixture arc-zonal-shift-nlb-zone-weight-shift-and-cancel, step task#8
  expected status 201, actual 201 (match)
  $.azGroupId: missing (expected 'us-west-2-zg-1', got None)
  divergence_key: service=arc-zonal-shift|StartZonalShift|status=201->201|keys=$.azGroupId,$.expiryTime,$.startTime,$.zonalShiftId

[mismatch] GetManagedResource — same fixture, step task#9
  expected status 200, actual 200 (match)
  $.zonalShifts[0].azGroupId: missing (expected 'us-west-2-zg-1', got None)
  divergence_key: service=arc-zonal-shift|GetManagedResource|status=200->200|keys=$.zonalShifts[0].azGroupId,$.zonalShifts[0].expiryTime,$.zonalShifts[0].startTime

[mismatch] ListZonalShifts — same fixture, step task#10
  expected status 200, actual 200 (match)
  $.items[0].azGroupId: missing (expected 'us-west-2-zg-1', got None)
  divergence_key: service=arc-zonal-shift|ListZonalShifts|status=200->200|keys=$.items[0].azGroupId,$.items[0].expiryTime,$.items[0].startTime

[mismatch] CancelZonalShift — same fixture, step task#11
  expected status 200, actual 200 (match)
  $.azGroupId: missing (expected 'us-west-2-zg-1', got None)
  divergence_key: service=arc-zonal-shift|CancelZonalShift|status=200->200|keys=$.azGroupId,$.expiryTime,$.startTime

SCOPE NOTE — what is NOT claimed here: each of those four diffs also lists startTime, expiryTime and (for task#8) zonalShiftId with expected '<shaped-not-asserted>' against the emulator's actual 1794237452.9437366 / 1794235652.9437366 / 'zs-0000000001'. Those are per-run values — the replay clock and a generated identifier — and the corpus asserts no value for them, so they are deliberately excluded from this issue; only the absent azGroupId member is claimed. (For reference, the emulator's values come from state::as_date_time() at src/state.rs line 538, which builds the timestamp from SystemTime::now() with sub-second precision, and from ZonalShiftStore::next_zonal_shift_id(), a sequential counter. 'zs-0000000001' satisfies ZonalShiftId's @length 6..36 and @pattern ^[A-Za-z0-9-]+$.)

Model evidence (smithy_query, model_path services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json):
  StartZonalShift: POST /zonalshifts, code 201, output_shape ZonalShift.
  CancelZonalShift: DELETE /zonalshifts/{zonalShiftId}, code 200, output_shape ZonalShift.
  ListZonalShifts: GET /zonalshifts, code 200, output {items: ZonalShiftSummaries, nextToken}, paginated.
  GetManagedResource: GET /managedresources/{resourceIdentifier}, code 200, output {arn, name, appliedWeights (required), zonalShifts (required, list of ZonalShiftInResource), autoshifts, practiceRunConfiguration, zonalAutoshiftStatus}.
  Shapes ZonalShift / ZonalShiftSummary / ZonalShiftInResource each declare: zonalShiftId, resourceIdentifier, awayFrom, expiryTime, startTime, comment (+ status, appliedStatus, shiftType, practiceRunOutcome variously) and NO azGroupId member. The recorded responses nevertheless carry azGroupId on all four, so the member is recorded behavior only against this snapshot.

Code evidence: services/service-com-amazonaws-arczonalshift-percdataplane/src/ops/start_zonal_shift.rs lines 314-322 build StartZonalShiftOutput from the stored shift with exactly the seven modeled members and no azGroupId; src/state.rs lines 511-533 (zonal_shifts_in_resource) build ZonalShiftInResource the same way; crate::state::ZonalShift itself holds no AZ-group field, so no site has a value to serialize.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, then replay the wire corpus simparity-wire-aws-v1 (fixture arc-zonal-shift-nlb-zone-weight-shift-and-cancel).
2. Reach a managed resource ARC holds for the caller (the fixture opts a Network Load Balancer into zonal shift, then reads it via GET /managedresources/{resourceIdentifier}).
3. Cause 1: POST /zonalshifts with resourceIdentifier = that resource's ARN, a valid awayFrom for it, expiresIn and comment. Expect 201 whose body carries azGroupId naming the Availability Zone group of that resource's Region alongside zonalShiftId/resourceIdentifier/awayFrom/expiryTime/startTime/status/comment. Observe the emulator's 201 body has no azGroupId member.
4. Cause 2: GET /managedresources/{resourceIdentifier}. Expect zonalShifts[0].azGroupId present with the same value; observe it absent.
5. Cause 3: GET /zonalshifts. Expect items[0].azGroupId present with the same value; observe it absent.
6. Cause 4: DELETE /zonalshifts/{zonalShiftId}. Expect the 200 body to carry azGroupId with the same value (and status CANCELED); observe it absent.
7. Re-replay arc-zonal-shift-nlb-zone-weight-shift-and-cancel#task#8, #task#9, #task#10 and #task#11 to confirm. All four causes must pass before submitting. Do not treat the startTime/expiryTime/zonalShiftId entries in those same diffs as part of this fix: they are per-run values the corpus does not assert.
