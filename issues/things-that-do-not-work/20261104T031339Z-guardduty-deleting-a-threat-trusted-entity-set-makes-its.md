Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.threatentityset-lifecycle guardduty.threatentityset-lifecycle-create-delete guardduty.trustedentityset-lifecycle-create-delete
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetThreatEntitySet|ResourceNotFoundException
Issue-Key: guardduty.threatentityset-lifecycle-create-delete|run#13|AssertionError|assert result in ("DELETE_PENDING", "DELETED", "BadRequestException"), (
Issue-Key: guardduty.trustedentityset-lifecycle-create-delete|run#13|AssertionError|assert result in ("DELETE_PENDING", "DELETED", "BadRequestException"), (
Issue-Commit: eb1ede2cbce186d0b7cae6d474f923ddb0f91b8a

# Deleting a Threat/Trusted entity set makes its id immediately unresolvable, so the DELETE_PENDING/DELETED statuses are never observable

> _Note: this issue was identified against the code base at commit `eb1ede2cbce186d0b7cae6d474f923ddb0f91b8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetThreatEntitySet

## Description
After a successful Delete{Threat,Trusted}EntitySet, the emulator treats the just-deleted entity set as an absent id: it keeps the record only as a tombstone (`crate::state::EntitySetRecord::is_deleting`) and the Get handlers filter it out, answering HTTP 404 `ResourceNotFoundException` / 'The specified entity set does not exist.' on the very next read. Real GuardDuty continues to answer the id while the asynchronous deletion runs and reports the deletion through the modelled lifecycle statuses, so a client can poll a delete to completion. 2 causes.

Cause 1 of 2: GetThreatEntitySet right after a successful DeleteThreatEntitySet.
AWS: DeleteThreatEntitySet starts an asynchronous deletion and the set stays addressable while it runs; GetThreatEntitySet keeps answering HTTP 200 with `status` moving DELETE_PENDING -> DELETED. Smithy: GetThreatEntitySet.Status targets `ThreatEntitySetStatus`, whose values are INACTIVE | ACTIVATING | ACTIVE | DEACTIVATING | ERROR | DELETE_PENDING | DELETED — the two delete states exist precisely because a deleting set is still readable; GetThreatEntitySet models only BadRequestException (400) and InternalServerErrorException (500). Recorded behavior: the replayed check accepts exactly one of three AWS-observed outcomes after the delete — `Status` DELETE_PENDING, `Status` DELETED, or a refusal with error code `BadRequestException` (HTTP 400) once the set is fully gone; it never observes 404 ResourceNotFoundException for an id this account just deleted.
Emulator: the first GetThreatEntitySet after the delete is refused with HTTP 404, error code `ResourceNotFoundException`, message 'The specified entity set does not exist.' (fixed text, no variable part), so neither DELETE_PENDING nor DELETED is ever observable and the refusal class is not one of the operation's modelled errors.
Why: a client that deletes and then polls Get until `Status` is DELETED (the documented completion signal) crashes on an exception class the operation does not model and cannot be caught typed for this operation; teardown code that distinguishes 'deletion in progress' from 'already gone' (BadRequestException) mis-branches, and nothing can confirm the deletion actually completed.
Traces: guardduty.threatentityset-lifecycle (GetThreatEntitySet raised ResourceNotFoundException from the post-delete poll), guardduty.threatentityset-lifecycle-create-delete (expected DELETE_PENDING/DELETED or BadRequestException, got ResourceNotFoundException).
Fix: the GetThreatEntitySet handler's id-resolution branch together with the delete/tombstone bookkeeping it reads — a set whose delete has been served must remain reportable with its deletion status for the retention window, and only then be refused the way AWS refuses a fully removed set.

Cause 2 of 2: GetTrustedEntitySet right after a successful DeleteTrustedEntitySet.
AWS: identical contract on the trusted family — Smithy: GetTrustedEntitySet.Status targets `TrustedEntitySetStatus` with the same values including DELETE_PENDING and DELETED, and the operation models only BadRequestException (400) and InternalServerErrorException (500). Recorded behavior: after DeleteTrustedEntitySet the set reports `Status` DELETE_PENDING/DELETED, or is refused with `BadRequestException` once gone.
Emulator: the first GetTrustedEntitySet after the delete is refused with HTTP 404, `ResourceNotFoundException`, 'The specified entity set does not exist.'
Why: as cause 1, on the trusted-entity-set family; a caller cannot observe or confirm trusted-entity-set deletion at all.
Traces: guardduty.trustedentityset-lifecycle-create-delete (expected DELETE_PENDING/DELETED or BadRequestException, got ResourceNotFoundException).
Fix: the GetTrustedEntitySet handler's id-resolution branch and the same delete/tombstone bookkeeping.

Note on an adjacent filed issue: 20261029T190006Z-guardduty-unknown-set-ids-are-refused-with-ownership-wording.md established that an UNKNOWN (never-minted) entity-set id is correctly refused HTTP 404 / ResourceNotFoundException / 'The specified entity set does not exist.' — that is not in question here and this issue does not reopen it. The divergence is that a set the caller JUST deleted is classified as that same never-existed absence, erasing the DELETE_PENDING/DELETED window.

Code citations: services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/get_threat_entity_set.rs lines 176-191 and get_trusted_entity_set.rs lines 174-189 (`.filter(|set| !set.is_deleting())` -> `Scaffolded::not_found(ENTITY_SET_NOT_FOUND_MESSAGE)`), and delete_threat_entity_set.rs lines 159-207 (comment: 'once the delete is served the detector no longer HAS the set, so nothing addresses it afterwards').

## Evidence
guardduty.threatentityset-lifecycle: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetThreatEntitySet operation: The specified entity set does not exist. — raised while polling GetThreatEntitySet for Status == 'DELETED' after DeleteThreatEntitySet returned 200. Expected: HTTP 200 with Status DELETE_PENDING then DELETED.
guardduty.threatentityset-lifecycle-create-delete: AssertionError: expected DELETE_PENDING/DELETED status or BadRequestException after delete, got ResourceNotFoundException.
guardduty.trustedentityset-lifecycle-create-delete: AssertionError: expected DELETE_PENDING/DELETED status or BadRequestException after delete, got ResourceNotFoundException.
Expected status/code/message after a served delete: HTTP 200 with Status 'DELETE_PENDING' or 'DELETED' (values from the modelled ThreatEntitySetStatus/TrustedEntitySetStatus enums), or, once the set is fully removed, HTTP 400 with error code 'BadRequestException'. Actual: HTTP 404, error code 'ResourceNotFoundException', message 'The specified entity set does not exist.' (entirely fixed text — it names neither the member nor the id, so no part of it comes from the request).

## Steps to Reproduce
1. CreateDetector(Enable=true).
2. CreateThreatEntitySet(DetectorId=<detector>, Name=<name>, Format='TXT', Location=<s3 list uri>, Activate=true) and note ThreatEntitySetId.
3. GetThreatEntitySet — succeeds with Status ACTIVATING/ACTIVE.
4. DeleteThreatEntitySet(DetectorId=<detector>, ThreatEntitySetId=<id>) — returns 200.
5. Immediately GetThreatEntitySet(DetectorId=<detector>, ThreatEntitySetId=<id>) and poll: the emulator answers HTTP 404 ResourceNotFoundException 'The specified entity set does not exist.' instead of HTTP 200 with Status DELETE_PENDING/DELETED (or HTTP 400 BadRequestException once fully gone).
6. Repeat steps 2-5 with CreateTrustedEntitySet / GetTrustedEntitySet / DeleteTrustedEntitySet for the trusted family.
7. Replay checks: guardduty.threatentityset-lifecycle, guardduty.threatentityset-lifecycle-create-delete, guardduty.trustedentityset-lifecycle-create-delete.
