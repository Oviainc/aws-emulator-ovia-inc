Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.configuration-set-sending-disabled-blocks-v2-send-email-then-restored ses.configuration-set-tracking-options-lifecycle-with-verified-domain ses.custom-verification-template-not-found-semantics ses.send-bulk-email-simulator-per-entry-message-ids s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption
Issue-Key: ses.configuration-set-sending-disabled-blocks-v2-send-email-then-restored|_sending_domain#3|AssertionError|raise AssertionError(
Issue-Key: ses.configuration-set-tracking-options-lifecycle-with-verified-domain|_verified_apex#4|AssertionError|raise AssertionError(
Issue-Key: ses.custom-verification-template-not-found-semantics|_sending_domain#3|AssertionError|raise AssertionError(
Issue-Key: ses.send-bulk-email-simulator-per-entry-message-ids|_dns_baseline_domain#5|AssertionError|raise AssertionError(
Issue-Key: s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers|_receiving_domain#5|AssertionError|raise AssertionError(
Issue-Key: s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule|_receiving_domain#5|AssertionError|raise AssertionError(
Issue-Key: s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact|_receiving_domain#5|AssertionError|raise AssertionError(
Issue-Key: s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption|_receiving_domain#6|AssertionError|raise AssertionError(
Issue-Commit: c512cc5ae0b026572edac7488ff338a4ffd1cf1a

# ses: the SES sending AND receiving planes are unreachable — the account's DNS-baseline public hosted zone is not discoverable through route53 ListHostedZones/ListTagsForResource, so no verified domain can be resolved (blocks 7 checks)

> _Note: this issue was identified against the code base at commit `c512cc5ae0b026572edac7488ff338a4ffd1cf1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListHostedZones

## Description
One root cause, grounded in seven net-new failing behavioral checks (across two check-id service tokens) that all abort in their very first step, before any SES call is made.

Cause 1 of 1: the account's tagged PUBLIC hosted zone — the DNS baseline from which every SES check resolves its verified sending/receiving domain — cannot be found through route53's own listing and tagging reads, so every SES v1/v2 sending scenario and every SES receipt-rule (inbound) scenario aborts at its first statement.

AWS: Smithy (com.amazonaws.route53): ListHostedZones — input {Marker (httpQuery marker, length 0..64), MaxItems (httpQuery maxitems), DelegationSetId, HostedZoneType}, output ListHostedZonesResponse{HostedZones (required, list), Marker (required), IsTruncated (required, default false), NextMarker, MaxItems (required)}, GET /2013-04-01/hostedzone, code 200, @paginated inputToken=Marker outputToken=NextMarker items=HostedZones pageSize=MaxItems; modelled refusals InvalidInput(400), NoSuchDelegationSet(400), DelegationSetNotReusable(400). ListTagsForResource — input {ResourceType (httpLabel, required, enum), ResourceId (httpLabel, required, length 0..64)}, output ListTagsForResourceResponse{ResourceTagSet (required)}, GET /2013-04-01/tags/{ResourceType}/{ResourceId}, code 200; refusals InvalidInput(400), NoSuchHostedZone(404), NoSuchHealthCheck(404), PriorRequestNotComplete(400), ThrottlingException(400). ChangeTagsForResource — POST /2013-04-01/tags/{ResourceType}/{ResourceId}, AddTags length 1..10, same refusal set. Beyond those signatures this is recorded behavior only: in the recorded account a PUBLIC hosted zone exists (Config.PrivateZone false) whose tag set contains {'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'}; ListHostedZones enumerates it across its Marker/NextMarker pages, ListTagsForResource(ResourceType='hostedzone', ResourceId=<the id after the last '/'>) returns that tag in ResourceTagSet.Tags, and sesv2.GetEmailIdentity(EmailIdentity=<the zone apex with the trailing dot stripped>) reports VerifiedForSendingStatus true with DkimAttributes.Status 'SUCCESS'. All of these are 200s — the recording contains no refusal at this point. The requirement is general: a hosted zone created and tagged in this account must be enumerated by ListHostedZones and must report, through ListTagsForResource, the tags that were written on it; the particular domain name, zone id and tag values are account values that fill the template, not part of the contract.

Emulator: the pagination completes and no enumerated public hosted zone carries the baseline tag, so the shared helper in each check raises before it issues any SES/SESv2 request. No HTTP refusal is reported for either route53 read, so the divergence is in what they enumerate/return, not in an error class. Two readings are consistent with the replay and the recording does not distinguish them — Hypothesis: either ListHostedZones enumerates no (or not this) public hosted zone for the account, or ListTagsForResource('hostedzone', <id>) answers with a ResourceTagSet that omits the tags that were written, so the zone is enumerated but never recognised. The seven checks abort with one of two harness messages, differing only in the clause naming what the domain was wanted for: for the four sending checks `AssertionError: no public hosted zone carries the DNS baseline tag, so the account has no verified domain to send from` (raised from _sending_domain / _verified_apex / _dns_baseline_domain), and for the four inbound checks the receiving variant `AssertionError: this account has no DNS baseline zone (a public hosted zone tagged sim2real-infra-dns-baseline) and so no domain to receive mail for` / `... no verified domain to receive mail for` (raised from _receiving_domain). Both forms are the same failed lookup.

Why: a client cannot discover, enumerate or tag-select its own DNS zones, and therefore nothing that depends on a verified SES domain can be exercised at all. Concretely, seven independent SES behaviors are unverifiable behind this one read: configuration-set sending-disabled blocking a v2 SendEmail and then being restored, the configuration-set tracking-options lifecycle against a verified domain, the CustomVerificationEmailTemplate not-found semantics for a verified sender, SendBulkEmail per-entry message ids against the mailbox simulator, and the four inbound receipt-rule scenarios (S3 delivery of mail to a verified domain, authentication/scan headers on the inbound message, byte-exact raw multipart attachment delivery, and S3 action object-key prefix plus KMS encryption). Any tag-driven discovery tooling (`aws route53 list-hosted-zones` + `list-tags-for-resource`, Terraform/CDK data sources, inventory and compliance scans) likewise sees an account with no zones.

Fix: route53's hosted-zone listing and hostedzone tag reads in the route53 service — the ListHostedZones enumeration over the account's zones (its Marker/NextMarker pagination and the Config.PrivateZone flag it reports) and the hostedzone tag store that ListTagsForResource projects after ChangeTagsForResource has written to it — so that a zone created and tagged in this account is enumerated and reports its tags. Then the SESv2 identity read for that zone's apex (GetEmailIdentity -> VerifiedForSendingStatus / DkimAttributes.Status) must agree for the SES checks to proceed.

Related open issue that does NOT cover these failures: 20261029T065537Z-sesv2-the-whole-ses-sending-surface-is-unreachable.md carries the same root cause but is keyed only to the five sesv2__sns__sqs check ids; the seven check ids listed here are not on it, and it does not mention the receiving (receipt-rule) side of the lookup at all.

Service attribution: four check ids carry the token 'ses' and four carry the compound token 's3__ses__sesv2' (their SERVICES are s3, ses, sesv2); in all seven the behavior under test is SES's, so the issue is filed under 'ses' and the failing route53 operations are named here and in the fix.

Traces: the eight check ids in trace_ids (seven distinct net-new scenarios plus their shared helper), listed per-check in Evidence.

## Evidence
Expected (AWS, recorded): route53.get_paginator('list_hosted_zones').paginate() yields at least one HostedZone with Config.PrivateZone false; route53.list_tags_for_resource(ResourceType='hostedzone', ResourceId=<zone id after the last '/'>)['ResourceTagSet']['Tags'] contains {'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'}; sesv2.get_email_identity(EmailIdentity=<zone apex without the trailing dot>) reports VerifiedForSendingStatus true and DkimAttributes.Status 'SUCCESS'. Every one of these calls returns HTTP 200.

Actual (emulator): the pagination completes with no public hosted zone carrying that tag, and all seven checks abort at their first statement. Fixed text of each message, with the variable parts (the tag key/value and the zone's domain) coming from the request/state, never from one run:

Sending variant — `AssertionError: no public hosted zone carries the DNS baseline tag, so the account has no verified domain to send from`
  [mismatch] ses.configuration-set-sending-disabled-blocks-v2-send-email-then-restored — line 108 in run -> _sending_domain line 35
  [mismatch] ses.configuration-set-tracking-options-lifecycle-with-verified-domain — line 68 in run -> _verified_apex line 34
  [mismatch] ses.custom-verification-template-not-found-semantics — line 78 _verified_sender_email -> _sending_domain line 100
  [mismatch] ses.send-bulk-email-simulator-per-entry-message-ids — line 134 in run -> _dns_baseline_domain line 40

Receiving variant — `AssertionError: this account has no DNS baseline zone (a public hosted zone tagged sim2real-infra-dns-baseline) and so no domain to receive mail for` (and, in one check, `AssertionError: no public hosted zone carries the DNS baseline tag, so the account has no verified domain to receive mail for`)
  [mismatch] s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers — line 103 in run -> _receiving_domain line 58
  [mismatch] s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule — line 71 in run -> _receiving_domain line 38
  [mismatch] s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact — line 107 in run -> _receiving_domain line 63
  [mismatch] s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption — line 120 in run -> _receiving_domain line 57

No non-2xx response and no error code is reported for either route53 read, so this is not a refusal-class divergence: the reads succeed and return content that omits the account's tagged zone (or its tags). The specific zone id and domain name are account values; the requirement is that a tagged public hosted zone in this account is enumerated and reports the tags written on it.

## Steps to Reproduce
1. Clone the repo and build/boot the emulator, then run the behavioral suite (region us-west-2).
2. Ensure the account holds a PUBLIC hosted zone tagged {'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'} whose apex is an SES identity verified for sending (route53 CreateHostedZone + ChangeTagsForResource, sesv2 CreateEmailIdentity and the Easy DKIM CNAMEs, plus an MX record for the receiving scenarios).
3. Page route53.list_hosted_zones() and, for each zone with Config.PrivateZone false, call route53.list_tags_for_resource(ResourceType='hostedzone', ResourceId=<zone id after the last '/'>).
4. Observe that no enumerated public zone's ResourceTagSet.Tags contains the baseline tag — either the zone is not enumerated at all, or its tag set comes back without the entry that ChangeTagsForResource wrote. Expected: the zone is enumerated and reports its tags, both with HTTP 200.
5. For the zone that should match, call sesv2.get_email_identity(EmailIdentity=<apex>) and confirm VerifiedForSendingStatus true / DkimAttributes.Status 'SUCCESS'.
6. Re-replay all check ids in trace_ids to confirm the fix; each then proceeds to its own SES assertion (configuration-set sending enablement, tracking-options lifecycle, custom-verification-template not-found semantics, SendBulkEmail per-entry message ids, and the four inbound receipt-rule scenarios), which this lookup is currently masking.
