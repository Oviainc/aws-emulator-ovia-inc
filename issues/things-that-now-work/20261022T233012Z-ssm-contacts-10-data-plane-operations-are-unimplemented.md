Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-contacts.activate-contact-channel-without-replication-set-rejected ssm-contacts.send-activation-code-without-replication-set-rejected ssm-contacts.create-rotation-without-replication-set-rejected ssm-contacts.list-rotations-without-replication-set-rejected ssm-contacts.describe-engagement-without-replication-set-rejected ssm-contacts.get-contact-policy-without-replication-set-rejected ssm-contacts.put-contact-policy-without-replication-set-rejected ssm-contacts.list-tags-for-resource-without-replication-set-rejected ssm-contacts.tag-resource-without-replication-set-rejected ssm-contacts.untag-resource-without-replication-set-rejected
Issue-Shape-Id: com.amazonaws.ssmcontacts#SSMContacts
Issue-Key: ssm-contacts.activate-contact-channel-without-replication-set-rejected|run#8|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.send-activation-code-without-replication-set-rejected|run#8|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.create-rotation-without-replication-set-rejected|run#7|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.list-rotations-without-replication-set-rejected|run#6|AssertionError|assert error.get("Code") == "ValidationException", (
Issue-Key: ssm-contacts.describe-engagement-without-replication-set-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: ssm-contacts.get-contact-policy-without-replication-set-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: ssm-contacts.put-contact-policy-without-replication-set-rejected|run#7|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.list-tags-for-resource-without-replication-set-rejected|run#5|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.tag-resource-without-replication-set-rejected|run#7|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.untag-resource-without-replication-set-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# ssm-contacts: 10 data-plane operations are unimplemented scaffolds — HTTP 501 NotImplemented instead of the modeled 400 ValidationException replication-set precondition

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ActivateContactChannel, SendActivationCode, CreateRotation, ListRotations, DescribeEngagement, GetContactPolicy, PutContactPolicy, ListTagsForResource, TagResource, UntagResource

## Description
Ten ssm-contacts operations are still scaffold stubs: each handler's `handle` body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("<Op>"))`, so every call answers HTTP 501 with error code `NotImplemented` and message "operation <Op> is scaffolded but not implemented in this emulator". Real AWS answers all ten with a modeled client error (HTTP 400 `ValidationException`) for the account-level Incident Manager replication-set precondition that every behavioral check in this cluster exercises: Amazon SSM Contacts is fronted by a per-account replication set, and until one exists the account has no entry in Incident Manager's replication-set-scoped registry, so the data-plane call is refused before the target resource is looked up. (Since AWS stopped accepting new Incident Manager customers on 2025-11-07, a fresh account can never satisfy this precondition, so this rejection is the only reachable behavior for these operations.) Each cause below is a distinct operation and a distinct handler file; all ten must be implemented before this issue is closed.

Cause 1 of 10: ActivateContactChannel.
AWS: Smithy ActivateContactChannel models errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(client, httpError 400); input ContactChannelId (SsmContactsArn, required, len 1..2048, pattern ^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:...) and ActivationCode (len 6..10, pattern ^[0-9]*$). For a well-formed but never-created contact-channel ARN in an account with no replication set the recorded response is HTTP 400 / code ValidationException; recorded message example "Invalid value provided - Account not found for the request" (the model does not state this wording, so the status and code are the requirement and the text is an example).
Emulator: HTTP 501, code NotImplemented.
Why: a client that gates its Incident Manager onboarding on a 400 ValidationException sees a 501 instead — botocore surfaces it as a non-modeled, non-retryable server-side failure, so error-path handling and onboarding fallbacks never run.
Fix: implement the handler in services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/activate_contact_channel.rs — consult the account's replication-set state and return the modeled ValidationException; delete the SCAFFOLDED marker.
Traces: ssm-contacts.activate-contact-channel-without-replication-set-rejected.

Cause 2 of 10: SendActivationCode.
AWS: Smithy SendActivationCode errors AccessDeniedException(403), DataEncryptionException(400), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400); input ContactChannelId (SsmContactsArn, required). Recorded: HTTP 400 / ValidationException, message example "Invalid value provided - Account not found for the request".
Emulator: HTTP 501, code NotImplemented.
Why: same as Cause 1 — the modeled 400 rejection class is unobservable.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/send_activation_code.rs.
Traces: ssm-contacts.send-activation-code-without-replication-set-rejected.

Cause 3 of 10: CreateRotation.
AWS: Smithy CreateRotation errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400); input Name, ContactIds (1..30), TimeZoneId, Recurrence required. Recorded: HTTP 400 / ValidationException, message example "Invalid value provided - Account not found for the request", and the check asserts the rejection is repeatable (an identical retry is rejected again, not admitted).
Emulator: HTTP 501, code NotImplemented.
Why: a client cannot distinguish "account not onboarded" from an emulator defect, and no rotation-creation error path is exercised.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/create_rotation.rs.
Traces: ssm-contacts.create-rotation-without-replication-set-rejected.

Cause 4 of 10: ListRotations.
AWS: Smithy ListRotations errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); paginated (NextToken/Rotations/MaxResults). Even this read-only list operation is gated on the account-level precondition: recorded HTTP 400 / ValidationException, message example "Invalid value provided - Account not found for the request" (unlike ListContacts, which is precondition-exempt and is used by these checks as a positive control and does succeed against the emulator).
Emulator: HTTP 501, code NotImplemented.
Why: clients that enumerate rotations to decide whether onboarding is needed get an unmodeled 501.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/list_rotations.rs.
Traces: ssm-contacts.list-rotations-without-replication-set-rejected.

Cause 5 of 10: DescribeEngagement.
AWS: Smithy DescribeEngagement errors AccessDeniedException(403), DataEncryptionException(400), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); input EngagementId (SsmContactsArn, required). Recorded for a well-formed but never-created engagement ARN: HTTP 400 / ValidationException with the ARN-naming variant of the message, example "Invalid value provided - Invalid resource Arn <EngagementId>" — i.e. the message is built from the request's ARN, so a correct implementation echoes the supplied ARN.
Emulator: HTTP 501, code NotImplemented.
Why: callers cannot tell an invalid/unknown engagement ARN from an emulator gap.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/describe_engagement.rs.
Traces: ssm-contacts.describe-engagement-without-replication-set-rejected.

Cause 6 of 10: GetContactPolicy.
AWS: Smithy GetContactPolicy errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); input ContactArn (SsmContactsArn, required). Recorded: HTTP 400 / ValidationException, message example "Invalid value provided - Account not found for the request" — the account precondition, not a 404 and not a policy-shape complaint.
Emulator: HTTP 501, code NotImplemented.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/get_contact_policy.rs.
Why: resource-policy read paths cannot be tested against the emulator at all.
Traces: ssm-contacts.get-contact-policy-without-replication-set-rejected.

Cause 7 of 10: PutContactPolicy.
AWS: Smithy PutContactPolicy errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); input ContactArn and Policy (len 1..395000) required. Recorded: HTTP 400 / ValidationException with the account-precondition message (example "Invalid value provided - Account not found for the request"), i.e. the account gate is evaluated before any policy-document validation.
Emulator: HTTP 501, code NotImplemented.
Why: a client relying on the precondition rejection ordering (account gate before policy validation) sees neither.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/put_contact_policy.rs.
Traces: ssm-contacts.put-contact-policy-without-replication-set-rejected.

Cause 8 of 10: ListTagsForResource.
AWS: Smithy ListTagsForResource errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); input ResourceARN (AmazonResourceName, required, len 1..1011). Recorded: HTTP 400 / ValidationException, message example "Invalid value provided - Account not found for the request".
Emulator: HTTP 501, code NotImplemented.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/list_tags_for_resource.rs.
Why: tag-read error handling is untestable and surfaces as a 501.
Traces: ssm-contacts.list-tags-for-resource-without-replication-set-rejected.

Cause 9 of 10: TagResource.
AWS: Smithy TagResource errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400); input ResourceARN and Tags required. Recorded: HTTP 400 / ValidationException (message example "Invalid value provided - Account not found for the request"), and the check also asserts the follow-up read is rejected the same way — no tag may be fabricated by the refused write.
Emulator: HTTP 501, code NotImplemented.
Why: a write that AWS refuses returns an unmodeled status, so no caller can assert the no-effect contract.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/tag_resource.rs.
Traces: ssm-contacts.tag-resource-without-replication-set-rejected.

Cause 10 of 10: UntagResource.
AWS: Smithy UntagResource errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); input ResourceARN and TagKeys required. Recorded: HTTP 400 / ValidationException, message example "Invalid value provided - Account not found for the request".
Emulator: HTTP 501, code NotImplemented.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/untag_resource.rs.
Why: same unmodeled 501 as Cause 9 on the inverse write.
Traces: ssm-contacts.untag-resource-without-replication-set-rejected.

Shared remedy: all ten handlers need the same account-level precondition consulted from the ssm-contacts service state (is there an active replication set for this account?) and, on absence, the modeled ValidationException serialized as HTTP 400 with code ValidationException. Related open stub issues exist for other ssm-contacts operations in the same family (GetRotation, ListRotationShifts, StartEngagement, StopEngagement, UpdateContact) and for ssm-incidents ListReplicationSets; those are filed separately and are NOT part of this issue.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service ssm-contacts: 1/24 checks passed. Verbatim failures for this cluster:

- ssm-contacts.activate-contact-channel-without-replication-set-rejected — `assert code == "ValidationException"` -> AssertionError: NotImplemented
- ssm-contacts.send-activation-code-without-replication-set-rejected — `assert code == "ValidationException"` -> AssertionError: NotImplemented
- ssm-contacts.create-rotation-without-replication-set-rejected — `assert code == "ValidationException"` -> AssertionError: NotImplemented
- ssm-contacts.list-rotations-without-replication-set-rejected — AssertionError: ListRotations without an active replication set returned 'NotImplemented', expected "ValidationException"
- ssm-contacts.describe-engagement-without-replication-set-rejected — AssertionError: expected ValidationException, got 'NotImplemented'
- ssm-contacts.get-contact-policy-without-replication-set-rejected — AssertionError: expected ValidationException, got NotImplemented
- ssm-contacts.put-contact-policy-without-replication-set-rejected — AssertionError: NotImplemented
- ssm-contacts.list-tags-for-resource-without-replication-set-rejected — AssertionError: NotImplemented
- ssm-contacts.tag-resource-without-replication-set-rejected — AssertionError: NotImplemented
- ssm-contacts.untag-resource-without-replication-set-rejected — AssertionError: expected ValidationException, got NotImplemented

Expected (per check + Smithy model): HTTP 400, Error.Code = "ValidationException" (modeled for every one of these operations with errorKind client / httpError 400); message example "Invalid value provided - Account not found for the request", or for DescribeEngagement "Invalid value provided - Invalid resource Arn <EngagementId>" (built from the request).

Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation <Op> is scaffolded but not implemented in this emulator".

Source of the 501: each op file still carries the scaffold marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("<Op>"))` — e.g. services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/activate_contact_channel.rs lines 4 and 162.

Positive control in every one of these checks: ListContacts (precondition-exempt) succeeds and returns an empty Contacts list, so the service is mounted and credentials are healthy — the 501s are per-operation gaps, not a service-level outage.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (git_clone, then run_behavioral_suite).
2. Point a botocore ssm-contacts client at the emulator in any region; confirm the control call `list_contacts(AliasPrefix=<unique>)` returns `{"Contacts": []}`.
3. Build a well-formed but never-created ARN, e.g. `arn:aws:ssm-contacts:<region>:<account>:contact/<prefix>` (and `.../contact-channel/<prefix>/email`, `.../engagement/<prefix>/<id>`).
4. Call each of: activate_contact_channel(ContactChannelId=<channel arn>, ActivationCode="000000"), send_activation_code(ContactChannelId=<channel arn>), create_rotation(...), list_rotations(RotationNamePrefix=<prefix>), describe_engagement(EngagementId=<engagement arn>), get_contact_policy(ContactArn=<contact arn>), put_contact_policy(ContactArn=<contact arn>, Policy=<json>), list_tags_for_resource(ResourceARN=<contact arn>), tag_resource(ResourceARN=<contact arn>, Tags=[...]), untag_resource(ResourceARN=<contact arn>, TagKeys=[...]).
5. Observe each raises ClientError with HTTP 501 and Error.Code "NotImplemented".
6. Expected: HTTP 400 with Error.Code "ValidationException" (the account-level replication-set precondition), with a message naming the precondition or, for DescribeEngagement, the supplied ARN.
7. Re-replay the ten check ids in trace_ids to confirm a fix.
