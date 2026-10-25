Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-contacts.activate-contact-channel-without-replication-set-rejected ssm-contacts.create-rotation-without-replication-set-rejected ssm-contacts.describe-engagement-without-replication-set-rejected ssm-contacts.get-contact-policy-without-replication-set-rejected ssm-contacts.get-rotation-without-replication-set-rejected ssm-contacts.list-tags-for-resource-without-replication-set-rejected ssm-contacts.put-contact-policy-without-replication-set-rejected ssm-contacts.send-activation-code-without-replication-set-rejected ssm-contacts.start-engagement-without-replication-set-rejected ssm-contacts.stop-engagement-without-replication-set-rejected
Issue-Shape-Id: com.amazonaws.ssmcontacts#SSMContacts
Issue-Key: ssm-contacts.activate-contact-channel-without-replication-set-rejected|run#8|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.create-rotation-without-replication-set-rejected|run#7|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.describe-engagement-without-replication-set-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: ssm-contacts.get-contact-policy-without-replication-set-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: ssm-contacts|GetRotation|ResourceNotFoundException
Issue-Key: ssm-contacts.list-tags-for-resource-without-replication-set-rejected|run#5|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.put-contact-policy-without-replication-set-rejected|run#7|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.send-activation-code-without-replication-set-rejected|run#8|AssertionError|assert code == "ValidationException", code
Issue-Key: ssm-contacts.start-engagement-without-replication-set-rejected|run#6|AssertionError|assert code == "ValidationException", f"{code}: {message}"
Issue-Key: ssm-contacts.stop-engagement-without-replication-set-rejected|run#5|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message}"
Issue-Commit: e00f7811d779d7cb6d7a0b5f4853bb8e7cdcc519

# ssm-contacts: replication-set account precondition skipped on 10 more operations — 404 ResourceNotFoundException instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `e00f7811d779d7cb6d7a0b5f4853bb8e7cdcc519`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ActivateContactChannel

## Description
The Amazon SSM Contacts API is fronted by a per-account Incident Manager replication set. Until that replication set exists, the account has no entry in Incident Manager's replication-set-scoped registry and every data-plane call (except the precondition-exempt list operations such as ListContacts) is refused with HTTP 400 / ValidationException BEFORE the target resource is looked up. The emulator skips that account gate entirely on the operations below and goes straight to its resource store, so it reports the resource missing (HTTP 404 / ResourceNotFoundException) instead of refusing the account. This is the same missing piece of state already described for four other operations in the open issue 20261022T233012Z-ssm-contacts-replication-set-account-precondition-never-consulted.md (GetContact/DeleteContact/CreateContactChannel/CreateContact) and for GetContactChannel in 20261024T063255Z-ssm-contacts-getcontactchannel-error-response.md; that issue does not cover these ten operations, so they are recorded here and should be resolved with it. 10 causes.

Cause 1 of 10: ActivateContactChannel returns the wrong rejection class.
AWS: Smithy ActivateContactChannel input ContactChannelId (SsmContactsArn, required, @length 1..2048, pattern ^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:...); modeled errors include ValidationException (errorKind client, httpError 400) and ResourceNotFoundException (client, 404). For a syntactically well-formed but never-created ContactChannelId ARN in an account with no active replication set the recorded response is HTTP 400 / code ValidationException — not ResourceNotFoundException, because the account gate is evaluated first. Recorded message example: "Invalid value provided - Account not found for the request".
Emulator: HTTP 404 / code ResourceNotFoundException.
Why: a client branches on the error class — ValidationException means "this account is not onboarded" (permanent, no restore leg), ResourceNotFoundException means "the account is fine, just create the channel". Against the emulator the client takes the create branch, which AWS would also refuse.
Fix: the ActivateContactChannel handler's precondition check in services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/activate_contact_channel.rs — consult replication-set state in crate::state before the store lookup.
Traces: ssm-contacts.activate-contact-channel-without-replication-set-rejected.

Cause 2 of 10: CreateRotation returns the wrong rejection class.
AWS: Smithy CreateRotation input Name/ContactIds/TimeZoneId/Recurrence all required; modeled errors include ValidationException (client, 400) and ResourceNotFoundException (client, 404). Recorded with no active replication set: HTTP 400 / ValidationException, message example "Invalid value provided - Account not found for the request".
Emulator: HTTP 404 / code ResourceNotFoundException.
Why: the client concludes the referenced contacts are merely missing and retries after creating them, instead of treating the account as permanently un-onboarded.
Fix: the same precondition check in services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/create_rotation.rs, ahead of ContactIds resolution.
Traces: ssm-contacts.create-rotation-without-replication-set-rejected.

Cause 3 of 10: DescribeEngagement returns the wrong rejection class.
AWS: Smithy DescribeEngagement input EngagementId (SsmContactsArn, required); modeled errors include ValidationException (client, 400) and ResourceNotFoundException (client, 404). Recorded for a well-formed but never-created EngagementId with no replication set: HTTP 400 / ValidationException.
Emulator: HTTP 404 / code ResourceNotFoundException.
Why: engagement-polling code that treats 404 as "engagement not started yet" loops forever against the emulator where AWS hard-fails the account precondition.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/describe_engagement.rs.
Traces: ssm-contacts.describe-engagement-without-replication-set-rejected.

Cause 4 of 10: GetContactPolicy returns the wrong rejection class.
AWS: Smithy GetContactPolicy input ContactArn (SsmContactsArn, required); modeled errors include ValidationException (client, 400) and ResourceNotFoundException (client, 404). Recorded with no replication set: HTTP 400 / ValidationException.
Emulator: HTTP 404 / code ResourceNotFoundException.
Why: policy-reconciliation code reads 404 as "no policy attached yet" and proceeds to PutContactPolicy, a call AWS also refuses.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/get_contact_policy.rs.
Traces: ssm-contacts.get-contact-policy-without-replication-set-rejected.

Cause 5 of 10: GetRotation returns the wrong rejection class.
AWS: Smithy GetRotation input RotationId (SsmContactsArn, required); modeled errors include ValidationException (client, 400) and ResourceNotFoundException (client, 404). Recorded with no replication set: HTTP 400 / ValidationException.
Emulator: HTTP 404 / code ResourceNotFoundException.
Why: same misclassification — the client believes the account is healthy and the rotation merely absent.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/get_rotation.rs.
Traces: ssm-contacts.get-rotation-without-replication-set-rejected.

Cause 6 of 10: ListTagsForResource returns the wrong rejection class.
AWS: Smithy ListTagsForResource input ResourceARN (AmazonResourceName, required, @length 1..1011); modeled errors include ValidationException (client, 400) and ResourceNotFoundException (client, 404). Recorded with no replication set: HTTP 400 / ValidationException.
Emulator: HTTP 404 / code ResourceNotFoundException.
Why: tag-sync tooling treats 404 as "resource has no tags" and silently continues. (Distinct from the open issue 20261024T155757Z-ssm-contacts-listtagsforresource-error-response.md, which covers a ResourceARN that is not an ARN at all; here the ARN is well formed and the account gate is what must fire.)
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/list_tags_for_resource.rs.
Traces: ssm-contacts.list-tags-for-resource-without-replication-set-rejected.

Cause 7 of 10: PutContactPolicy returns the wrong rejection class.
AWS: Smithy PutContactPolicy input ContactArn and Policy (both required, Policy @length 1..395000); modeled errors include ValidationException (client, 400), ConflictException (409) and ResourceNotFoundException (client, 404). Recorded with no replication set: HTTP 400 / ValidationException.
Emulator: HTTP 404 / code ResourceNotFoundException.
Why: a write path that distinguishes "target missing" from "account refused" takes the wrong recovery branch.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/put_contact_policy.rs.
Traces: ssm-contacts.put-contact-policy-without-replication-set-rejected.

Cause 8 of 10: SendActivationCode returns the wrong rejection class.
AWS: Smithy SendActivationCode input ContactChannelId (SsmContactsArn, required); modeled errors include ValidationException (client, 400) and ResourceNotFoundException (client, 404). Recorded with no replication set: HTTP 400 / ValidationException.
Emulator: HTTP 404 / code ResourceNotFoundException.
Why: channel-activation flows retry on 404 waiting for the channel to appear instead of surfacing the permanent account refusal.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/send_activation_code.rs.
Traces: ssm-contacts.send-activation-code-without-replication-set-rejected.

Cause 9 of 10: StartEngagement returns the wrong rejection class.
AWS: Smithy StartEngagement input ContactId/Sender/Subject/Content required; modeled errors include ValidationException (client, 400) and ResourceNotFoundException (client, 404). Recorded with no replication set: HTTP 400 / ValidationException.
Emulator: HTTP 404 / code ResourceNotFoundException (observed message example "Contact <arn> not found").
Why: incident-response automation treats 404 as a transient/missing-contact condition and keeps paging, where AWS refuses the account outright.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/start_engagement.rs.
Traces: ssm-contacts.start-engagement-without-replication-set-rejected.

Cause 10 of 10: StopEngagement returns the wrong rejection class.
AWS: Smithy StopEngagement input EngagementId (SsmContactsArn, required); modeled errors include ValidationException (client, 400) and ResourceNotFoundException (client, 404). Recorded with no replication set: HTTP 400 / ValidationException.
Emulator: HTTP 404 / code ResourceNotFoundException (observed message example "Engagement <arn> not found").
Why: cleanup paths that swallow 404 as "already stopped" succeed vacuously against the emulator.
Fix: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/stop_engagement.rs.
Traces: ssm-contacts.stop-engagement-without-replication-set-rejected.

All ten causes share the one missing piece of state named in the open precondition issue: the emulator has no notion of "this account has / has not an active Incident Manager replication set". Implementing that state once and consulting it from each of these handlers (before any store lookup) closes all ten.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, service ssm-contacts (3/20 passed). Verbatim failures:

- ssm-contacts.activate-contact-channel-without-replication-set-rejected — `assert code == "ValidationException", code` -> AssertionError: ResourceNotFoundException
- ssm-contacts.create-rotation-without-replication-set-rejected — `assert code == "ValidationException", code` -> AssertionError: ResourceNotFoundException
- ssm-contacts.describe-engagement-without-replication-set-rejected — AssertionError: expected ValidationException, got 'ResourceNotFoundException'
- ssm-contacts.get-contact-policy-without-replication-set-rejected — AssertionError: expected ValidationException, got ResourceNotFoundException
- ssm-contacts.get-rotation-without-replication-set-rejected — `assert code == "ValidationException", code` -> AssertionError: ResourceNotFoundException
- ssm-contacts.list-tags-for-resource-without-replication-set-rejected — `assert code == "ValidationException", code` -> AssertionError: ResourceNotFoundException
- ssm-contacts.put-contact-policy-without-replication-set-rejected — `assert code == "ValidationException", code` -> AssertionError: ResourceNotFoundException
- ssm-contacts.send-activation-code-without-replication-set-rejected — `assert code == "ValidationException", code` -> AssertionError: ResourceNotFoundException
- ssm-contacts.start-engagement-without-replication-set-rejected — AssertionError: ResourceNotFoundException: Contact arn:aws:ssm-contacts:us-west-2:<account>:contact/<prefix> not found
- ssm-contacts.stop-engagement-without-replication-set-rejected — AssertionError: expected ValidationException, got ResourceNotFoundException: Engagement arn:aws:ssm-contacts:us-west-2:<account>:engagement/<id>/<id> not found

Expected (recorded AWS + Smithy): HTTP 400 with Error.Code "ValidationException"; recorded message example "Invalid value provided - Account not found for the request" (an example, the requirement is the 400/ValidationException pair). ValidationException is modeled errorKind client / httpError 400 on all ten operations.

Actual (emulator): HTTP 404 with Error.Code "ResourceNotFoundException" on all ten.

Positive control: the precondition-exempt ListContacts(AliasPrefix=<unique-prefix>) returns {"Contacts": []} before and after each rejected call in every one of these checks, so the service is mounted, routed and healthy — only the account gate is missing. The same reads confirm the emulator created nothing under the prefix.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; point a botocore ssm-contacts client at it in an account with no Incident Manager replication set (the emulator's default state).
2. Control: `list_contacts(AliasPrefix=<unique-prefix>)` -> {"Contacts": []}.
3. Build well-formed but never-created ARNs, e.g. `arn:aws:ssm-contacts:<region>:<account>:contact/<unique-prefix>`, `.../contact-channel/<unique-prefix>/email`, `.../rotation/<unique-prefix>`, `.../engagement/<uuid>/<uuid>`.
4. Call each of: `activate_contact_channel(ContactChannelId=<channel-arn>, ActivationCode="000000")`, `create_rotation(Name=..., ContactIds=[<contact-arn>], TimeZoneId="UTC", Recurrence=...)`, `describe_engagement(EngagementId=<engagement-arn>)`, `get_contact_policy(ContactArn=<contact-arn>)`, `get_rotation(RotationId=<rotation-arn>)`, `list_tags_for_resource(ResourceARN=<contact-arn>)`, `put_contact_policy(ContactArn=<contact-arn>, Policy=<json>)`, `send_activation_code(ContactChannelId=<channel-arn>)`, `start_engagement(ContactId=<contact-arn>, Sender=..., Subject=..., Content=...)`, `stop_engagement(EngagementId=<engagement-arn>)`.
5. Each raises ClientError with Error.Code "ResourceNotFoundException" and HTTP 404; expected Error.Code "ValidationException" with HTTP 400.
6. Re-replay the ten check ids listed in trace_ids to confirm a fix.
