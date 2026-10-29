Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.email-identity-inherits-verified-parent-domain-dkim sesv2.list-management-options-unknown-list-rejected-contact-auto-created sesv2.email-identity-lifecycle
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.email-identity-inherits-verified-parent-domain-dkim|_dns_baseline_domain#3|AssertionError|raise AssertionError(
Issue-Key: sesv2.list-management-options-unknown-list-rejected-contact-auto-created|_dns_baseline_domain#3|AssertionError|raise AssertionError(
Issue-Key: sesv2.email-identity-lifecycle|run#26|AssertionError|assert err.get('Message') == expected_message, f"expected the message {expected_message!r}, got {err.get('Message')!r}"
Issue-Commit: 53a5dd15047be2179c1348b38f08e1b01645017e

# sesv2: two causes — the DNS-baseline hosted zone is undiscoverable so domain-scoped identity/contact checks cannot start, and PutEmailIdentityFeedbackAttributes refuses a deleted identity with the wrong error message

> _Note: this issue was identified against the code base at commit `53a5dd15047be2179c1348b38f08e1b01645017e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEmailIdentityFeedbackAttributes

## Description
Two distinct root causes in the sesv2 surface, each grounded in its own failing checks.

Cause 1 of 2 — the account's DNS-baseline public hosted zone is not discoverable through route53 ListHostedZones/ListTagsForResource, so every sesv2 check that needs a verified domain aborts at its first statement. Failing checks: sesv2.email-identity-inherits-verified-parent-domain-dkim, sesv2.list-management-options-unknown-list-rejected-contact-auto-created.
AWS: Smithy (com.amazonaws.route53): ListHostedZones -> ListHostedZonesResponse{HostedZones (required), Marker, IsTruncated (required), MaxItems (required)}, GET /2013-04-01/hostedzone, paginated by Marker/NextMarker, refusals InvalidInput(400)/NoSuchDelegationSet(400)/DelegationSetNotReusable(400); ListTagsForResource(ResourceType, ResourceId required) -> {ResourceTagSet (required)}, GET /2013-04-01/tags/{ResourceType}/{ResourceId}, refusals InvalidInput(400)/NoSuchHostedZone(404)/PriorRequestNotComplete(400)/ThrottlingException(400). Smithy (com.amazonaws.sesv2): GetEmailIdentity GET /v2/email/identities/{EmailIdentity} -> {IdentityType, VerifiedForSendingStatus, DkimAttributes, ...}. Beyond those signatures, recorded behavior only: the account holds a PUBLIC hosted zone (Config.PrivateZone false) that ListHostedZones enumerates, whose ListTagsForResource('hostedzone', <id after the last '/'>) ResourceTagSet.Tags contains {'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'}, and GetEmailIdentity on the zone apex (trailing dot stripped) is 200 with VerifiedForSendingStatus true and DkimAttributes.Status 'SUCCESS'. Both route53 reads are 200s. The requirement is general: a hosted zone created and tagged in the account is enumerated and reports its tags.
Emulator: the pagination completes and no enumerated public zone carries the baseline tag, so both checks raise `AssertionError: no public hosted zone carries the DNS baseline tag, so the account has no verified domain to send from` before any sesv2 mutation. Hypothesis: either ListHostedZones enumerates no public zone for the account, or ListTagsForResource returns a ResourceTagSet without the written tags; no refusal is reported for either call.
Why: a client cannot tag-discover its own DNS zones, and every behavior that presupposes a verified domain is unreachable — here DKIM inheritance by an email-address identity under a verified parent domain, and SendEmail's ListManagementOptions contract (NotFoundException for an unknown contact list; contact auto-creation with/without TopicName) are both masked.
Fix: route53's hosted-zone enumeration and hostedzone tag reads (ListHostedZones pagination and PrivateZone flag; the tag store ListTagsForResource projects), then the sesv2 GetEmailIdentity state for that apex must agree.
Note: 20261029T065537Z-sesv2-the-whole-ses-sending-surface-is-unreachable.md reports this same route53 cause but carries only the five sesv2__sns__sqs check ids; it does not cover these two sesv2 checks.
Traces: sesv2.email-identity-inherits-verified-parent-domain-dkim (run line 52 -> _dns_baseline_domain line 31), sesv2.list-management-options-unknown-list-rejected-contact-auto-created (run line 65 -> _dns_baseline_domain line 34).

Cause 2 of 2 — PutEmailIdentityFeedbackAttributes on an identity that does not exist is refused with the wrong error message: the emulator reuses the generic 'does not exist' text instead of SES's 'is invalid. Must be a verified email address or domain.' wording. Failing check: sesv2.email-identity-lifecycle.
AWS: Smithy (com.amazonaws.sesv2): PutEmailIdentityFeedbackAttributes PUT /v2/email/identities/{EmailIdentity}/feedback, input {EmailIdentity (httpLabel, required, @length min 1), EmailForwardingEnabled (default false)}, errors BadRequestException(400), NotFoundException(404), TooManyRequestsException(429). Recorded behavior for the missing-identity class (the sibling PutEmailIdentityMailFromAttributes is recorded identically): the call is refused with a 4xx — BadRequestException/400 in the recording, NotFoundException/404 also modeled — and the message is `Identity <identity> is invalid. Must be a verified email address or domain.` Fixed text: 'Identity ' + ' is invalid. Must be a verified email address or domain.'; the only variable part is the identity taken verbatim from the request's EmailIdentity path label (no angle brackets). Note the same service words its OTHER missing-identity messages differently and the emulator already matches those: DeleteEmailIdentity yields `Email identity <identity> does not exist.` (no brackets) and GetEmailIdentity yields `Email identity <<identity>> does not exist.` (identity wrapped in angle brackets) — both of those assertions passed in this check, so this is specifically the feedback-attributes message class.
Emulator: the refusal's code and 4xx status are accepted, but the message body is the delete/get wording: got `Email identity sim2real-behav-identity-<id>@example.com does not exist.` where AWS words it `Identity sim2real-behav-identity-<id>@example.com is invalid. Must be a verified email address or domain.` (the local part shown is a per-run generated name filling the template, not part of it).
Why: SES distinguishes 'this identity is not a usable sending identity' from 'this identity record is absent'; clients and SDK-based tooling that branch on, log, or surface the message (retry/verification flows, user-facing error reporting, test fixtures asserting the documented text) mis-classify the refusal when the mail-from/feedback mutation path returns the record-absent wording.
Fix: the PutEmailIdentityFeedbackAttributes handler's validation/error construction in the sesv2 service — the missing/unverified-identity refusal must emit the 'Identity {EmailIdentity} is invalid. Must be a verified email address or domain.' message (with the request's EmailIdentity interpolated) rather than delegating to the shared 'Email identity ... does not exist.' not-found text; its sibling PutEmailIdentityMailFromAttributes must word it the same way.
Traces: sesv2.email-identity-lifecycle (check line 79: assertion on err['Message'] after the delete->delete->get sequence).

## Evidence
Cause 1 — expected (AWS, recorded): list_hosted_zones pagination yields a public zone whose list_tags_for_resource ResourceTagSet.Tags contains {'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'}, and sesv2.get_email_identity(<apex>) is 200 with VerifiedForSendingStatus true, DkimAttributes.Status 'SUCCESS'. Actual (emulator):
[mismatch] sesv2.email-identity-inherits-verified-parent-domain-dkim — run line 52 -> _dns_baseline_domain line 31: AssertionError: no public hosted zone carries the DNS baseline tag, so the account has no verified domain to send from
[mismatch] sesv2.list-management-options-unknown-list-rejected-contact-auto-created — run line 65 -> _dns_baseline_domain line 34: identical AssertionError
Neither route53 call is refused; the divergence is in what they enumerate/return.

Cause 2 — expected (AWS): PutEmailIdentityFeedbackAttributes(EmailIdentity=<deleted identity>, EmailForwardingEnabled=True) -> 4xx refusal, BadRequestException/400 (recorded) or the modeled NotFoundException/404, with
  Message = "Identity <the EmailIdentity from the request> is invalid. Must be a verified email address or domain."
Actual (emulator): the code/status pair is accepted but the message is
  Message = "Email identity <the EmailIdentity from the request> does not exist."
Replay line: AssertionError: expected the message 'Identity sim2real-behav-identity-e2b3f56887a0@example.com is invalid. Must be a verified email address or domain.', got 'Email identity sim2real-behav-identity-e2b3f56887a0@example.com does not exist.' (the address is a per-run generated value filling the template). Earlier assertions in the same check confirm the emulator's DeleteEmailIdentity ('Email identity {id} does not exist.') and GetEmailIdentity ('Email identity <{id}> does not exist.') messages already match AWS, so only this message class diverges.

## Steps to Reproduce
Cause 1:
1. Run the behavioral suite against the built emulator.
2. Ensure the account holds a PUBLIC hosted zone tagged {'sim2real-infra-dns-baseline': 'true'} whose apex is a sesv2 identity with VerifiedForSendingStatus true and DKIM status SUCCESS.
3. Page route53.list_hosted_zones() and call route53.list_tags_for_resource(ResourceType='hostedzone', ResourceId=<id after the last '/'>) for each public zone; observe no zone reports the baseline tag.
4. Re-replay sesv2.email-identity-inherits-verified-parent-domain-dkim and sesv2.list-management-options-unknown-list-rejected-contact-auto-created.

Cause 2:
1. sesv2.create_email_identity(EmailIdentity='<local>@example.com').
2. sesv2.delete_email_identity(EmailIdentity=<same>) so the identity no longer exists.
3. sesv2.put_email_identity_feedback_attributes(EmailIdentity=<same>, EmailForwardingEnabled=True).
4. Inspect the ClientError: the code (BadRequestException or NotFoundException) and 4xx status are right, but Error.Message reads 'Email identity {id} does not exist.' instead of 'Identity {id} is invalid. Must be a verified email address or domain.'
5. Re-replay sesv2.email-identity-lifecycle to confirm.
