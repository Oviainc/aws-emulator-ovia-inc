Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-catalog-list-cloud-watch-alarm-template-groups
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-catalog-list-cloud-watch-alarm-template-groups medialive-cloudwatch-alarm-template-group-lifecycle medialive-catalog-list-cloud-watch-alarm-templates medialive-catalog-list-offerings medialive-list-channel-engine-versions medialive-account-configuration-roundtrip
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[*]
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-CloudFrontGeneralWorkflows"]
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectArqProtocolWorkflows"]
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectCdiJpegxsWorkflows"]
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectInputFailoverWorkflows"]
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectRtpWorkflows"]
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveMultiplexWorkflows"]
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveToMediaPackageWorkflows"]
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveUdpAndRtmpWorkflows"]
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaPackageGeneralWorkflows"]
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaTailorAdInsertionWorkflows"]
Issue-Key: medialive|ListCloudWatchAlarmTemplates|2xx->2xx|$.cloudWatchAlarmTemplates[*]
Issue-Key: medialive|ListOfferings|2xx->2xx|$.offerings[arn="arn
Issue-Key: medialive|ListVersions|2xx->2xx|$.versions[*]
Issue-Key: medialive|DescribeAccountConfiguration|2xx->2xx|$.accountConfiguration.kmsKeyId
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# medialive read paths serve emulator-local state: empty AWS-managed catalogs, invented engine versions, dropped account kmsKeyId

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCloudWatchAlarmTemplateGroups

## Description
This issue bundles 6 causes. They share one fault: MediaLive's read paths for data the SERVICE owns (the AWS-managed CloudWatch alarm template catalog, the regional reserved-offering catalog, the channel engine-version catalog) and for the account-level configuration singleton are answered out of the emulator's own per-run resource store, which contains only what the current run created. The result is a 200 with a structurally valid but substantively wrong body: an empty list where AWS publishes a catalog, a synthesised list where AWS publishes none, and a dropped member on a setting the account persisted. Every status matches (200 -> 200); it is the payload that diverges. Each cause below must be closed before submitting.

Cause 1 of 6: ListCloudWatchAlarmTemplateGroups publishes none of the AWS-managed template groups.
AWS: Smithy: ListCloudWatchAlarmTemplateGroups -> GET /prod/cloudwatch-alarm-template-groups, output member CloudWatchAlarmTemplateGroups (jsonName 'cloudWatchAlarmTemplateGroups', list of CloudWatchAlarmTemplateGroupSummary), paginated on NextToken/MaxResults, with an optional `scope` query filter. Recorded behavior: in a brand-new account the list is NOT empty — MediaLive publishes a set of service-owned ('AWS-' prefixed) template groups that every account sees, each a summary carrying arn, id, name, templateCount, createdAt and tags. Their ids are service-assigned 'aws-<digits>' values and their arns have an EMPTY account-id field (arn:aws:medialive:<region>::cloudwatch-alarm-template-group:<id>), which is how an AWS-managed entry is distinguished from a customer-created one. The particular ids, names, counts and createdAt values in the recording are the catalog's contents, not a per-run value.
Emulator: answers 200 with the list containing nothing from the managed catalog — every expected entry reports `missing ... got None`. The account's own groups, had any been created, would be all it serves.
Why: the `scope` filter and the whole 'start from an AWS-managed group, then clone or attach it' workflow have nothing to operate on. A client that lists groups to resolve a managed group's id before creating an alarm template in it, or that renders a picker of available groups, gets an empty result and concludes the account has no monitoring templates — a state a real account cannot be in.
Fix: the medialive crate's monitor/CloudWatch-alarm-template state must seed the AWS-managed template-group catalog (service-owned, account-independent, read-only) and ListCloudWatchAlarmTemplateGroups must merge it with the account's own groups.
Traces: medialive-catalog-list-cloud-watch-alarm-template-groups#task#1.

Cause 2 of 6: the same absence observed mid-lifecycle, after the account has created its own group.
AWS: recorded behavior: a list taken while the account holds its own freshly-created group STILL contains the full AWS-managed set alongside it (the recording asserts the managed entries by name and does not assert their createdAt).
Emulator: 200, and the managed entries are still absent — creating a customer group does not change the outcome, confirming the catalog is never seeded rather than being filtered away by a query parameter.
Why: a lifecycle client that lists to confirm its own group exists also loses every managed group from the same response, so pagination counts and 'does this name collide with a managed group' checks are computed over the wrong population.
Fix: same as Cause 1 — the merge must hold on every list call, not only on an empty account.
Traces: medialive-cloudwatch-alarm-template-group-lifecycle#task#3.

Cause 3 of 6: ListCloudWatchAlarmTemplates publishes none of the AWS-managed alarm templates.
AWS: Smithy: ListCloudWatchAlarmTemplates -> GET /prod/cloudwatch-alarm-templates, output member CloudWatchAlarmTemplates (jsonName 'cloudWatchAlarmTemplates'). Recorded behavior: a new account sees a large set of service-owned templates, each carrying arn (again with the empty account-id field), id ('aws-<digits>'), groupId linking it to one of the managed groups from Cause 1, name, targetResourceType, comparisonOperator, evaluationPeriods, metricName, period, statistic, treatMissingData, threshold, datapointsToAlarm, description and tags.
Emulator: answers 200 with every expected template reported `missing ... got None`.
Why: the managed templates are the documented starting point for MediaLive monitoring; with none of them served, GetCloudWatchAlarmTemplate on a managed id cannot succeed, the groupId cross-reference from Cause 1 dangles, and a client that copies a managed template to build its own has no source.
Fix: same component as Cause 1 — seed the managed alarm-template catalog and serve it from the list (and single-get) path, with each template's groupId consistent with the seeded groups.
Traces: medialive-catalog-list-cloud-watch-alarm-templates#task#1.

Cause 4 of 6: ListOfferings publishes no reserved-capacity offerings.
AWS: Smithy: ListOfferings -> GET /prod/offerings, output member Offerings (jsonName 'offerings', list of Offering), paginated, with filter query parameters channelClass, codec, resolution, maximumBitrate, resourceType, duration, specialFeature, videoQuality, maximumFramerate, channelConfiguration. Recorded behavior: the offering catalog is published by the service per region and is identical for every account — each Offering carries arn (empty account-id field), offeringId, offeringDescription, resourceSpecification (resourceType/codec/resolution/maximumBitrate/channelClass), region, offeringType, duration, durationUnits, usagePrice, fixedPrice and currencyCode. The specific offeringIds and prices are catalog data for that region, not per-run values.
Emulator: answers 200 with an offerings list from which every recorded offering is `missing ... got None`.
Why: this is the only way to discover a purchasable offeringId, so PurchaseOffering can never be driven the way a real client drives it (list, filter, pick an offeringId, purchase) — the id has to be invented, and the filter query parameters are untestable because there is nothing to filter.
Fix: the medialive crate's reservation/offering state must carry the regional offering catalog as service-owned read-only data, and ListOfferings must serve and filter it.
Traces: medialive-catalog-list-offerings#task#1.

Cause 5 of 6: ListVersions invents a channel-engine-version catalog where the recording has none.
AWS: Smithy: ListVersions -> GET /prod/versions, no input, single output member Versions (jsonName 'versions', list of ChannelEngineVersionResponse). Recorded behavior only: the recorded response carries no entries in `versions`.
Emulator: answers 200 with four synthesised entries ('1.3.0', '1.2.0', '1.1.0', '1.0.0'), each with an expirationDate that is clearly computed from the emulator's own clock at first touch rather than being a published service value.
Why: the engine-version catalog is service-published, and a client that reads ListVersions to decide which version to pin on CreateChannel/UpdateChannel is handed versions and expiry dates the service never advertised. Because the expirationDate is derived from the run's clock, the same version reports a different expiry on different runs, so nothing a caller caches about it is stable. Hypothesis: the catalog is seeded lazily at first touch with now()-relative expiry dates, which is also why the values are not reproducible.
Why this is not a clock artefact: the divergence is the PRESENCE of four entries where the recording has none, not a difference in a timestamp's value.
Fix: the medialive crate's channel-engine-version state/ListVersions handler — this catalog must not be synthesised from the local clock; it must reflect the service-published set (empty where the service publishes nothing for the account).
Traces: medialive-list-channel-engine-versions#task#1.

Cause 6 of 6: DescribeAccountConfiguration drops the account's persisted kmsKeyId.
AWS: Smithy: DescribeAccountConfiguration -> GET /prod/accountConfiguration, output member AccountConfiguration (jsonName 'accountConfiguration') targeting the AccountConfiguration structure, whose member KmsKeyId (jsonName 'kmsKeyId') is documented 'Specifies the KMS key to use for all features that use key encryption. Specify the ARN of a KMS key that you have created. Or leave blank to use the key that MediaLive creates and manages for you.' UpdateAccountConfiguration (PUT /prod/accountConfiguration) takes and returns the same structure. Recorded behavior: a describe reports the kmsKeyId the account currently has configured, and the roundtrip asserts it both at the start and again after the configuration is restored.
Emulator: answers 200 with an accountConfiguration object in which kmsKeyId is absent entirely, on both the baseline read and the post-restore read of the same fixture.
Why: the account-level KMS key is not readable, so a client cannot discover which key MediaLive encrypts with, cannot verify that its own UpdateAccountConfiguration took effect, and cannot restore the prior value because the prior value cannot be read. An absent member is indistinguishable from 'blank, use the MediaLive-managed key', which is a different account state.
Fix: the medialive crate's account-configuration state and the DescribeAccountConfiguration handler/serializer — the configured kmsKeyId must be persisted by UpdateAccountConfiguration and emitted on every describe.
Traces: medialive-account-configuration-roundtrip#task#2 (also observed at task#6 of the same fixture, the post-restore read).

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, score 83.2%), 6 mismatch clusters, all with expected_status == actual_status == 200:

1. medialive-catalog-list-cloud-watch-alarm-template-groups#task#1 ListCloudWatchAlarmTemplateGroups: $.cloudWatchAlarmTemplateGroups[0..9+] all 'missing (expected {...}, got None)'. Expected entries are AWS-managed summaries such as id 'aws-1160891' / name 'AWS-CloudFrontGeneralWorkflows' / templateCount 2, arn 'arn:aws:medialive:us-west-2::cloudwatch-alarm-template-group:aws-1160891' (note the empty account-id field).
2. medialive-cloudwatch-alarm-template-group-lifecycle#task#3 ListCloudWatchAlarmTemplateGroups: the same managed groups missing, asserted by name (createdAt marked <shaped-not-asserted>), on a list taken after the fixture created its own group.
3. medialive-catalog-list-cloud-watch-alarm-templates#task#1 ListCloudWatchAlarmTemplates: $.cloudWatchAlarmTemplates[0..55] all missing; expected entries carry groupId values pointing at the managed groups from (1), e.g. id 'aws-0253256' groupId 'aws-1484347' metricName 'ChannelInputErrorSeconds'.
4. medialive-catalog-list-offerings#task#1 ListOfferings: $.offerings[arn="...::offering:304736645"] and siblings all missing; expected entries carry offeringId, resourceSpecification, offeringType, duration/durationUnits, usagePrice/fixedPrice/currencyCode.
5. medialive-list-channel-engine-versions#task#1 ListVersions: $.versions[0..3] all 'extra (expected None, got ...)' — emulator emitted {'version': '1.3.0', 'expirationDate': '2027-10-29T20:14:35.243885Z'}, {'1.2.0', '2027-07-31T...'}, {'1.1.0', '2027-05-02T...'}, {'1.0.0', '2027-02-01T...'} where the recording has no entries.
6. medialive-account-configuration-roundtrip#task#2 DescribeAccountConfiguration: $.accountConfiguration.kmsKeyId 'missing (expected <the account's configured key>, got None)'; affected_count 2, the other member being task#6 of the same fixture.

The concrete ids, names, prices, versions and timestamps above are the catalogs' / the account's data as recorded; the requirement is that the member or entry is served at all and that its value comes from service-published or account-persisted state rather than the local clock.

## Steps to Reproduce
1. Boot the emulator with an account that has created nothing, then GET /prod/cloudwatch-alarm-template-groups (ListCloudWatchAlarmTemplateGroups): the response must already list the AWS-managed 'AWS-*' groups whose arns have an empty account-id field. Replay medialive-catalog-list-cloud-watch-alarm-template-groups#task#1.
2. Create a customer template group, then list again (replay medialive-cloudwatch-alarm-template-group-lifecycle#task#3): the managed groups must still be present alongside it.
3. GET /prod/cloudwatch-alarm-templates (replay medialive-catalog-list-cloud-watch-alarm-templates#task#1): the managed templates must be listed, each with a groupId matching a group from step 1.
4. GET /prod/offerings (replay medialive-catalog-list-offerings#task#1): the regional reserved-offering catalog must be listed, so an offeringId can be picked for PurchaseOffering.
5. GET /prod/versions (replay medialive-list-channel-engine-versions#task#1): the response must not contain locally synthesised versions/expirationDates; compare against the recorded empty versions list, and note the expirationDates shift with the emulator's clock across runs.
6. PUT /prod/accountConfiguration with a kmsKeyId, then GET /prod/accountConfiguration (replay medialive-account-configuration-roundtrip#task#2 and #task#6): accountConfiguration.kmsKeyId must be present and equal to the configured key on both reads.
