Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-contacts.list-rotations-without-replication-set-rejected ssm-contacts.tag-resource-without-replication-set-rejected ssm-contacts.untag-resource-without-replication-set-rejected ssm-contacts.update-contact-without-replication-set-rejected
Issue-Shape-Id: com.amazonaws.ssmcontacts#SSMContacts
Issue-Key: ssm-contacts.list-rotations-without-replication-set-rejected|_error_code#5|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ssm-contacts.tag-resource-without-replication-set-rejected|run#7|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.untag-resource-without-replication-set-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: ssm-contacts.update-contact-without-replication-set-rejected|run#8|AssertionError|assert err["Code"] == "ValidationException", err
Issue-Commit: e00f7811d779d7cb6d7a0b5f4853bb8e7cdcc519

# ssm-contacts: replication-set gate skipped on TagResource/UntagResource/UpdateContact (404 instead of 400 ValidationException) and ListRotations is serviced with 200 where AWS refuses it

> _Note: this issue was identified against the code base at commit `e00f7811d779d7cb6d7a0b5f4853bb8e7cdcc519`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRotations

## Description
Continuation of the same account-level gap, for four more operations. Amazon SSM Contacts refuses every non-exempt data-plane call with HTTP 400 / ValidationException while the account has no active Incident Manager replication set, before the target resource is looked up. Only the alias-scoped list operation ListContacts is precondition-exempt. The emulator never evaluates that gate: three of these operations answer the resource-level 404 instead, and ListRotations is serviced outright with a 200. The open issue 20261022T233012Z-ssm-contacts-replication-set-account-precondition-never-consulted.md describes the same missing state for GetContact/DeleteContact/CreateContactChannel/CreateContact but does not cover these four operations. 4 causes.

Cause 1 of 4: ListRotations is serviced where AWS refuses it, and is wrongly treated as precondition-exempt.
AWS: Smithy ListRotations input RotationNamePrefix/NextToken/MaxResults (all optional), output Rotations (required list); modeled errors include ValidationException (errorKind client, httpError 400) and ResourceNotFoundException (client, 404). Unlike ListContacts, ListRotations is NOT precondition-exempt: with no active replication set the recorded response is a refusal, HTTP 400 / code ValidationException, message example "Invalid value provided - Account not found for the request".
Emulator: HTTP 200 with body {"Rotations": []} — the call is admitted and answers an empty page.
Why: this is the worst class of divergence for a client: an "empty but healthy" answer. Inventory/reconciliation code concludes the account has zero rotations and proceeds to create them (or reports drift), where AWS would have told it the account is not onboarded at all. Every error-path test asserting ValidationException on this operation also passes vacuously.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/list_rotations.rs — the replication-set precondition must be consulted before the list is served, and ListRotations must not share ListContacts' exemption.
Traces: ssm-contacts.list-rotations-without-replication-set-rejected.

Cause 2 of 4: TagResource returns the wrong rejection class.
AWS: Smithy TagResource input ResourceARN (AmazonResourceName, required, @length 1..1011) and Tags (required, @length 0..50); modeled errors include ValidationException (client, 400) and ResourceNotFoundException (client, 404). Recorded for a well-formed but never-created ResourceARN with no replication set: HTTP 400 / ValidationException.
Emulator: HTTP 404 / code ResourceNotFoundException.
Why: tagging automation retries or skips on 404 ("resource not created yet") instead of surfacing the permanent account refusal, so the run reports partial success where AWS would fail it.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/tag_resource.rs — gate check before the store lookup.
Traces: ssm-contacts.tag-resource-without-replication-set-rejected.

Cause 3 of 4: UntagResource returns the wrong rejection class.
AWS: Smithy UntagResource input ResourceARN (required) and TagKeys (required); modeled errors include ValidationException (client, 400) and ResourceNotFoundException (client, 404). Recorded with no replication set: HTTP 400 / ValidationException.
Emulator: HTTP 404 / code ResourceNotFoundException.
Why: idempotent-untag logic treats 404 as "nothing to remove" and succeeds, masking the account precondition entirely.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/untag_resource.rs.
Traces: ssm-contacts.untag-resource-without-replication-set-rejected.

Cause 4 of 4: UpdateContact returns the wrong rejection class.
AWS: Smithy UpdateContact input ContactId (SsmContactsArn, required, pattern ^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:...), DisplayName and Plan optional; modeled errors include ValidationException (client, 400) and ResourceNotFoundException (client, 404). Recorded for a well-formed but never-created ContactId with no replication set: HTTP 400 / ValidationException.
Emulator: HTTP 404 / code ResourceNotFoundException, Error.Message example "Contact arn:aws:ssm-contacts:<region>:<account>:contact/<prefix> not found".
Why: upsert code branches on 404 to mean "create it instead", issuing a CreateContact that AWS also refuses; the client never learns the account is un-onboarded. (Separate from the open issue 20261024T063255Z-ssm-contacts-updatecontact-validation.md, which is about Plan ARN resolution on an existing contact.)
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/update_contact.rs.
Traces: ssm-contacts.update-contact-without-replication-set-rejected.

All four causes depend on the same missing state — an account-level "has an active replication set" fact in crate::state — plus, for Cause 1, a correct exemption list that contains ListContacts but not ListRotations.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, service ssm-contacts (3/20 passed). Verbatim failures:

- ssm-contacts.list-rotations-without-replication-set-rejected — AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'Rotations': [], 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', ...}}}
- ssm-contacts.tag-resource-without-replication-set-rejected — `assert code == "ValidationException", code` -> AssertionError: ResourceNotFoundException
- ssm-contacts.untag-resource-without-replication-set-rejected — AssertionError: expected ValidationException, got ResourceNotFoundException
- ssm-contacts.update-contact-without-replication-set-rejected — AssertionError: {'Message': 'Contact arn:aws:ssm-contacts:us-west-2:<account>:contact/<prefix> not found', 'Code': 'ResourceNotFoundException'}

Expected (recorded AWS + Smithy): HTTP 400 with Error.Code "ValidationException" on all four; recorded message example "Invalid value provided - Account not found for the request" (example only — the requirement is the 400 / ValidationException pair). ValidationException is modeled errorKind client / httpError 400 on ListRotations, TagResource, UntagResource and UpdateContact.

Actual (emulator): HTTP 200 with {"Rotations": []} for ListRotations; HTTP 404 with Error.Code "ResourceNotFoundException" for TagResource, UntagResource and UpdateContact.

Positive control: the precondition-exempt ListContacts(AliasPrefix=<unique-prefix>) returns {"Contacts": []} before and after each call, so the service is mounted and healthy and the checks' effect-absent reads show no state was created.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; point a botocore ssm-contacts client at it in an account with no Incident Manager replication set (the emulator's default state).
2. Control: `list_contacts(AliasPrefix=<unique-prefix>)` -> {"Contacts": []}.
3. `list_rotations(RotationNamePrefix=<unique-prefix>)` -> emulator returns HTTP 200 {"Rotations": []}; expected ClientError with Error.Code "ValidationException" and HTTP 400.
4. Build a well-formed but never-created ARN: `arn:aws:ssm-contacts:<region>:<account>:contact/<unique-prefix>`.
5. `tag_resource(ResourceARN=<arn>, Tags=[{"Key": "k", "Value": "v"}])` -> emulator raises ResourceNotFoundException (404); expected ValidationException (400).
6. `untag_resource(ResourceARN=<arn>, TagKeys=["k"])` -> emulator raises ResourceNotFoundException (404); expected ValidationException (400).
7. `update_contact(ContactId=<arn>, DisplayName="x")` -> emulator raises ResourceNotFoundException (404) with message "Contact <arn> not found"; expected ValidationException (400).
8. Re-replay the four check ids listed in trace_ids to confirm a fix.
