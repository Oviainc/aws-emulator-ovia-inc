Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sns__sqs.account-suppression-from-a-hard-bounce-blocks-the-next-send sesv2__sns__sqs.event-destination-disabled-publishes-no-events sesv2__sns__sqs.send-bulk-email-per-entry-replacement-headers-override-the-template-default sesv2__sns__sqs.send-raw-email-preserves-custom-headers-exactly-in-the-published-event sesv2__sns__sqs.templated-send-with-missing-variable-publishes-rendering-failure
Issue-Key: sesv2__sns__sqs.account-suppression-from-a-hard-bounce-blocks-the-next-send|_dns_baseline_domain#5|AssertionError|raise AssertionError(
Issue-Key: sesv2__sns__sqs.event-destination-disabled-publishes-no-events|_dns_baseline_domain#5|AssertionError|raise AssertionError(
Issue-Key: sesv2__sns__sqs.send-bulk-email-per-entry-replacement-headers-override-the-template-default|_sending_domain#5|AssertionError|raise AssertionError(
Issue-Key: sesv2__sns__sqs.send-raw-email-preserves-custom-headers-exactly-in-the-published-event|_dns_baseline_domain#5|AssertionError|raise AssertionError(
Issue-Key: sesv2__sns__sqs.templated-send-with-missing-variable-publishes-rendering-failure|_dns_baseline_domain#5|AssertionError|raise AssertionError(
Issue-Commit: 6c829b3943454ada5df5dfe2630a7a4c6694ce1c

# sesv2: the whole SES sending surface is unreachable — the account's DNS-baseline hosted zone is no longer discoverable through route53 ListHostedZones/ListTagsForResource, so no verified sending domain can be found (blocks 5 checks)

> _Note: this issue was identified against the code base at commit `6c829b3943454ada5df5dfe2630a7a4c6694ce1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListHostedZones

## Description
One root cause, grounded in five failing behavioral checks that all abort in the same first step.

Cause 1 of 1: the account's tagged public hosted zone - the DNS baseline every SES sending check resolves its verified sending domain from - cannot be found through route53's own listing/tagging reads, so every SESv2 send scenario aborts before it sends anything.

AWS: Smithy (com.amazonaws.route53): ListHostedZones -> ListHostedZonesResponse{HostedZones (required), Marker (required), IsTruncated (required, default false), NextMarker, MaxItems (required)}, GET /2013-04-01/hostedzone, paginated by Marker/NextMarker over HostedZones; refusals are InvalidInput(400), NoSuchDelegationSet(400), DelegationSetNotReusable(400). ListTagsForResource(ResourceType, ResourceId required) -> ListTagsForResourceResponse{ResourceTagSet (required)}, GET /2013-04-01/tags/{ResourceType}/{ResourceId}, refusals InvalidInput(400), NoSuchHostedZone(404), NoSuchHealthCheck(404), PriorRequestNotComplete(400), ThrottlingException(400). Beyond those signatures this is recorded behavior only: in the recorded account a PUBLIC hosted zone exists whose tag set contains {'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'}, ListHostedZones enumerates it (Config.PrivateZone false), ListTagsForResource(ResourceType='hostedzone', ResourceId=<the id after the last '/'>) returns that tag among ResourceTagSet.Tags, and sesv2.GetEmailIdentity(EmailIdentity=<the zone apex, trailing dot stripped>) reports VerifiedForSendingStatus true with DKIM status SUCCESS. Both reads are 200s - the recording contains no refusal here. The requirement is that a hosted zone created and tagged in this account is enumerated by ListHostedZones and that ListTagsForResource reports the tags that were put on it; the particular domain name and zone id are account values, not part of the contract.

Emulator: the pagination completes and no public hosted zone in it carries the baseline tag, so the shared helper raises before any SES call is made. Two readings are consistent with the replay and the recording does not distinguish them - Hypothesis: either ListHostedZones enumerates no (or not this) public hosted zone for the account, or ListTagsForResource('hostedzone', <id>) returns a ResourceTagSet without the tags that were written, so the zone is enumerated but never recognised. Either way the lookup finds nothing and five checks abort identically at their first statement with `AssertionError: no public hosted zone carries the DNS baseline tag, so the account has no verified domain to send from`. This is a regression in reach, not a new assertion: these same five checks previously got far enough to be filed against later-stage SES causes (see the related issues below), which is only possible if the baseline zone was discoverable then.

Why: a client cannot find, enumerate or tag-select its own DNS zones, and nothing downstream of a verified sending domain can be exercised at all. Concretely, five independent SES behaviors are now unverifiable behind this one read: account-level suppression after a hard bounce, an event destination with Enabled false, SendBulkEmail per-entry replacement headers, raw-MIME header preservation in the published Send event, and the asynchronous Rendering Failure event for a templated send with a missing variable. Any tooling that discovers resources by tag (`route53 list-hosted-zones` + `list-tags-for-resource`, Terraform/CDK data sources, inventory and compliance scans) also sees an account with no zones.

Fix: route53's hosted-zone listing and tag reads in the route53 service - the ListHostedZones enumeration over the account's zones (including its Marker/NextMarker pagination and the PrivateZone flag it reports) and the hostedzone tag store ListTagsForResource projects - so a zone created and tagged in the account is enumerated and reports its tags. Then the SESv2 identity read for that zone's apex (GetEmailIdentity -> VerifiedForSendingStatus) must agree.

Related open issues that do NOT cover this failure (they carry these check ids against later-stage causes that are now unreachable): 20261027T003232Z-sesv2-sns-sqs-a-hard-bounce-never-adds.md, 20261026T065823Z-sesv2-sns-sqs-ses-publishes-no-events-to.md, 20261027T161051Z-sesv2-sns-sqs-sesv2-sendemail-with-content-template-is-refused-501.md, 20261026T065823Z-sesv2-sendbulkemail-stub.md.

Traces: sesv2__sns__sqs.account-suppression-from-a-hard-bounce-blocks-the-next-send, sesv2__sns__sqs.event-destination-disabled-publishes-no-events, sesv2__sns__sqs.send-bulk-email-per-entry-replacement-headers-override-the-template-default, sesv2__sns__sqs.send-raw-email-preserves-custom-headers-exactly-in-the-published-event, sesv2__sns__sqs.templated-send-with-missing-variable-publishes-rendering-failure.

Service attribution: these check ids carry the compound token 'sesv2__sns__sqs' (their SERVICES are sesv2, sns, sqs) and the behavior under test is SESv2's; the failing reads are route53's ListHostedZones/ListTagsForResource, which the check id does not name, so the issue is filed under the check id's own service 'sesv2' and the route53 operations are named here and in the fix.

## Evidence
Expected (AWS, recorded): route53.get_paginator('list_hosted_zones').paginate() yields at least one HostedZone with Config.PrivateZone false; route53.list_tags_for_resource(ResourceType='hostedzone', ResourceId=<zone id after the last '/'>)['ResourceTagSet']['Tags'] contains {'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'}; sesv2.get_email_identity(EmailIdentity=<zone apex without the trailing dot>) reports VerifiedForSendingStatus true. Both route53 calls return HTTP 200.

Actual (emulator): the pagination completes with no public hosted zone carrying that tag, and all five checks abort at their first statement, each with the identical message:

  AssertionError: no public hosted zone carries the DNS baseline tag, so the account has no verified domain to send from

[mismatch] sesv2__sns__sqs.account-suppression-from-a-hard-bounce-blocks-the-next-send — line 104 in run -> _dns_baseline_domain line 42
[mismatch] sesv2__sns__sqs.event-destination-disabled-publishes-no-events — line 103 in run -> _dns_baseline_domain line 35
[mismatch] sesv2__sns__sqs.send-bulk-email-per-entry-replacement-headers-override-the-template-default — line 98 in run -> _sending_domain line 38
[mismatch] sesv2__sns__sqs.send-raw-email-preserves-custom-headers-exactly-in-the-published-event — line 82 in run -> _dns_baseline_domain line 35
[mismatch] sesv2__sns__sqs.templated-send-with-missing-variable-publishes-rendering-failure — line 89 in run -> _dns_baseline_domain line 37

No refusal is reported for either route53 call, so the divergence is in what they enumerate/return, not in an error class. The specific zone id and domain name are account values; the requirement is that a tagged public hosted zone in this account is enumerated and reports its tags.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite against the built emulator (region us-west-2).
2. Ensure the account holds a PUBLIC hosted zone tagged {'sim2real-infra-dns-baseline': 'true'} whose apex is a SESv2 identity with VerifiedForSendingStatus true (the DNS baseline these checks require; create it with route53 CreateHostedZone + ChangeTagsForResource and sesv2 CreateEmailIdentity if it is not already present).
3. Page route53.list_hosted_zones() and, for each zone with Config.PrivateZone false, call route53.list_tags_for_resource(ResourceType='hostedzone', ResourceId=<zone id after the last '/'>).
4. Observe that no enumerated public zone's ResourceTagSet.Tags contains {'Key': 'sim2real-infra-dns-baseline', 'Value': 'true'} — either the zone is not enumerated at all or its tags come back without that entry. Expected: the zone is enumerated and its tag set contains the tag that was written.
5. For the zone that should match, call sesv2.get_email_identity(EmailIdentity=<apex>) and check VerifiedForSendingStatus.
6. Re-replay all five check ids listed in trace_ids to confirm the fix; each then proceeds to its own SES assertion (suppression, Enabled=false, bulk replacement headers, raw-MIME header preservation, Rendering Failure), which this lookup has been masking.
