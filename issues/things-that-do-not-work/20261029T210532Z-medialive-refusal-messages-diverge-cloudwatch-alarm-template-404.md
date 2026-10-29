Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.cloudwatch-alarm-template-group-hierarchy-crud medialive.multiplex-start-without-programs-rejected
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive.cloudwatch-alarm-template-group-hierarchy-crud|run#18|AssertionError|assert message is None, f"expected no message reading deleted template, got {message!r}"
Issue-Key: medialive.multiplex-start-without-programs-rejected|run#16|AssertionError|assert message == "Cannot delete a multiplex that is in RUNNING state", message
Issue-Commit: fa14c0f225749a1f587b9baa3463a5a1c30960f9

# medialive refusal messages diverge: CloudWatch-alarm-template 404 adds a message AWS omits, and DeleteMultiplex-while-RUNNING 409 is worded by the emulator

> _Note: this issue was identified against the code base at commit `fa14c0f225749a1f587b9baa3463a5a1c30960f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCloudWatchAlarmTemplate

## Description
This issue bundles 2 causes, both in the medialive crate's refusal-construction paths: the HTTP status and the error class are correct on both, but the `message` body member is emulator-authored rather than MediaLive's own. Each cause below must be closed before submitting.

Cause 1 of 2: the CloudWatch-alarm-TEMPLATE not-found refusal synthesises a `message` member on a 404 where AWS sends none.
AWS: Smithy: GetCloudWatchAlarmTemplate -> GET /prod/cloudwatch-alarm-templates/{Identifier} (200), UpdateCloudWatchAlarmTemplate -> PATCH /prod/cloudwatch-alarm-templates/{Identifier} (200); both model NotFoundException (errorKind client, httpError 404) whose single member is `Message` (jsonName `message`, target __string) and is NOT @required. Recorded behavior only: for a template Identifier the account no longer holds (a template that was just deleted), AWS answers 404 NotFoundException with NO `message` member in the body at all — the behavioral check asserts `Message` is absent (None) for both the get and the update path. There is no fixed text and no variable part, because AWS emits no message for this error class on these two operations.
Emulator: answers 404 NotFoundException but adds a synthesised lowercase-prose sentence, 'cloudwatch alarm template <identifier> not found' (observed: 'cloudwatch alarm template 2000000 not found', where 2000000 is the id the check created and deleted — that value fills the emulator's template and is not itself a requirement). The same emulator-local resource-label helper is the one already reported for the enclosing GROUP resource in open issue 20261029T201850Z-medialive-not-found-refusals-carry-emulator-authored-resource (its Cause 8 covers GetCloudWatchAlarmTemplateGroup only, via Issue-Key medialive|GetCloudWatchAlarmTemplateGroup|4xx->4xx|$.message); the TEMPLATE operations Get/UpdateCloudWatchAlarmTemplate are not carried by that issue's keys, so they are reported here.
Why: a client that treats presence of `message` as meaningful — surfacing it to a user, asserting the body shape of a refusal, or matching the refusal text to decide whether to recreate the template — sees a body member AWS does not send, carrying words AWS never writes for any MediaLive resource. The error class is right, so only body-shape- and text-sensitive callers break, but the body shape of a refusal is part of the contract.
Fix: the not-found refusal construction reached from the CloudWatch-alarm-template handlers (and the shared resource-label not-found helper they call) in services/service-com-amazonaws-medialive-medialive: these paths must emit the 404 NotFoundException with no `message` member synthesised, for both the get and the update path.
Traces: medialive.cloudwatch-alarm-template-group-hierarchy-crud.

Cause 2 of 2: DeleteMultiplex on a RUNNING multiplex is refused with the right class but the emulator's own wording.
AWS: Smithy: DeleteMultiplex -> DELETE /prod/multiplexes/{MultiplexId} (202 on success), modelled errors include ConflictException (errorKind client, httpError 409) with a single optional `Message` member (jsonName `message`). Recorded behavior only: deleting a multiplex whose State is RUNNING is refused 409 ConflictException with message 'Cannot delete a multiplex that is in RUNNING state'. That sentence is entirely FIXED text — it names the state class in the refusal but carries NO variable part: no multiplex id, no name, no timestamp. The effect must also be absent: a subsequent DescribeMultiplex still reports State RUNNING.
Emulator: refuses with the correct 409 ConflictException but the message 'Multiplex <id> is in state RUNNING and must be IDLE before it can be deleted' (observed: 'Multiplex 6000000 is in state RUNNING and must be IDLE before it can be deleted'; 6000000 is the created multiplex's id, a per-run value). So the emulator both re-words the fixed sentence and injects an identifier AWS does not put in this message.
Why: a caller that classifies this conflict by its message — retry/remediation logic that stops the multiplex and retries the delete, or a test that asserts the refusal text — does not recognise MediaLive's refusal. Worse, the emulator's wording interpolates a per-request id, so any client-side exact match or normalisation is impossible. Note the related open issue 20261028T160823Z-medialive-startmultiplex-error-response covers StartMultiplex against a DELETING multiplex, not DeleteMultiplex against a RUNNING one, so it does not cover this cause.
Fix: the state-precondition refusal in the DeleteMultiplex handler of the medialive crate (services/service-com-amazonaws-medialive-medialive): the 409 ConflictException it raises for a non-IDLE multiplex must carry MediaLive's fixed sentence for this error class rather than an emulator-authored, id-interpolating one.
Traces: medialive.multiplex-start-without-programs-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator (19/21 passed, 2 mismatched, 0 errored, bootstrap_signal false — medialive is bootstrapped and answering). Both failures are verdict=mismatch with the expected status and error code already matching; only the message diverges.

1. medialive.cloudwatch-alarm-template-group-hierarchy-crud, check_medialive_cloudwatch_alarm_template_group_hierarchy_crud.py line 52:
   AssertionError: expected no message reading deleted template, got 'cloudwatch alarm template 2000000 not found'
   The check creates a template group, creates a template in it, updates its Threshold (95.0 read back OK), deletes the template, then calls GetCloudWatchAlarmTemplate on the deleted Identifier. It asserts code == 'NotFoundException' (which PASSES) and then Message is None (which FAILS). The check aborts there, so the identical assertions it makes immediately afterwards on UpdateCloudWatchAlarmTemplate (and on the group-level get/update) were never reached — the update-template path goes through the same refusal helper and must be fixed with it.
   Smithy grounding: NotFoundException = {errorKind: client, httpError: 404, members: {Message: {target __string, jsonName 'message'}}} — Message optional. AWS leaves it absent here.

2. medialive.multiplex-start-without-programs-rejected, check_medialive_multiplex_start_without_programs_rejected.py line 53:
   AssertionError: Multiplex 6000000 is in state RUNNING and must be IDLE before it can be deleted
   (the assertion is `message == "Cannot delete a multiplex that is in RUNNING state"`).
   Everything before it passed: CreateMultiplex settles to IDLE, StartMultiplex with zero programs succeeds and reaches RUNNING, DescribeMultiplex reports ProgramCount 0 and PipelinesRunningCount >= 1, and DeleteMultiplex while RUNNING is correctly refused with code ConflictException. Only the message text differs.
   Smithy grounding: ConflictException = {errorKind: client, httpError: 409, members: {Message: {target __string, jsonName 'message'}}}; DeleteMultiplex declares ConflictException among its errors.

The ids 2000000 and 6000000 are values the checks' own creates produced; they fill the emulator's message templates and are not themselves requirements.

## Steps to Reproduce
1. Replay check medialive.cloudwatch-alarm-template-group-hierarchy-crud: CreateCloudWatchAlarmTemplateGroup, then CreateCloudWatchAlarmTemplate in that group (ComparisonOperator GreaterThanThreshold, EvaluationPeriods 1, MetricName ActiveAlerts, Period 300, Statistic Average, TargetResourceType MEDIALIVE_CHANNEL, Threshold 80.0, TreatMissingData notBreaching), DeleteCloudWatchAlarmTemplate, then GET /prod/cloudwatch-alarm-templates/<deleted-id>. The 404 NotFoundException body must carry NO 'message' member. Repeat with PATCH /prod/cloudwatch-alarm-templates/<deleted-id> (UpdateCloudWatchAlarmTemplate, Threshold 50.0) — same requirement.
2. Replay check medialive.multiplex-start-without-programs-rejected: CreateMultiplex (two AZs, TransportStreamBitrate 10000000, TransportStreamId 1), wait for State IDLE, StartMultiplex, wait for State RUNNING, then DELETE /prod/multiplexes/<id>. The refusal must be 409 ConflictException with message exactly 'Cannot delete a multiplex that is in RUNNING state' (no id interpolated), and a following DescribeMultiplex must still report State RUNNING.
3. In both cases the status and the error code already match; the failing assertion is on the message (its text in step 2, its absence in step 1).
