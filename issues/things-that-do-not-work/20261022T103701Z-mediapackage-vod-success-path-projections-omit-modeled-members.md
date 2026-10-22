Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-vod-configure-egress-logs
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-vod-configure-egress-logs mediapackage-vod-list-packaging-groups-pagination mediapackage-vod-packaging-configuration-multi-format mediapackage-vod-packaging-configuration-dash-round-trip mediapackage-vod-packaging-configuration-mss-round-trip mediapackage-vod-list-assets-filtered-by-packaging-group mediapackage-vod-packaging-group-lifecycle mediapackage-vod-tagging-round-trip
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: mediapackage-vod|CreatePackagingGroup|2xx->2xx|$.approximateAssetCount
Issue-Key: mediapackage-vod|CreatePackagingGroup|2xx->2xx|$.domainName
Issue-Key: mediapackage-vod|DescribePackagingGroup|2xx->2xx|$.egressAccessLogs
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|status
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.hlsPackage.hlsManifests[*].includeIframeOnlyStream
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.hlsPackage.hlsManifests[*].programDateTimeIntervalSeconds
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.hlsPackage.hlsManifests[*].repeatExtXKey
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.hlsPackage.includeDvbSubtitles
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.hlsPackage.useAudioRenditionGroup
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.dashPackage.dashManifests[*].minBufferTimeSeconds
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.dashPackage.dashManifests[*].profile
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.dashPackage.dashManifests[*].scteMarkersSource
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.dashPackage.includeEncoderConfigurationInSegments
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.dashPackage.includeIframeOnlyStream
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.dashPackage.periodTriggers
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.dashPackage.segmentTemplateFormat
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.cmafPackage.hlsManifests[*].adMarkers
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.cmafPackage.hlsManifests[*].includeIframeOnlyStream
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.cmafPackage.hlsManifests[*].programDateTimeIntervalSeconds
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.cmafPackage.hlsManifests[*].repeatExtXKey
Issue-Key: mediapackage-vod|CreatePackagingConfiguration|2xx->2xx|$.cmafPackage.includeEncoderConfigurationInSegments
Issue-Key: mediapackage-vod|ListAssets|2xx->2xx|$.assets[*]
Issue-Key: mediapackage-vod|DescribePackagingGroup|4xx->2xx|$
Issue-Key: mediapackage-vod|DescribePackagingGroup|4xx->2xx|$.message
Issue-Key: mediapackage-vod|DescribePackagingGroup|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediapackage-vod|DescribePackagingGroup|4xx->2xx|status
Issue-Commit: adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf

# MediaPackage VOD success-path projections omit modeled members and lifecycle state is not persisted

> _Note: this issue was identified against the code base at commit `adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePackagingGroup

## Description
Nine distinct causes, all on the MediaPackage VOD (restJson1, sdkId "MediaPackage Vod") success paths: the emulator's resource projections drop members AWS always materialises, CreatePackagingConfiguration answers the wrong success status, writes to a packaging group are not persisted, ListAssets does not project the asset it just created, and DeletePackagingGroup leaves the group readable. A fixer must close ALL NINE before submitting.

=== Cause 1 of 9: CreatePackagingGroup response omits approximateAssetCount and its domainName fails the recorded shape ===
AWS: recorded behavior only for approximateAssetCount — Smithy models ApproximateAssetCount (jsonName approximateAssetCount, __integer) on DescribePackagingGroupResponse but not on CreatePackagingGroupResponse, while the recording shows the create response carrying approximateAssetCount = 0 for a brand-new group. DomainName (jsonName domainName, __string) IS modeled on CreatePackagingGroupResponse and AWS returns a server-materialised egress hostname for it.
Emulator: answers 200 with the group but no approximateAssetCount member at all, and its domainName value does not satisfy the fixture's shape assertion for that member.
Why: a client that reads approximateAssetCount off the create response (a console or SDK waiter that seeds its view of the group from the create reply) gets a null/absent integer and either throws on deserialization of a non-optional projection or renders an empty asset count; a client that parses domainName to build an egress URL builds one of the wrong form.
Fix: the CreatePackagingGroup handler's response serializer in the mediapackagevod service crate — materialise approximateAssetCount (0 for a new group) and emit domainName in the service's egress-hostname form.
Traces: mediapackage-vod-configure-egress-logs#setup#1 (representative; same cluster also mediapackage-vod-list-packaging-groups-pagination#task#1, #task#2 and mediapackage-vod-packaging-configuration-multi-format#setup#1).

=== Cause 2 of 9: ConfigureLogs is not persisted, so DescribePackagingGroup never returns egressAccessLogs ===
AWS: Smithy: ConfigureLogs PUT /packaging_groups/{Id}/configure_logs, whose job per its own doc is "Changes the packaging group's properities to configure log subscription"; DescribePackagingGroupResponse.EgressAccessLogs (jsonName egressAccessLogs, target EgressAccessLogs) then reports the configured log group. After a successful ConfigureLogs the recording shows describe returning egressAccessLogs with the logGroupName that was configured (example: {"logGroupName": "/aws/MediaPackage/..."}).
Emulator: DescribePackagingGroup answers 200 with egressAccessLogs entirely absent — the ConfigureLogs mutation never reaches the packaging-group state.
Why: a client that configures egress access logging and then reads the group back to confirm it sees no log subscription, so it concludes logging is off and either re-configures in a loop or reports the group as unlogged.
Fix: the ConfigureLogs handler's state write in the mediapackagevod service crate (store egressAccessLogs on the packaging-group record) and the DescribePackagingGroup projection that reads it.
Traces: mediapackage-vod-configure-egress-logs#task#3 (representative; same cluster also #task#5).

=== Cause 3 of 9: CreatePackagingConfiguration answers 200 instead of the recorded 202 ===
AWS: recorded behavior only — the Smithy model binds CreatePackagingConfiguration to POST /packaging_configurations with no explicit @http code, but the recordings of the DASH and MSS round trips show AWS answering HTTP 202 on successful creation.
Emulator: answers HTTP 200.
Why: a client or SDK waiter that discriminates "accepted, packaging configuration is being provisioned" (202) from a plain 200 takes the wrong branch, and any status-code assertion on the create call fails against the emulator.
Fix: the CreatePackagingConfiguration handler's success status in the mediapackagevod service crate.
Traces: mediapackage-vod-packaging-configuration-dash-round-trip#task#2 (representative; same cluster also mediapackage-vod-packaging-configuration-mss-round-trip#task#2).

=== Cause 4 of 9: CreatePackagingConfiguration drops the defaulted hlsPackage members from its response ===
AWS: Smithy: HlsPackage.UseAudioRenditionGroup (jsonName useAudioRenditionGroup, __boolean), HlsPackage.IncludeDvbSubtitles (jsonName includeDvbSubtitles, __boolean), HlsManifest.ProgramDateTimeIntervalSeconds (jsonName programDateTimeIntervalSeconds, __integer), HlsManifest.IncludeIframeOnlyStream (jsonName includeIframeOnlyStream, __boolean), HlsManifest.RepeatExtXKey (jsonName repeatExtXKey, __boolean). The recording shows AWS echoing all of them with their server-side defaults (the booleans false, the interval 0) even when the request omitted them. The same step also answers 202, not 200 (see Cause 3).
Emulator: returns the hlsPackage block with every one of those five members absent.
Why: a client that round-trips the created configuration — reads it back and re-sends it, or diffs it against its desired state — sees unset booleans instead of explicit false and an absent interval instead of 0, so it computes a spurious drift and rewrites the configuration on every reconcile.
Fix: the CreatePackagingConfiguration response serializer for HlsPackage/HlsManifest in the mediapackagevod service crate (materialise server-side defaults rather than echoing only what the request supplied).
Traces: mediapackage-vod-packaging-configuration-multi-format#task#2.

=== Cause 5 of 9: CreatePackagingConfiguration drops the defaulted dashPackage members from its response ===
AWS: Smithy: DashPackage.SegmentTemplateFormat (jsonName segmentTemplateFormat, enum SegmentTemplateFormat), DashPackage.PeriodTriggers (jsonName periodTriggers, list), DashPackage.IncludeEncoderConfigurationInSegments and DashPackage.IncludeIframeOnlyStream (__boolean), DashManifest.Profile (enum Profile), DashManifest.MinBufferTimeSeconds (__integer), DashManifest.ScteMarkersSource (enum ScteMarkersSource). The recording shows AWS echoing all of them with server-side defaults (segmentTemplateFormat NUMBER_WITH_TIMELINE, periodTriggers [], profile NONE, minBufferTimeSeconds 30, scteMarkersSource SEGMENTS, the booleans false). The same step also answers 202, not 200 (see Cause 3).
Emulator: returns the dashPackage block with all seven members absent.
Why: same round-trip breakage as Cause 4, and additionally a client reading minBufferTimeSeconds or profile to configure a player gets nothing where AWS always supplies a value.
Fix: the CreatePackagingConfiguration response serializer for DashPackage/DashManifest in the mediapackagevod service crate.
Traces: mediapackage-vod-packaging-configuration-multi-format#task#3.

=== Cause 6 of 9: CreatePackagingConfiguration drops the defaulted cmafPackage members from its response ===
AWS: Smithy: CmafPackage.IncludeEncoderConfigurationInSegments (jsonName includeEncoderConfigurationInSegments, __boolean) and, on the nested HlsManifest list, AdMarkers (enum AdMarkers), ProgramDateTimeIntervalSeconds (__integer), IncludeIframeOnlyStream and RepeatExtXKey (__boolean). The recording shows AWS echoing all of them with server-side defaults (adMarkers NONE, the interval 0, the booleans false). The same step also answers 202, not 200 (see Cause 3).
Emulator: returns the cmafPackage block with all five members absent.
Why: same round-trip breakage as Causes 4 and 5, on the CMAF packaging path.
Fix: the CreatePackagingConfiguration response serializer for CmafPackage in the mediapackagevod service crate.
Traces: mediapackage-vod-packaging-configuration-multi-format#task#4.

=== Cause 7 of 9: ListAssets returns an empty collection for an asset that was created and is readable ===
AWS: Smithy: ListAssets GET /assets, ListAssetsResponse.Assets (jsonName assets, list of AssetShallow). AWS lists every asset in the account, filtered by packagingGroupId when that filter is supplied, and each AssetShallow carries id, arn, sourceArn, sourceRoleArn, packagingGroupId, createdAt and tags.
Emulator: answers 200 but assets[0] is absent — the list is empty even though the asset was created earlier in the same fixture and the create call succeeded.
Why: a client that enumerates assets to find what belongs to a packaging group (before deleting the group, or to render an inventory) sees none, so it believes the group is empty and proceeds to operations that AWS would refuse, or silently reports zero assets.
Fix: the ListAssets handler and its packagingGroupId filter in the mediapackagevod service crate — created assets must be indexed into the collection the list reads, and the AssetShallow projection must carry the modeled members.
Traces: mediapackage-vod-list-assets-filtered-by-packaging-group#task#9.

=== Cause 8 of 9: DeletePackagingGroup does not remove the group — the delete-confirmation poll never observes 404 ===
AWS: Smithy: DeletePackagingGroup DELETE /packaging_groups/{Id} with @http code 202; DescribePackagingGroup GET /packaging_groups/{Id} then fails with NotFoundException (errorKind client, httpError 404, member Message jsonName message) because "The requested resource does not exist."
Emulator: after the delete, DescribePackagingGroup keeps answering HTTP 200 with the full group projection (arn, createdAt, domainName, id, approximateAssetCount, tags) — the teardown poll retried 15 times and never saw 404, so the response carries neither the message member nor the x-amzn-errortype header.
Why: the delete is a no-op, so the emulator leaks resources across a client's lifecycle: a client that deletes a packaging group and polls for its disappearance hangs until its own timeout, and a client that re-creates the same id afterwards hits a spurious conflict.
Fix: the DeletePackagingGroup handler's state removal in the mediapackagevod service crate, and the DescribePackagingGroup lookup that must then raise NotFoundException/404.
Traces: mediapackage-vod-packaging-configuration-multi-format#teardown#12.

=== Cause 9 of 9: DescribePackagingGroup answers 200 for a deleted group instead of 404 NotFoundException ===
AWS: Smithy: DescribePackagingGroup errors include NotFoundException (errorKind client, httpError 404); the recording shows the 404 body carrying a message member and the header x-amzn-errortype: NotFoundException.
Emulator: answers HTTP 200 with no message member and no x-amzn-errortype header, i.e. the group is still served after its lifecycle ended.
Why: a client cannot distinguish a live packaging group from a deleted one, so error handling keyed on NotFoundException never fires and the client operates on a resource AWS would have told it is gone.
Fix: the DescribePackagingGroup handler's existence check and NotFoundException mapping in the mediapackagevod service crate (same state path as Cause 8; verify both clusters, they are reached by different fixtures).
Traces: mediapackage-vod-packaging-group-lifecycle#task#8 (representative; same cluster also mediapackage-vod-tagging-round-trip#teardown#8).

## Evidence
All nine causes are wire-replay divergences from trace set simparity-wire-aws-v1 (service mediapackage-vod, emulator up, 0 state_reset_failures).

Cause 1 — mediapackage-vod-configure-egress-logs#setup#1 CreatePackagingGroup, expected 200 / actual 200:
  $.domainName: value_mismatch (expected '<shaped-not-asserted>', got 'bc3ef5a2362366c8.egress.mediapackage-vod.us-west-2.amazonaws.com')
  $.approximateAssetCount: missing (expected 0, got None)
  affected_count 4 (also mediapackage-vod-list-packaging-groups-pagination#task#1, #task#2, mediapackage-vod-packaging-configuration-multi-format#setup#1)

Cause 2 — mediapackage-vod-configure-egress-logs#task#3 DescribePackagingGroup, expected 200 / actual 200:
  $.egressAccessLogs: missing (expected {'logGroupName': '/aws/MediaPackage/sim2real-probe-mediapackage-vod-pkggroup-78a6215b'}, got None)
  affected_count 2 (also #task#5)

Cause 3 — mediapackage-vod-packaging-configuration-dash-round-trip#task#2 CreatePackagingConfiguration:
  status: expected 202, got 200
  affected_count 2 (also mediapackage-vod-packaging-configuration-mss-round-trip#task#2)

Cause 4 — mediapackage-vod-packaging-configuration-multi-format#task#2 CreatePackagingConfiguration:
  status: expected 202, got 200
  $.hlsPackage.useAudioRenditionGroup: missing (expected False, got None)
  $.hlsPackage.includeDvbSubtitles: missing (expected False, got None)
  $.hlsPackage.hlsManifests[0].programDateTimeIntervalSeconds: missing (expected 0, got None)
  $.hlsPackage.hlsManifests[0].includeIframeOnlyStream: missing (expected False, got None)
  $.hlsPackage.hlsManifests[0].repeatExtXKey: missing (expected False, got None)

Cause 5 — mediapackage-vod-packaging-configuration-multi-format#task#3 CreatePackagingConfiguration:
  status: expected 202, got 200
  $.dashPackage.segmentTemplateFormat: missing (expected 'NUMBER_WITH_TIMELINE', got None)
  $.dashPackage.periodTriggers: missing (expected [], got None)
  $.dashPackage.dashManifests[0].profile: missing (expected 'NONE', got None)
  $.dashPackage.dashManifests[0].minBufferTimeSeconds: missing (expected 30, got None)
  $.dashPackage.dashManifests[0].scteMarkersSource: missing (expected 'SEGMENTS', got None)
  $.dashPackage.includeEncoderConfigurationInSegments: missing (expected False, got None)
  $.dashPackage.includeIframeOnlyStream: missing (expected False, got None)

Cause 6 — mediapackage-vod-packaging-configuration-multi-format#task#4 CreatePackagingConfiguration:
  status: expected 202, got 200
  $.cmafPackage.hlsManifests[0].adMarkers: missing (expected 'NONE', got None)
  $.cmafPackage.hlsManifests[0].programDateTimeIntervalSeconds: missing (expected 0, got None)
  $.cmafPackage.hlsManifests[0].includeIframeOnlyStream: missing (expected False, got None)
  $.cmafPackage.hlsManifests[0].repeatExtXKey: missing (expected False, got None)
  $.cmafPackage.includeEncoderConfigurationInSegments: missing (expected False, got None)

Cause 7 — mediapackage-vod-list-assets-filtered-by-packaging-group#task#9 ListAssets, expected 200 / actual 200:
  $.assets[0]: missing (expected an AssetShallow with id/arn/sourceArn/sourceRoleArn/packagingGroupId/createdAt/tags, got None)

Cause 8 — mediapackage-vod-packaging-configuration-multi-format#teardown#12 DescribePackagingGroup:
  status: expected 404, got 200
  $.message: missing (expected a not-found message, got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  $: poll: status 200 not in [404] after 15 attempt(s); actual body {'approximateAssetCount': 0, 'arn': 'arn:aws:mediapackage-vod:us-west-2:000000000000:packaging-groups/...', 'createdAt': ..., 'domainName': ..., 'id': ..., 'tags': {}}

Cause 9 — mediapackage-vod-packaging-group-lifecycle#task#8 DescribePackagingGroup:
  status: expected 404, got 200
  $.message: missing (expected a not-found message, got None)
  $headers.x-amzn-errortype: missing (expected 'NotFoundException', got None)
  affected_count 2 (also mediapackage-vod-tagging-round-trip#teardown#8)

Model citations taken from services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json (restJson1, sigv4, 17 operations). Recorded hostnames, generated ids, timestamps and log-group names above are examples, not requirements; the required facts are the statuses, the presence of the modeled members and their server-side default values.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay trace set simparity-wire-aws-v1 against it (replay_wire_corpus).
3. Cause 1: fixture mediapackage-vod-configure-egress-logs, step setup#1 — CreatePackagingGroup; inspect the 200 body for approximateAssetCount and domainName.
4. Cause 2: same fixture, step task#3 — ConfigureLogs on that group, then DescribePackagingGroup; inspect egressAccessLogs.
5. Cause 3: fixture mediapackage-vod-packaging-configuration-dash-round-trip, step task#2 — CreatePackagingConfiguration; observe HTTP 200 where 202 is recorded.
6. Causes 4-6: fixture mediapackage-vod-packaging-configuration-multi-format, steps task#2 (HLS), task#3 (DASH), task#4 (CMAF) — CreatePackagingConfiguration omitting request-unset members; inspect hlsPackage/dashPackage/cmafPackage for the defaulted members.
7. Cause 7: fixture mediapackage-vod-list-assets-filtered-by-packaging-group, step task#9 — CreateAsset earlier in the fixture, then ListAssets filtered by packagingGroupId; the assets list comes back empty.
8. Cause 8: fixture mediapackage-vod-packaging-configuration-multi-format, step teardown#12 — DeletePackagingGroup, then poll DescribePackagingGroup for 404; it stays 200 for all 15 attempts.
9. Cause 9: fixture mediapackage-vod-packaging-group-lifecycle, step task#8 — DeletePackagingGroup, then DescribePackagingGroup; expect 404 NotFoundException with x-amzn-errortype, observe 200.
