Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-catalog-list-cloud-watch-alarm-template-groups
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-catalog-list-cloud-watch-alarm-template-groups medialive-catalog-list-cloud-watch-alarm-templates medialive-catalog-list-offerings medialive-cloudwatch-alarm-template-group-lifecycle
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[arn="arn
Issue-Key: medialive|ListCloudWatchAlarmTemplates|2xx->2xx|$.cloudWatchAlarmTemplates[arn="arn
Issue-Key: medialive|ListOfferings|2xx->2xx|$.offerings[arn="arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-CloudFrontGeneralWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-CloudFrontGeneralWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectArqProtocolWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectArqProtocolWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectArqProtocolWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectCdiJpegxsWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectCdiJpegxsWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectCdiJpegxsWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectInputFailoverWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectInputFailoverWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectInputFailoverWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectRtpWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectRtpWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectRtpWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveMultiplexWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveMultiplexWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveMultiplexWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveToMediaPackageWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveToMediaPackageWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveToMediaPackageWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveUdpAndRtmpWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveUdpAndRtmpWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveUdpAndRtmpWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaPackageGeneralWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaPackageGeneralWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaTailorAdInsertionWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaTailorAdInsertionWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaTailorAdInsertionWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="sim2real-probe-medialive-cwgroup-7f786eeb"]
Issue-Commit: 1e71a3102b418a18abda1ed5ba2c04b37249c268

# medialive catalog listings diverge: AWS-provided alarm template groups and templates and offerings are emulator-minted, and a created group is absent

> _Note: this issue was identified against the code base at commit `1e71a3102b418a18abda1ed5ba2c04b37249c268`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCloudWatchAlarmTemplateGroups

## Description
Four distinct causes, all in the medialive crate's handling of the SERVICE-PUBLISHED read-only catalogs (the AWS-provided CloudWatch alarm template groups, the AWS-provided alarm templates, and the reserved-resource offerings) and in the group listing itself. A fixer must close all four before submitting.

Cause 1 of 4 — ListCloudWatchAlarmTemplateGroups does not publish AWS's provided template groups; it publishes groups minted from the crate's own id counter.
AWS: ListCloudWatchAlarmTemplateGroups (Smithy: GET /prod/cloudwatch-alarm-template-groups, output CloudWatchAlarmTemplateGroups: __listOfCloudWatchAlarmTemplateGroupSummary, paginated on NextToken/MaxResults, Scope query member documented as "Represents the scope of a resource, with options for all scopes, AWS provided resources, or local resources") returns, in addition to the caller's own groups, the AWS-PROVIDED groups. Smithy documents CloudWatchAlarmTemplateGroupSummary$Id as "A cloudwatch alarm template group's id. AWS provided template groups have ids that start with `aws-`" (target __stringMin7Max11PatternAws097), and each summary carries the required Arn/Id/Name/CreatedAt/TemplateCount. Those AWS-provided entries are a FIXED published table: the same ids, names, template counts and createdAt instants are reported to every account in a region — they are not per-account or per-run values. The recording shows that table, e.g. id `aws-1160891` / name `AWS-CloudFrontGeneralWorkflows` / templateCount 2, id `aws-1484347` / `AWS-MediaLiveUdpAndRtmpWorkflows` / templateCount 9, id `aws-2520023` / `AWS-MediaPackageGeneralWorkflows` / templateCount 3, id `aws-3414886` / `AWS-MediaConnectRtpWorkflows` / templateCount 4, each with arn `arn:aws:medialive:<region>::cloudwatch-alarm-template-group:<id>` (note the EMPTY account field in an AWS-provided group's ARN) and tags {}.
Emulator: 200 with a cloudWatchAlarmTemplateGroups list that contains NO entry at any of the published ARNs — every expected `arn:aws:medialive:us-west-2::cloudwatch-alarm-template-group:aws-NNNNNNN` lookup resolves to None. The sibling lifecycle fixture shows why: the emulator synthesizes the AWS-provided groups at store seed time out of the crate's sequential id mint, so `AWS-CloudFrontGeneralWorkflows` is served as id `aws-1000001` instead of `aws-1160891` and `AWS-MediaLiveUdpAndRtmpWorkflows` as `aws-1000008` instead of `aws-1484347`. The minted ids satisfy the `aws-` pattern but are not the published catalog's ids, and the accompanying templateCount is also synthesized (4 reported for `AWS-MediaLiveUdpAndRtmpWorkflows`, which publishes 9).
Why: the AWS-provided groups are the handle a client uses to reach the managed templates — it lists the groups, picks one by id, and passes that id as GroupIdentifier to ListCloudWatchAlarmTemplates or as the group of a signal map's monitor deployment. A client that hardcodes or caches a real AWS group id (`aws-1160891`) gets a 404 from the emulator, and a client that reads the emulator's id back cannot use it against AWS. templateCount being synthesized also makes any "this group has N templates" assertion wrong.
Fix: the crate's seeded alarm-template-group catalog state (the AWS-provided groups must be a fixed published table carrying AWS's own ids/names/templateCounts/createdAt, allocated OUTSIDE the resource id mint that serves caller-created groups) plus the ListCloudWatchAlarmTemplateGroups projection that reports it.
Traces: medialive-catalog-list-cloud-watch-alarm-template-groups#task#1; corroborated by medialive-cloudwatch-alarm-template-group-lifecycle#task#3.

Cause 2 of 4 — ListCloudWatchAlarmTemplates does not publish AWS's provided alarm templates.
AWS: ListCloudWatchAlarmTemplates (Smithy: GET /prod/cloudwatch-alarm-templates, output CloudWatchAlarmTemplates, paginated, with GroupIdentifier/Scope/SignalMapIdentifier query filters) returns the AWS-provided templates alongside the caller's. Each AWS-provided template is a fixed published row: id starting with `aws-`, arn `arn:aws:medialive:<region>::cloudwatch-alarm-template:<id>` with an empty account field, the groupId of the AWS-provided group it belongs to, and the full alarm definition (targetResourceType, comparisonOperator, evaluationPeriods, metricName, period, statistic, treatMissingData, threshold, datapointsToAlarm, description, createdAt, tags). The recording shows that table, e.g. id `aws-0253256` in groupId `aws-1484347`, name `AWS-ChannelInputErrorSecondsHigh-MediaLiveUdpAndRtmpWorkflows`, targetResourceType MEDIALIVE_CHANNEL, metricName ChannelInputErrorSeconds, comparisonOperator GreaterThanThreshold, period 60, statistic Maximum, treatMissingData `missing`, threshold 0, evaluationPeriods 1, datapointsToAlarm 1; and id `aws-0563153` in groupId `aws-5775786`, metricName InputLossSeconds.
Emulator: 200 with a cloudWatchAlarmTemplates list holding no entry at any published template ARN — every `arn:aws:medialive:us-west-2::cloudwatch-alarm-template:aws-NNNNNNN` lookup is None. Same root shape as cause 1: the managed templates are either absent or minted with emulator-authored ids, and their groupId cross-reference therefore cannot point at AWS's group ids either.
Why: a client that deploys AWS's recommended alarms (the normal use of this API — list the managed templates for a group, then deploy that group onto a signal map) finds nothing to deploy, and GroupIdentifier filtering by a real AWS group id returns an empty page instead of that group's templates. Any assertion on an alarm's metric/threshold definition is unverifiable.
Fix: the same seeded catalog state, extended to the AWS-provided template table (ids and groupIds fixed to AWS's published values, full alarm definition fields stored, not defaulted), plus the ListCloudWatchAlarmTemplates projection and its GroupIdentifier/Scope filters.
Traces: medialive-catalog-list-cloud-watch-alarm-templates#task#1.

Cause 3 of 4 — ListOfferings does not publish the region's reserved-resource offering catalog.
AWS: ListOfferings (Smithy: GET /prod/offerings, output Offerings: __listOfOffering, paginated, filterable by ResourceType/Codec/Resolution/MaximumBitrate/ChannelClass/Duration/...) returns the offerings purchasable in the signed region. Smithy documents Offering$Arn as "Unique offering ARN", Offering$OfferingId as "Unique offering ID", plus offeringDescription, resourceSpecification (ReservationResourceSpecification), region, offeringType, duration, durationUnits, usagePrice, fixedPrice and currencyCode. The catalog is service-published and region-stable, not per-account: the recording shows offeringId `304736645` (arn `arn:aws:medialive:us-west-2::offering:304736645`, "Reserved Single Pipeline UHD AVC input at <10mbps in US West (Oregon)", resourceSpecification {resourceType INPUT, codec AVC, resolution UHD, maximumBitrate MAX_10_MBPS, channelClass SINGLE_PIPELINE}, NO_UPFRONT, duration 12 MONTHS, usagePrice 0.2071, fixedPrice 0.0, USD), `304741391` (AVC FHD multiplex output, MULTIPLEX/STANDARD, 12 MONTHS, usagePrice 2.025), `304735075` (AVC HD multiplex output, 36 MONTHS, usagePrice 1.1475), `304736652` (HD Link input).
Emulator: 200 with an offerings list containing no entry at any of those ARNs — each `arn:aws:medialive:us-west-2::offering:NNNNNNNNN` lookup is None, so the published offering ids, their resourceSpecification combinations and their prices are not served.
Why: PurchaseOffering takes an OfferingId that the caller got from ListOfferings. A client that selects an offering by a real id, or that filters the catalog by resourceType/codec/resolution to find one, has nothing to purchase against the emulator; reservation pricing and duration are also unverifiable.
Fix: the crate's offering catalog state — a fixed published table of offerings per region keyed by AWS's offeringIds (with arn, resourceSpecification, offeringType, duration/durationUnits, usagePrice/fixedPrice, currencyCode stored rather than computed per request) — plus the ListOfferings handler's filtering and projection, and PurchaseOffering's lookup into it.
Traces: medialive-catalog-list-offerings#task#1.

Cause 4 of 4 — a group the caller just created is absent from ListCloudWatchAlarmTemplateGroups.
AWS: CreateCloudWatchAlarmTemplateGroup (Smithy: POST /prod/cloudwatch-alarm-template-groups, 201, returns Arn/Id/Name/Description/CreatedAt/ModifiedAt/Tags) creates a local (caller-scoped) group; the group is immediately visible to ListCloudWatchAlarmTemplateGroups, which enumerates the account's local groups together with the AWS-provided ones. CloudWatchAlarmTemplateGroupSummary$Name is documented "Names must be unique within the scope of a resource type in a specific region", and a freshly created group reports templateCount 0 and the description it was created with.
Emulator: after a successful create, the list at the next step contains NO entry whose name is the created group's (the fixture's probe name, created with description "initial description"), while it does return the synthesized AWS-provided groups. So a resource that exists (create answered 201 and the fixture's later get/update/delete steps address it) is not enumerated. The step asserts only the SHAPE of that entry's arn/id/createdAt/modifiedAt, so this is not an id-format difference: the entry is missing outright.
Why: list-then-act is the standard discovery path — a client that creates a group and then lists to find it, or that reconciles desired state against the listing, concludes the group does not exist and creates it again (hitting a 409 name conflict) or treats its own resource as lost. It also breaks Scope filtering semantics, since the local scope is exactly what is unreported.
Fix: the ListCloudWatchAlarmTemplateGroups handler's enumeration over state — it must iterate the account's caller-created groups in the signed region as well as the published AWS-provided table, under one stable order and one cursor, rather than returning only the seeded catalog.
Traces: medialive-cloudwatch-alarm-template-group-lifecycle#task#3.

Related but NOT covering these: the open issue 20261029T182501Z-medialive-listversions-wrong-behavior.md reports that another service-published catalog (the channel engine versions) is seeded into the per-region partition at first touch; it is about that catalog's placement/timestamps only and names none of the replay keys above. The alarm-template 404 wording issues (20261029T201850Z-medialive-not-found-refusals-carry-emulator-authored-resource.md, 20261029T210532Z-medialive-refusal-messages-diverge-cloudwatch-alarm-template-404.md) concern refusal messages, not catalog contents.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 97/113 steps passed, 16 diverged). The four net-new clusters here, verbatim:

Cause 1 — fixture medialive-catalog-list-cloud-watch-alarm-template-groups, step task#1, ListCloudWatchAlarmTemplateGroups, expected 200 / actual 200:
  $.cloudWatchAlarmTemplateGroups[arn="arn:aws:medialive:us-west-2::cloudwatch-alarm-template-group:aws-1160891"]: missing (expected {'arn': 'arn:aws:medialive:us-west-2::cloudwatch-alarm-template-group:aws-1160891', 'id': 'aws-1160891', 'name': 'AWS-CloudFrontGeneralWorkflows', 'templateCount': 2, 'createdAt': '2024-04-09T16:15:25.342Z', 'tags': {}}, got None)
  ... same for aws-1484347 (AWS-MediaLiveUdpAndRtmpWorkflows, templateCount 9), aws-2520023 (AWS-MediaPackageGeneralWorkflows, 3), aws-2587288 (AWS-MediaConnectCdiJpegxsWorkflows, 3), aws-3414886 (AWS-MediaConnectRtpWorkflows, 4), aws-3840949, ...

Cause 2 — fixture medialive-catalog-list-cloud-watch-alarm-templates, step task#1, ListCloudWatchAlarmTemplates, expected 200 / actual 200:
  $.cloudWatchAlarmTemplates[arn="arn:aws:medialive:us-west-2::cloudwatch-alarm-template:aws-0253256"]: missing (expected {'id': 'aws-0253256', 'groupId': 'aws-1484347', 'name': 'AWS-ChannelInputErrorSecondsHigh-MediaLiveUdpAndRtmpWorkflows', 'targetResourceType': 'MEDIALIVE_CHANNEL', 'comparisonOperator': 'GreaterThanThreshold', 'evaluationPeriods': 1, 'metricName': 'ChannelInputErrorSeconds', 'period': 60, 'statistic': 'Maximum', 'treatMissingData': 'missing', 'threshold': 0, 'datapointsToAlarm': 1, ...}, got None)
  $.cloudWatchAlarmTemplates[arn="...:cloudwatch-alarm-template:aws-0563153"]: missing (expected groupId 'aws-5775786', name 'AWS-InputLossSecondsHigh-MediaLiveToMediaPackageWorkflows', metricName 'InputLossSeconds', ...), got None)

Cause 3 — fixture medialive-catalog-list-offerings, step task#1, ListOfferings, expected 200 / actual 200:
  $.offerings[arn="arn:aws:medialive:us-west-2::offering:304736645"]: missing (expected {'offeringId': '304736645', 'offeringDescription': 'Reserved Single Pipeline UHD AVC input at <10mbps in US West (Oregon)', 'resourceSpecification': {'resourceType': 'INPUT', 'codec': 'AVC', 'resolution': 'UHD', 'maximumBitrate': 'MAX_10_MBPS', 'channelClass': 'SINGLE_PIPELINE'}, 'region': 'us-west-2', 'offeringType': 'NO_UPFRONT', 'duration': 12, 'durationUnits': 'MONTHS', 'usagePrice': 0.2071, 'fixedPrice': 0.0, 'currencyCode': 'USD'}, got None)
  ... same for offerings 304741391 (usagePrice 2.025), 304735075 (36 MONTHS, 1.1475), 304736652 (HD Link input)

Cause 4 — fixture medialive-cloudwatch-alarm-template-group-lifecycle, step task#3, ListCloudWatchAlarmTemplateGroups, expected 200 / actual 200:
  $.cloudWatchAlarmTemplateGroups[name="sim2real-probe-medialive-cwgroup-7f786eeb"]: missing (expected {'arn': '<shaped-not-asserted>', 'id': '<shaped-not-asserted>', 'name': 'sim2real-probe-medialive-cwgroup-7f786eeb', 'templateCount': 0, 'createdAt': '<shaped-not-asserted>', 'modifiedAt': '<shaped-not-asserted>', 'description': 'initial description', 'tags': {}}, got None)
  $.cloudWatchAlarmTemplateGroups[name="AWS-CloudFrontGeneralWorkflows"].id: value_mismatch (expected 'aws-1160891', got 'aws-1000001')
  $.cloudWatchAlarmTemplateGroups[name="AWS-CloudFrontGeneralWorkflows"].arn: value_mismatch (expected '...:cloudwatch-alarm-template-group:aws-1160891', got '...:cloudwatch-alarm-template-group:aws-1000001')
  $.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveUdpAndRtmpWorkflows"].id: value_mismatch (expected 'aws-1484347', got 'aws-1000008')
  $.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveUdpAndRtmpWorkflows"].templateCount: value_mismatch (expected 9, got 4)
  ... the same .arn/.id/.templateCount mismatch for every other AWS-provided group (AWS-MediaPackageGeneralWorkflows -> aws-1000009, AWS-MediaConnectCdiJpegxsWorkflows -> aws-1000003, AWS-MediaConnectRtpWorkflows, AWS-MediaLiveMultiplexWorkflows, AWS-MediaLiveToMediaPackageWorkflows, AWS-MediaTailorAdInsertionWorkflows, AWS-MediaConnectArqProtocolWorkflows, AWS-MediaConnectInputFailoverWorkflows)

The `aws-1000001`/`aws-1000008` form is the emulator's sequential resource mint; AWS's published group ids (`aws-1160891`, `aws-1484347`) are fixed, region-stable catalog values identical in every account, which is what makes this a content divergence and not a per-run id difference.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS SDK/CLI for medialive at it, signed for us-west-2.
2. Cause 1: GET /prod/cloudwatch-alarm-template-groups (aws medialive list-cloud-watch-alarm-template-groups). Look for the AWS-provided groups by id: no entry has id `aws-1160891` / arn `arn:aws:medialive:us-west-2::cloudwatch-alarm-template-group:aws-1160891` (AWS-CloudFrontGeneralWorkflows, templateCount 2); the group of that name is served with an emulator-minted id (`aws-1000001`) and a synthesized templateCount. Replay fixture medialive-catalog-list-cloud-watch-alarm-template-groups step task#1.
3. Cause 2: GET /prod/cloudwatch-alarm-templates (aws medialive list-cloud-watch-alarm-templates), and again with ?groupIdentifier=aws-1484347. No template with id `aws-0253256` / name AWS-ChannelInputErrorSecondsHigh-MediaLiveUdpAndRtmpWorkflows is returned, and the groupIdentifier filter on a published group id yields nothing. Replay fixture medialive-catalog-list-cloud-watch-alarm-templates step task#1.
4. Cause 3: GET /prod/offerings (aws medialive list-offerings), optionally ?resourceType=INPUT&codec=AVC&resolution=UHD. No offering with offeringId `304736645` or arn `arn:aws:medialive:us-west-2::offering:304736645` is returned, so PurchaseOffering has no published offering id to take. Replay fixture medialive-catalog-list-offerings step task#1.
5. Cause 4: POST /prod/cloudwatch-alarm-template-groups with {"name":"<probe-name>","description":"initial description"} -> 201 with an arn/id. Then GET /prod/cloudwatch-alarm-template-groups and look for an entry whose name is <probe-name>: it is absent, although GET /prod/cloudwatch-alarm-template-groups/<id> on the returned id answers 200. Replay fixture medialive-cloudwatch-alarm-template-group-lifecycle step task#3.
6. Re-run the corpus: these four clusters must pass, and the 97 steps that already pass must stay passing.
