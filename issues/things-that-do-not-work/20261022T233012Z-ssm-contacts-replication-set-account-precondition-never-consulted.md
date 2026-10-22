Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-contacts.get-contact-without-replication-set-rejected ssm-contacts.delete-contact-without-replication-set-rejected ssm-contacts.create-contact-channel-without-replication-set-rejected ssm-contacts.create-contact-without-replication-set-rejected
Issue-Shape-Id: com.amazonaws.ssmcontacts#SSMContacts
Issue-Key: ssm-contacts.get-contact-without-replication-set-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: ssm-contacts.delete-contact-without-replication-set-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: ssm-contacts.create-contact-channel-without-replication-set-rejected|run#8|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.create-contact-without-replication-set-rejected|_create#6|AssertionError|raise AssertionError(
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# ssm-contacts: replication-set account precondition never consulted — GetContact/DeleteContact/CreateContactChannel answer 404 ResourceNotFoundException and CreateContact succeeds, instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetContact, DeleteContact, CreateContactChannel, CreateContact

## Description
These four ssm-contacts operations ARE implemented in the emulator, but none of them consults the account-level Incident Manager replication-set precondition. Amazon SSM Contacts is fronted by a per-account replication set; until one exists the account has no entry in Incident Manager's replication-set-scoped registry and the data-plane call is refused with HTTP 400 ValidationException BEFORE the target resource is looked up. The emulator instead goes straight to its resource store, so it either reports the resource missing (404) or, worse, services the call. 4 causes.

Cause 1 of 4: GetContact returns the wrong rejection class.
AWS: Smithy GetContact models both ResourceNotFoundException (client, httpError 404) and ValidationException (client, httpError 400); input ContactId (SsmContactsArn, required, pattern ^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:...). For a syntactically well-formed but never-created ContactId in an account with no active replication set the recorded response is HTTP 400 / code ValidationException — explicitly NOT ResourceNotFoundException, because the account gate is evaluated first. Recorded message example: "Invalid value provided - Account not found for the request".
Emulator: HTTP 404 / code ResourceNotFoundException.
Why: a client branches on the error class — ValidationException means "this account is not onboarded, run replication-set bootstrap (or fail permanently)", ResourceNotFoundException means "the account is fine, just create this contact". Against the emulator the client takes the create branch, which in AWS would be refused as well.
Fix: the GetContact handler's precondition check in the ssm-contacts service (services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/get_contact.rs) — consult the replication-set state held in crate::state before the store lookup.
Traces: ssm-contacts.get-contact-without-replication-set-rejected.

Cause 2 of 4: DeleteContact returns the wrong rejection class.
AWS: Smithy DeleteContact models ResourceNotFoundException (404), ConflictException (409) and ValidationException (client, httpError 400); input ContactId required. Recorded for a well-formed but never-created ContactId with no replication set: HTTP 400 / ValidationException, message example "Invalid value provided - Account not found for the request".
Emulator: HTTP 404 / code ResourceNotFoundException.
Why: idempotent-delete logic that treats 404 as "already gone" silently succeeds against the emulator, where AWS would hard-fail the account precondition.
Fix: same precondition check in services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/delete_contact.rs.
Traces: ssm-contacts.delete-contact-without-replication-set-rejected.

Cause 3 of 4: CreateContactChannel answers an error its own model does not declare.
AWS: Smithy CreateContactChannel models AccessDeniedException(403), ConflictException(409), DataEncryptionException(400), InternalServerException(500), ThrottlingException(429), ValidationException(client, httpError 400) — ResourceNotFoundException is NOT in CreateContactChannel's error list at all. For a well-formed but never-created ContactId in a non-onboarded account the recorded response is HTTP 400 / ValidationException, message example "Invalid value provided - Account not found for the request".
Emulator: HTTP 404 / code ResourceNotFoundException — an error class outside the operation's modeled closure, so generated SDK clients cannot map it to a typed exception.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/create_contact_channel.rs — the missing-parent/missing-account path must return the modeled ValidationException (400), never ResourceNotFoundException.
Why: a strongly typed client (Java/Rust/Go) receives an unmodeled error shape and falls through to a generic service exception, breaking retry/branch logic entirely.
Traces: ssm-contacts.create-contact-channel-without-replication-set-rejected.

Cause 4 of 4: CreateContact is serviced where AWS refuses it.
AWS: Smithy CreateContact models ValidationException (client, httpError 400) with members Message (required), Reason, Fields. With no active replication set the create is refused: recorded HTTP 400 / ValidationException, message example "Invalid value provided - Account not found for the request", and the refusal is repeatable (an identical retry is refused again, never admitted on the second attempt).
Emulator: the call SUCCEEDS (HTTP 200 with a ContactArn), so the check fails at "real AWS rejects CreateContact without an active replication set, but this call SUCCEEDED".
Why: the client ends up holding a ContactArn for a contact that could not exist in AWS, and every error-path test asserting ValidationException/400 passes vacuously. NOTE: the already-open wire-corpus issue "CreateContact succeeds with no replication set instead of ValidationException" (20261022T020639Z-ssm-contacts-createcontact-succeeds-with-no-replication-set-instead-of.md, Issue-Key ssm-contacts|CreateContact|4xx->2xx) describes this same root cause from the wire corpus; this behavioral check was not marked OWNED by it, so it is recorded here and the two should be resolved together.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/create_contact.rs — precondition check before admitting the write, returning the modeled ValidationException through the error serializer.
Traces: ssm-contacts.create-contact-without-replication-set-rejected.

All four causes share one missing piece of state: the emulator has no notion of "this account has/has not an active Incident Manager replication set", so the gate cannot be evaluated anywhere. Implementing that state once and consulting it from each handler closes all four.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service ssm-contacts. Verbatim failures:

- ssm-contacts.get-contact-without-replication-set-rejected — AssertionError: expected ValidationException, got ResourceNotFoundException
- ssm-contacts.delete-contact-without-replication-set-rejected — AssertionError: expected ValidationException, got 'ResourceNotFoundException'
- ssm-contacts.create-contact-channel-without-replication-set-rejected — `assert code == "ValidationException"` -> AssertionError: ResourceNotFoundException
- ssm-contacts.create-contact-without-replication-set-rejected — AssertionError: real AWS rejects CreateContact without an active replication set, but this call SUCCEEDED

Expected (recorded AWS + Smithy): HTTP 400 with Error.Code "ValidationException"; message example "Invalid value provided - Account not found for the request". ValidationException is modeled on all four operations (errorKind client, httpError 400). ResourceNotFoundException is modeled (404) on GetContact and DeleteContact but is NOT in CreateContactChannel's modeled error list.

Actual (emulator): 404 / ResourceNotFoundException for GetContact, DeleteContact and CreateContactChannel; 200 success with a ContactArn for CreateContact (and the check's retry leg shows the same call is admitted repeatedly).

Positive control: ListContacts (precondition-exempt) returns an empty Contacts list before and after each rejected call in all four checks, so the service is mounted and healthy; the effect-absent reads also confirm the emulator created nothing for the three 404 cases.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; point a botocore ssm-contacts client at it.
2. Control: `list_contacts(AliasPrefix=<unique-prefix>)` -> {"Contacts": []}.
3. Build a well-formed but never-created ARN: `arn:aws:ssm-contacts:<region>:<account>:contact/<unique-prefix>`.
4. `get_contact(ContactId=<arn>)` -> emulator raises ResourceNotFoundException (404); expected ValidationException (400).
5. `delete_contact(ContactId=<arn>)` -> emulator raises ResourceNotFoundException (404); expected ValidationException (400).
6. `create_contact_channel(ContactId=<arn>, Name=..., Type="EMAIL", DeliveryAddress={"SimpleAddress": "..."})` -> emulator raises ResourceNotFoundException (404), an error not modeled for this operation; expected ValidationException (400).
7. `create_contact(Alias=<unique>, Type="PERSONAL", Plan={"Stages": []})` -> emulator returns 200 with a ContactArn; expected ValidationException (400). Repeat the identical call: AWS refuses again.
8. Re-replay the four check ids in trace_ids to confirm a fix.
