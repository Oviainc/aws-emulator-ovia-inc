Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption
Issue-Key: s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers|_receiving_domain#5|AssertionError|raise AssertionError(
Issue-Key: s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule|_receiving_domain#5|AssertionError|raise AssertionError(
Issue-Key: s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact|_receiving_domain#5|AssertionError|raise AssertionError(
Issue-Key: s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption|_receiving_domain#6|AssertionError|raise AssertionError(
Issue-Commit: 07e288609021e6571eb22a2173d293968d6da2ab

# SES inbound mail receiving is unreachable: the account's DNS-baseline public hosted zone is not discoverable through route53 ListHostedZones/ListTagsForResource, so every ses receipt-rule e2e check aborts before it sends

> _Note: this issue was identified against the code base at commit `07e288609021e6571eb22a2173d293968d6da2ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListHostedZones

## Description
One root cause, grounded in four failing checks of the s3+ses+sesv2 inbound-mail surface.

Cause 1 of 1 — tag-based discovery of the account's public hosted zones yields nothing, so no receiving domain can be resolved and all four inbound-mail checks abort at their first statement, before any SES receipt rule, S3 action or send is exercised. Failing checks: s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers, s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule, s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact, s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption.

AWS: Smithy (com.amazonaws.route53): ListHostedZones — GET /2013-04-01/hostedzone, input {Marker (httpQuery marker, @length 0..64), MaxItems (httpQuery maxitems), DelegationSetId, HostedZoneType}, output ListHostedZonesResponse{HostedZones (required, list), Marker (required), IsTruncated (required, default false), NextMarker, MaxItems (required)}, paginated inputToken=Marker/outputToken=NextMarker/items=HostedZones; modeled refusals InvalidInput(400), NoSuchDelegationSet(400), DelegationSetNotReusable(400). ListTagsForResource — GET /2013-04-01/tags/{ResourceType}/{ResourceId}, input {ResourceType (httpLabel, required, enum), ResourceId (httpLabel, required, @length 0..64)}, output {ResourceTagSet (required)}; modeled refusals InvalidInput(400), NoSuchHostedZone(404), NoSuchHealthCheck(404), PriorRequestNotComplete(400), ThrottlingException(400). Smithy (com.amazonaws.sesv2): GetEmailIdentity GET /v2/email/identities/{EmailIdentity} -> {IdentityType, VerifiedForSendingStatus, DkimAttributes, ...}. Beyond those signatures this is recorded behavior only: against real AWS both route53 reads answer 200, the pagination enumerates at least one PUBLIC zone (HostedZone.Config.PrivateZone false), ListTagsForResource(ResourceType='hostedzone', ResourceId=<the id segment after the last '/'>) returns a ResourceTagSet whose Tags contain the tag that was written on that zone — here {'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'} — and GetEmailIdentity on that zone's apex (trailing dot stripped) is 200 with VerifiedForSendingStatus true. The requirement is general and not about any one zone name: a hosted zone that exists in the account is enumerated by ListHostedZones, and the tags written on it are projected by ListTagsForResource for ResourceType 'hostedzone'.

Emulator: both calls succeed (no refusal is reported for either), pagination terminates, and not one enumerated public zone reports the baseline tag. Each check therefore raises its own precondition assertion and stops: `AssertionError: this account has no DNS baseline zone (a public hosted zone tagged sim2real-infra-dns-baseline) and so no domain to receive mail for; provision it with SimParityProbe infra/pool/setup_domain_baseline.py` (three checks) and `AssertionError: no public hosted zone carries the DNS baseline tag, so the account has no verified domain to receive mail for` (the receipt-rule check). Hypothesis: either ListHostedZones enumerates no public zone for the account (empty HostedZones, or zones reported with Config.PrivateZone true), or the hostedzone tag store is not projected by ListTagsForResource so the ResourceTagSet comes back without the written tags.

Why: a client cannot tag-discover its own DNS zones — the standard way inbound-mail tooling locates the domain it receives for — and so everything downstream of a verified receiving domain is masked: MX-delivered mail matched by the active receipt rule set's recipient condition, the S3 action storing the raw message (byte-exact multipart attachments, ObjectKeyPrefix placement, KMS encryption), and the Authentication-Results/X-SES-Spam-Verdict scan headers SES adds to a received message. None of those behaviors can even be attempted, so their fidelity is unknown while this read is empty.

Fix: route53's hosted-zone enumeration and hostedzone tag reads — ListHostedZones' listing/pagination and its PrivateZone flag, and the tag state ListTagsForResource projects for ResourceType 'hostedzone' — must agree with what CreateHostedZone/ChangeTagsForResource recorded; the sesv2 GetEmailIdentity state for that apex must then report the zone's apex as verified. Note two open issues report this same route53 cause but carry only other services' check ids and so do not cover these four: 20261029T085623Z-sesv2-two-causes-the-dns-baseline-hosted-zone.md (two sesv2 checks), 20261029T065537Z-sesv2-the-whole-ses-sending-surface-is-unreachable.md (five sesv2__sns__sqs checks), and 20261029T093731Z-acm-dns-validated-certificate-issuance-is-unreachable-the.md (acm). This issue is the inbound/receiving half of the same read.

Traces: s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers (run line 103 -> _receiving_domain line 58), s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule (run line 71 -> _receiving_domain line 38), s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact (run line 107 -> _receiving_domain line 63), s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption (run line 120 -> _receiving_domain line 57).

## Evidence
Expected (AWS, recorded): route53.list_hosted_zones() pagination enumerates at least one public zone (Config.PrivateZone false); for each such zone route53.list_tags_for_resource(ResourceType='hostedzone', ResourceId=<id after the last '/'>) is 200 and its ResourceTagSet.Tags contains the tag written on the zone ({'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'} for the baseline zone); sesv2.get_email_identity(EmailIdentity=<zone apex, trailing dot stripped>) is 200 with VerifiedForSendingStatus true. The checks then proceed to create a receipt rule set + S3 action and receive mail.

Actual (emulator) — all four checks never get past the discovery loop, no call is refused:
[mismatch] s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers — run line 103 -> _receiving_domain line 58: AssertionError: this account has no DNS baseline zone (a public hosted zone tagged sim2real-infra-dns-baseline) and so no domain to receive mail for; provision it with SimParityProbe infra/pool/setup_domain_baseline.py
[mismatch] s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact — run line 107 -> _receiving_domain line 63: identical AssertionError
[mismatch] s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption — run line 120 -> _receiving_domain line 57: identical AssertionError
[mismatch] s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule — run line 71 -> _receiving_domain line 38: AssertionError: no public hosted zone carries the DNS baseline tag, so the account has no verified domain to receive mail for

The divergence is in what the two route53 reads enumerate/return, not in their status codes: both answer 2xx, so the client sees an account with no tagged public hosted zone at all.

## Steps to Reproduce
1. Build and boot the emulator, then run the behavioral suite.
2. Ensure the account holds a PUBLIC hosted zone (CreateHostedZone, then ChangeTagsForResource ResourceType='hostedzone' adding {'sim2real-infra-dns-baseline': 'true'}) whose apex is a sesv2 email identity with VerifiedForSendingStatus true.
3. Page route53.list_hosted_zones(); for each returned zone with Config.PrivateZone false call route53.list_tags_for_resource(ResourceType='hostedzone', ResourceId=<the segment of HostedZone.Id after the last '/'>).
4. Observe: no enumerated public zone reports the baseline tag (either HostedZones is empty/private-only, or ResourceTagSet.Tags omits the written tag) — both calls return 2xx, so the discovery loop simply finds nothing.
5. Re-replay s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers, s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule, s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact and s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption; each aborts in _receiving_domain before sending any mail.
