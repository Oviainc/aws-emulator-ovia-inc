Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-origin-endpoint-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-origin-endpoint-lifecycle mediapackage-conflict-error-paths mediapackage-harvest-job-workflow
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.hlsPackage.adMarkers
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.hlsPackage.adTriggers
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.hlsPackage.adsOnDeliveryRestrictions
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.hlsPackage.includeDvbSubtitles
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.hlsPackage.includeIframeOnlyStream
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.hlsPackage.programDateTimeIntervalSeconds
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.hlsPackage.useAudioRenditionGroup
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.origination
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.timeDelaySeconds
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.hlsPackage.playlistType
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.hlsPackage.playlistWindowSeconds
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.startoverWindowSeconds
Issue-Key: mediapackage|ListOriginEndpoints|2xx->2xx|$.originEndpoints[arn="arn
Issue-Related: 20261022T124635Z-mediapackage-mutating-operations-do-not-persist-channel-and.md
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# MediaPackage origin-endpoint projections omit defaulted members

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOriginEndpoint

## Description
This issue bundles 5 causes that share ONE fault: real MediaPackage server-fills the origin-endpoint members a request leaves unset and returns them on every origin-endpoint projection, while the emulator omits them. A fixer must close all 5 causes below before submitting.

Cause 1 of 5 - CreateOriginEndpoint response omits server-filled members (fixture mediapackage-origin-endpoint-lifecycle, step task#2).
AWS: Smithy: CreateOriginEndpointResponse carries Origination (jsonName 'origination', enum), TimeDelaySeconds ('timeDelaySeconds'), StartoverWindowSeconds ('startoverWindowSeconds') and HlsPackage ('hlsPackage') whose members include AdMarkers ('adMarkers', enum), AdTriggers ('adTriggers', list), AdsOnDeliveryRestrictions ('adsOnDeliveryRestrictions', enum), ProgramDateTimeIntervalSeconds, IncludeIframeOnlyStream, UseAudioRenditionGroup and IncludeDvbSubtitles (all booleans/integers). The model declares no @default, so recorded behavior only: the recorded create response populates all of them even when the request omitted them (origination ALLOW, timeDelaySeconds/programDateTimeIntervalSeconds 0, adMarkers NONE, adsOnDeliveryRestrictions RESTRICTED, the full adTriggers list, and the three booleans false).
Emulator: returns 200 but leaves every one of those members absent from the body.
Why: a client that reads endpoint configuration back - to decide whether to update it, to render it, or to deserialize into a non-optional field - gets null where AWS always supplies a value, so a boolean check on includeIframeOnlyStream or an enum parse of origination fails on the emulator only.
Fix: the CreateOriginEndpoint handler must materialize these server-filled members into the stored endpoint, and the origin-endpoint serializer must emit them.
Traces: mediapackage-origin-endpoint-lifecycle#task#2.

Cause 2 of 5 - same omission on a create that also loses playlist/startover defaults (fixture mediapackage-conflict-error-paths, step setup#2).
AWS: same members as Cause 1, plus HlsPackage.PlaylistWindowSeconds ('playlistWindowSeconds') and HlsPackage.PlaylistType ('playlistType', enum), which the recording also shows populated (playlistWindowSeconds 60, playlistType EVENT) and StartoverWindowSeconds 0.
Emulator: all of those are absent from the 200 create response.
Why: the HLS packaging settings a player-config generator depends on are missing, so generated manifests/configuration differ from AWS.
Fix: same handler/serializer as Cause 1, extended to the playlist members.
Traces: mediapackage-conflict-error-paths#setup#2.

Cause 3 of 5 - same omission in the harvest-job setup path (fixture mediapackage-harvest-job-workflow, step setup#2).
AWS: same members as Cause 2 (origination, startoverWindowSeconds, timeDelaySeconds, hlsPackage playlistType/adMarkers/adTriggers/adsOnDeliveryRestrictions/programDateTimeIntervalSeconds/includeIframeOnlyStream/useAudioRenditionGroup/includeDvbSubtitles).
Emulator: absent on the 200 create response, which is what the harvest-job workflow then builds on.
Why: downstream operations that read the endpoint (harvest job creation/validation) see an under-populated endpoint, so failures here cascade into unrelated-looking divergences.
Fix: same handler/serializer.
Traces: mediapackage-harvest-job-workflow#setup#2.

Cause 4 of 5 - DescribeOriginEndpoint omits the same members (fixture mediapackage-origin-endpoint-lifecycle, step task#3).
AWS: Smithy: DescribeOriginEndpoint GET /origin_endpoints/{Id} -> the same output members as create; the read echoes the server-filled values.
Emulator: 200 with origination, timeDelaySeconds and the hlsPackage ad/playlist/boolean members all absent.
Why: the read path clients actually poll returns an incomplete endpoint, so it cannot be used to verify configuration.
Fix: the origin-endpoint read projection must emit the stored server-filled members.
Traces: mediapackage-origin-endpoint-lifecycle#task#3.

Cause 5 of 5 - ListOriginEndpoints omits the same members per listed endpoint (fixture mediapackage-origin-endpoint-lifecycle, step task#4).
AWS: Smithy: ListOriginEndpoints -> ListOriginEndpointsResponse.OriginEndpoints[] of OriginEndpoint, same member set.
Emulator: each entry of $.originEndpoints[] is missing origination, timeDelaySeconds and the hlsPackage ad/boolean members.
Why: enumeration-based tooling (inventory, drift detection) sees every endpoint as unconfigured.
Fix: the list projection must reuse the complete origin-endpoint serializer.
Traces: mediapackage-origin-endpoint-lifecycle#task#4.

## Evidence
Replay (trace set simparity-wire-aws-v1); all five steps are 200 expected / 200 actual with body members missing:
- mediapackage-origin-endpoint-lifecycle#task#2 CreateOriginEndpoint: $.origination missing (expected 'ALLOW'), $.timeDelaySeconds missing (expected 0), $.hlsPackage.adMarkers missing (expected 'NONE'), $.hlsPackage.adTriggers missing (expected ['SPLICE_INSERT','PROVIDER_ADVERTISEMENT','DISTRIBUTOR_ADVERTISEMENT','PROVIDER_PLACEMENT_OPPORTUNITY','DISTRIBUTOR_PLACEMENT_OPPORTUNITY']), $.hlsPackage.adsOnDeliveryRestrictions missing (expected 'RESTRICTED'), $.hlsPackage.programDateTimeIntervalSeconds missing (expected 0), $.hlsPackage.includeIframeOnlyStream / useAudioRenditionGroup / includeDvbSubtitles missing (expected false).
- mediapackage-conflict-error-paths#setup#2 CreateOriginEndpoint: the above plus $.startoverWindowSeconds missing (expected 0), $.hlsPackage.playlistWindowSeconds missing (expected 60), $.hlsPackage.playlistType missing (expected 'EVENT').
- mediapackage-harvest-job-workflow#setup#2 CreateOriginEndpoint: same member set as the conflict-error-paths create (minus playlistWindowSeconds).
- mediapackage-origin-endpoint-lifecycle#task#3 DescribeOriginEndpoint: same nine members missing.
- mediapackage-origin-endpoint-lifecycle#task#4 ListOriginEndpoints: same nine members missing under $.originEndpoints[arn="...origin_endpoints/<endpoint-id>"].
The requirement is that these members are present and server-filled; the particular ids/ARNs in the recording are environment-specific.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. CreateOriginEndpoint with only channelId/id/description/hlsPackage basics (fixture mediapackage-origin-endpoint-lifecycle step task#2, or mediapackage-conflict-error-paths/mediapackage-harvest-job-workflow setup#2) and inspect the 200 body: origination, timeDelaySeconds, startoverWindowSeconds and the hlsPackage ad/playlist/boolean members are absent.
3. DescribeOriginEndpoint (task#3) and ListOriginEndpoints (task#4) on the same endpoint omit the same members.
