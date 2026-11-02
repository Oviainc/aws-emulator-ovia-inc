Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.accept-invitation-without-pending-invite-rejected
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty.accept-invitation-without-pending-invite-rejected|run#9|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Commit: 9b689e7a3706c4d3e9e7bd76513ebd3e34560844

# AcceptInvitation refuses a non-pending (MasterId, InvitationId) pair with an emulator-authored reason instead of GuardDuty's fixed invalid-input sentence

> _Note: this issue was identified against the code base at commit `9b689e7a3706c4d3e9e7bd76513ebd3e34560844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptInvitation

## Description
One cause.

Cause 1 of 1: AcceptInvitation called with a MasterId/InvitationId pair that identifies no invitation pending for the caller's account is refused with the correct error code and status but with a message the service never emits.

AWS: Smithy (services/service-com-amazonaws-guardduty-guarddutyapiservice/smithy-model/com-amazonaws-guardduty-guarddutyapiservice.json) models AcceptInvitation with errors BadRequestException (httpError 400) and InternalServerErrorException (500); MasterId (jsonName masterId) and InvitationId (jsonName invitationId) are required Strings with no modelled pattern, so a pair that resolves to nothing is a parameter-level refusal. Recorded behavior: HTTP 400, error code BadRequestException, message 'The request is rejected because an invalid or out-of-range value is specified as an input parameter.' That sentence is entirely FIXED text — it interpolates no part of the request or of service state, and it deliberately names neither masterId nor invitationId nor the pending-invitation rule.

Emulator: HTTP 400, code BadRequestException, message 'The request is rejected because the input masterId and invitationId do not identify an invitation pending for the current account.' — a semantic explanation of the lookup failure that AWS does not give. The text is produced by crate::state::invitation_not_pending_message(), reached from the invitation_pending() branch of the AcceptInvitation handler.

Why: a client that classifies GuardDuty failures by the documented refusal text (log/alert matching, retry-vs-fail decisions, test assertions written against the emulator) never matches the generic invalid-input sentence, so code validated against the emulator misclassifies the same failure on real AWS. The emulator also asserts a specific cause (the pair is not pending) where AWS asserts only that an input parameter was invalid or out of range, so an operator acts on a reason the service never stated.

Fix: the AcceptInvitation handler's non-pending-invitation refusal branch and the shared message helper it calls — the refusal must carry GuardDuty's fixed invalid-input sentence, not a reason derived from which lookup failed. Status (400) and code (BadRequestException) are already correct and must not change; the surrounding effects the check also verifies (no administrator relationship recorded, the detector still ENABLED and readable) are already correct.

Related open issue, which does NOT cover this check and carries no replay key for it: 20261029T190006Z-guardduty-invitation-and-administrator-relationship-refusals-report-the-wrong.md (its cause 2 describes the same wording defect from a wire fixture; this behavioral check is additional grounding for the same handler branch).

Traces: behavioral check guardduty.accept-invitation-without-pending-invite-rejected; handler at services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/accept_invitation.rs lines 177-181.

## Evidence
Check guardduty.accept-invitation-without-pending-invite-rejected (verdict: mismatch).

CreateDetector(Enable=true) -> DetectorId; then AcceptInvitation(DetectorId=<id>, MasterId='111122223333', InvitationId='sim2real-bogus-invitation-id').

Error code: BadRequestException on both sides (HTTP 400) — matches.
Error message:
  expected (AWS, fixed text, no interpolated values): 'The request is rejected because an invalid or out-of-range value is specified as an input parameter.'
  got (emulator): 'The request is rejected because the input masterId and invitationId do not identify an invitation pending for the current account.'

Assertion raised: AssertionError: expected 'The request is rejected because an invalid or out-of-range value is specified as an input parameter.', got 'The request is rejected because the input masterId and invitationId do not identify an invitation pending for the current account.' (check_guardduty_accept_invitation_without_pending_invite_rejected.py line 36).

The check's remaining assertions pass: GetMasterAccount reports no AccountId (no relationship established) and GetDetector still reports Status ENABLED, so the divergence is confined to the refusal message.

## Steps to Reproduce
1. Boot the emulator and point a botocore guardduty client at it.
2. detector_id = create_detector(Enable=True)['DetectorId'].
3. Call accept_invitation(DetectorId=detector_id, MasterId='111122223333', InvitationId=<any id that was never issued to this account>).
4. Catch the ClientError and inspect response['Error']['Code'] and ['Message']: the code is BadRequestException (correct) but the message is the emulator's 'input masterId and invitationId do not identify an invitation pending for the current account' sentence instead of AWS's fixed 'The request is rejected because an invalid or out-of-range value is specified as an input parameter.'
5. Equivalently, re-replay behavioral check guardduty.accept-invitation-without-pending-invite-rejected.
