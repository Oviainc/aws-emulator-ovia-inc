Issue-Source: trace-sweep-divergence
Issue-Trace: datasync-task-lifecycle-and-tags
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datasync-task-lifecycle-and-tags
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: datasync|DeleteLocation|4xx->2xx|$
Issue-Key: datasync|DeleteLocation|4xx->2xx|status
Issue-Key: datasync|DeleteLocation|2xx->4xx|status
Issue-Commit: deff4a6e6d31627acea4018c09886a861433b132

# DataSync DeleteLocation ignores task references: an in-use location is deleted, so the later delete fails

> _Note: this issue was identified against the code base at commit `deff4a6e6d31627acea4018c09886a861433b132`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteLocation

## Description
Two net-new causes, both in DeleteLocation and both rooted in the same missing rule: the emulator's DeleteLocation removes a location from the store unconditionally, with no check for transfer tasks that still reference it. Cause 1 is the refusal AWS gives and the emulator does not; cause 2 is the state divergence that refusal's absence leaves behind, observed on a later step of the same fixture. Both must be closed: the second is a separate replayed step and will not pass just because the first does, since it asserts that the location is still deletable at teardown AFTER the task that referenced it was removed.

Smithy model consulted: services/service-com-amazonaws-datasync-fmrsservice/smithy-model/com-amazonaws-datasync-fmrsservice.json — DeleteLocation(input DeleteLocationRequest{LocationArn @required, location ARN pattern}, output DeleteLocationResponse{} i.e. an empty 200 body, errors InternalException(500) and InvalidRequestException(400, members message/errorCode/datasyncErrorCode)). InvalidRequestException is the only modeled client error of the operation, so every client refusal of DeleteLocation is that type and is discriminated by its errorCode/datasyncErrorCode.

=== Cause 1 of 2: DeleteLocation deletes a location a task still references instead of refusing it LocationInUse ===
AWS: a transfer location may not be deleted while a transfer task still names it as SourceLocationArn or DestinationLocationArn. DataSync answers HTTP 400 with __type InvalidRequestException and BOTH classification members set to the discriminator for that class: errorCode = "LocationInUse" and datasyncErrorCode = "LocationInUse" (both are fixed text for the class, not request-derived), and a non-empty `message` string. The recording asserts the message is present but does not pin its text (recorded as '<shaped-not-asserted>'), so the message's exact wording is NOT a requirement of this issue; only the 400 status, the InvalidRequestException type and the two LocationInUse discriminators are. Hypothesis: AWS's text for this class names the still-referencing resource, so the variable part would be the location/task identifier taken from the request and from state — do not assert a literal here without a recording that pins it.
Emulator: HTTP 200 with an empty body — the delete SUCCEEDS and the location is removed while a task still points at it. services/service-com-amazonaws-datasync-fmrsservice/src/ops/delete_location.rs:153-157 calls store.remove_location(&input.location_arn) and maps only its not-found refusal; services/service-com-amazonaws-datasync-fmrsservice/src/state.rs:752-756 is a bare HashMap remove whose only Err is LocationRefusal::not_found. Nothing consults the task store, which does hold each task's source and destination location ARNs.
Why: a client relying on AWS's behavior treats a successful DeleteLocation as proof that nothing referenced the location, and relies on the LocationInUse refusal to order teardown (delete tasks, then locations) and to detect accidental deletion of a location a live task depends on. Against the emulator that guard never fires: the delete reports success, and the service is left holding a task whose source/destination ARNs resolve to nothing — a state AWS makes unreachable. Teardown/cleanup code that catches LocationInUse and retries after deleting tasks never takes that branch.
Fix: the DeleteLocation handler (services/service-com-amazonaws-datasync-fmrsservice/src/ops/delete_location.rs) together with the location store in crate::state — the removal must first resolve the tasks that reference the ARN and, when any does, refuse with InvalidRequestException carrying errorCode = datasyncErrorCode = "LocationInUse" and leave the store untouched.
Traces: datasync-task-lifecycle-and-tags#task#14 (DeleteLocation, expected 400, got 200).

=== Cause 2 of 2: after the un-refused delete, the subsequent DeleteLocation of that location fails 400 instead of succeeding 200 ===
AWS: once the referencing task has been deleted, DeleteLocation on that still-existing location succeeds — HTTP 200 with the empty DeleteLocationResponse body. The location survived the earlier in-use attempt precisely because AWS refused it, so the teardown delete addresses a location the account still holds.
Emulator: HTTP 400 — the location is gone (the earlier call removed it), so remove_location takes its not-found path and the handler answers InvalidRequestException with the LocationNotFound discriminator and the message built at services/service-com-amazonaws-datasync-fmrsservice/src/state.rs:858-862. The status itself is the divergence here (expected 200, got 400): the emulator refuses a delete AWS performs.
Why: this is the observable consequence of Cause 1 and it breaks idempotent teardown: a client that follows AWS's ordering (refusal -> delete the task -> delete the location) gets a client error on the final step and cannot distinguish "already deleted by me" from "never existed", so cleanup scripts and integration-test fixtures abort on a 400 where AWS returns 200. It is a distinct replayed step and a distinct store state, so it needs its own re-replay to confirm the fix.
Fix: the same DeleteLocation handler/state removal path — with Cause 1's in-use guard in place the location is still present at this point and the delete returns the empty 200; no separate change should be needed, but the step must be re-replayed to prove the state sequence, not just the single refusal.
Traces: datasync-task-lifecycle-and-tags#teardown#18 (DeleteLocation, expected 200, got 400).

Related open issues, none of which cover either cause: 20261029T180920Z-datasync-refusals-omit-errorcode-datasyncerrorcode-and-use-emulator.md covers the error ENVELOPE of refusals the emulator already issues (missing errorCode/datasyncErrorCode, emulator-authored message text) for CreateLocationS3/DescribeLocationS3/CreateTask/DescribeTask/ListTasks — it never mentions DeleteLocation and does not cover a refusal that is not issued at all; 20261029T180920Z-datasync-accepts-three-requests-aws-refuses-unassumable-bucket.md covers CreateLocationS3 role-assumption, an absent location and a self-referential task, not location-in-use on delete; 20261023T110503Z-datasync-createtask-createlocations3-cross-member-and-access-validation.md is behavioral and concerns CreateTask/CreateLocationS3 validation and the execution stub. Note that fixing Cause 1 will produce an InvalidRequestException whose errorCode/datasyncErrorCode must be serialized — that serializer defect is the other issue's Cause 1, so both must land for this step to pass.

## Evidence
Replay of simparity-wire-aws-v1 against the booted emulator (emulator_up=true, /__admin/health answered 200, data listener accepting). Summary: 122 steps, 79 passed, 43 mismatched, score 64.8%, 15 divergence clusters. The two clusters below are the only ones with no owning issue and no blocking stub (owned_by: [], stub_path: "").

Cause 1 — fixture datasync-task-lifecycle-and-tags, step task#14, operation DeleteLocation, verdict=mismatch, affected_count=1:
  status: expected 400, got 200
  $: empty_body (expected {'__type': 'InvalidRequestException', 'datasyncErrorCode': 'LocationInUse', 'errorCode': 'LocationInUse', 'message': '<shaped-not-asserted>'}, got {})
  divergence_key: service=datasync|DeleteLocation|status=400->200|keys=$,status

Cause 2 — fixture datasync-task-lifecycle-and-tags, step teardown#18, operation DeleteLocation, verdict=mismatch, affected_count=1:
  status: expected 200, got 400
  divergence_key: service=datasync|DeleteLocation|status=200->400|keys=status

Code evidence for the shared root cause:
  services/service-com-amazonaws-datasync-fmrsservice/src/ops/delete_location.rs:153-157 — store.remove_location(&input.location_arn).map_err(|refusal| InvalidRequestException(refusal.into_invalid_request()))?; no reference check.
  services/service-com-amazonaws-datasync-fmrsservice/src/state.rs:747-756 — remove_location is self.locations.remove(arn).ok_or_else(|| LocationRefusal::not_found(arn)); the doc comment states the only refusal it models is "an ARN naming no location this account holds".
  services/service-com-amazonaws-datasync-fmrsservice/src/state.rs:856-863 — LocationRefusal::not_found sets error_code = LOCATION_NOT_FOUND_CODE, the refusal teardown#18 wrongly receives.

Model references: DeleteLocation errors = [InternalException(500, server), InvalidRequestException(400, client)]; DeleteLocationResponse has no members (empty 200 body); InvalidRequestException members = {message, errorCode, datasyncErrorCode}.

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 datasync fixtures; both causes are steps of fixture datasync-task-lifecycle-and-tags, in order.

Cause 1: CreateLocationS3 (or any CreateLocation*) twice to get a source and a destination location, CreateTask referencing both, then call DeleteLocation with one of those two location ARNs while the task still exists. Expected: HTTP 400, __type InvalidRequestException, errorCode = datasyncErrorCode = "LocationInUse", non-empty message, and the location still present (a following DescribeLocation*/ListLocations still resolves it). Observed: HTTP 200 with body {} and the location removed. Replay: datasync-task-lifecycle-and-tags#task#14

Cause 2: continuing the same sequence, DeleteTask the referencing task, then DeleteLocation the same location ARN again. Expected: HTTP 200 with an empty body. Observed: HTTP 400 InvalidRequestException (LocationNotFound), because the step above already removed the location. Replay: datasync-task-lifecycle-and-tags#teardown#18
