Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackagev2-channel-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackagev2-channel-lifecycle mediapackagev2-channel-policy-lifecycle mediapackagev2-multiple-origin-endpoints-isolation mediapackagev2-error-paths mediapackagev2-create-origin-endpoint-multiple-hls-manifests-url-uniqueness mediapackagev2-origin-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2|CreateChannel|2xx->2xx|$.InputSwitchConfiguration
Issue-Key: mediapackagev2|CreateChannel|2xx->2xx|$.OutputHeaderConfiguration
Issue-Key: mediapackagev2|GetChannel|2xx->2xx|$.InputSwitchConfiguration
Issue-Key: mediapackagev2|GetChannel|2xx->2xx|$.OutputHeaderConfiguration
Issue-Key: mediapackagev2|CreateChannel|2xx->2xx|$.ETag
Issue-Key: mediapackagev2|CreateChannel|2xx->2xx|$.IngestEndpoints[*].Url
Issue-Key: mediapackagev2|CreateOriginEndpoint|2xx->2xx|$.HlsManifests[*].ManifestWindowSeconds
Issue-Key: mediapackagev2|CreateOriginEndpoint|2xx->2xx|$.HlsManifests[*].UriPathType
Issue-Key: mediapackagev2|CreateOriginEndpoint|2xx->2xx|$.HlsManifests[*].UrlEncodeChildManifest
Issue-Key: mediapackagev2|CreateOriginEndpoint|2xx->2xx|$.Segment.IncludeIframeOnlyStreams
Issue-Key: mediapackagev2|CreateOriginEndpoint|2xx->2xx|$.Segment.SegmentDurationSeconds
Issue-Key: mediapackagev2|CreateOriginEndpoint|2xx->2xx|$.Segment.SegmentName
Issue-Key: mediapackagev2|CreateOriginEndpoint|2xx->2xx|$.Segment.TsIncludeDvbSubtitles
Issue-Key: mediapackagev2|CreateOriginEndpoint|2xx->2xx|$.Segment.TsUseAudioRenditionGroup
Issue-Key: mediapackagev2|CreateOriginEndpoint|2xx->2xx|$.StartoverWindowSeconds
Issue-Key: mediapackagev2|CreateOriginEndpoint|2xx->2xx|$.StreamNameOutputMode
Issue-Key: mediapackagev2|CreateOriginEndpoint|2xx->2xx|$.UriSeparator
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.HlsManifests[*].ManifestWindowSeconds
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.HlsManifests[*].UriPathType
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.HlsManifests[*].UrlEncodeChildManifest
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.Segment.IncludeIframeOnlyStreams
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.Segment.SegmentDurationSeconds
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.Segment.SegmentName
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.Segment.TsIncludeDvbSubtitles
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.Segment.TsUseAudioRenditionGroup
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.StartoverWindowSeconds
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.StreamNameOutputMode
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.UriSeparator
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.HlsManifests[*]
Issue-Key: mediapackagev2|ListOriginEndpoints|2xx->2xx|$.Items[Arn="arn
Issue-Commit: 07653fe6d7ae123674bcbfa9037d42ab6af0ac40

# mediapackagev2 channel and origin-endpoint projections omit server-materialised configuration members

> _Note: this issue was identified against the code base at commit `07653fe6d7ae123674bcbfa9037d42ab6af0ac40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOriginEndpoint

## Description
Across every mediapackagev2 channel and origin-endpoint create/read/list response the emulator returns a 200 whose body is structurally thinner than AWS's: the members AWS materialises on the server side (the input-switch/output-header configuration blocks, the segment settings, the HLS-manifest settings, and the endpoint-level URI/stream-name modes) are absent entirely rather than present with their service-applied values. The status and the required identity members are correct, so this is purely a response-projection/state-materialisation gap. There are 4 distinct projection sites, carried by 8 replay clusters.

Cause 1 of 4: Channel projection drops InputSwitchConfiguration and OutputHeaderConfiguration.
AWS: Smithy: CreateChannel/GetChannel/UpdateChannel output both bind InputSwitchConfiguration (structure InputSwitchConfiguration, member MQCSInputSwitching: Boolean) and OutputHeaderConfiguration (structure OutputHeaderConfiguration, member PublishMQCS: Boolean). Neither member carries @default in the model, so the concrete values are recorded behavior only: the recording shows AWS always returns both structures on a channel, populated with the service-applied switching state (MQCSInputSwitching and PublishMQCS both false for a channel created without them).
Emulator: both members are absent from the JSON body (reported as missing, got None) on CreateChannel and GetChannel.
Why: a client that reads channel.InputSwitchConfiguration.MQCSInputSwitching or OutputHeaderConfiguration.PublishMQCS to decide whether MQCS input switching / CMSD header publication is on gets a null dereference or a silently wrong 'unset' instead of the server's actual boolean, and a read-modify-write of a channel cannot round-trip the blocks it never saw.
Fix: the mediapackagev2 channel response projection and the channel record in service state — the channel must store and emit these two configuration structures, defaulted at create time, not omit them.
Traces: mediapackagev2-channel-lifecycle#task#2 (CreateChannel), mediapackagev2-channel-lifecycle#task#3 (GetChannel), mediapackagev2-error-paths#task#7 (CreateChannel).

Cause 2 of 4: Origin-endpoint projection drops the Segment settings and the endpoint-level StartoverWindowSeconds / StreamNameOutputMode / UriSeparator.
AWS: Smithy: CreateOriginEndpoint/GetOriginEndpoint output bind Segment as @required (structure Segment with SegmentDurationSeconds @range 1..30, SegmentName @length 1..256 @pattern ^[a-zA-Z0-9_-]+$, TsUseAudioRenditionGroup, IncludeIframeOnlyStreams, TsIncludeDvbSubtitles), plus StartoverWindowSeconds (Integer), UriSeparator (enum UriSeparator) and StreamNameOutputMode (enum StreamNameOutputMode). Recorded behavior only for the values: AWS echoes the segment settings it applied and reports UriSeparator/StreamNameOutputMode for every endpoint (recording shows SegmentDurationSeconds 6, SegmentName 'segment', IncludeIframeOnlyStreams false, TsIncludeDvbSubtitles true, TsUseAudioRenditionGroup true, UriSeparator 'UNDERSCORE', StreamNameOutputMode 'INDEX', and the StartoverWindowSeconds the endpoint was created/updated with).
Emulator: Segment's members and all three endpoint-level members are absent (missing, got None); Segment is @required, so the emulator is emitting an output that the modeled shape does not permit.
Why: a client cannot read back the segmentation or URI/stream-name behavior of an endpoint it just created; packaging tooling that computes playback behavior from SegmentDurationSeconds, or that round-trips an endpoint through UpdateOriginEndpoint, loses every one of these settings.
Fix: the mediapackagev2 origin-endpoint response projection and the origin-endpoint record in service state — persist the server-applied segment and URI/stream-name settings at create time and emit them on every create/read.
Traces: mediapackagev2-create-origin-endpoint-multiple-hls-manifests-url-uniqueness#task#3 (CreateOriginEndpoint), #task#4 (GetOriginEndpoint), mediapackagev2-multiple-origin-endpoints-isolation#task#3 (CreateOriginEndpoint), #task#6 (GetOriginEndpoint).

Cause 3 of 4: HLS-manifest projection drops ManifestWindowSeconds, UriPathType and UrlEncodeChildManifest.
AWS: Smithy: HlsManifests on the origin-endpoint output targets GetHlsManifests of GetHlsManifestConfiguration, which binds ManifestName (@required), Url (@required), ManifestWindowSeconds (Integer), UriPathType (enum UriPathType, LEAF/ROOT) and UrlEncodeChildManifest (Boolean). Recorded behavior only for the values: AWS returns a fully-populated manifest configuration per manifest (recording shows ManifestWindowSeconds 60, UriPathType 'LEAF', UrlEncodeChildManifest false), and does so for each manifest when several are requested.
Emulator: each HlsManifests entry contains only ManifestName and Url; the other three members are absent. This holds for single-manifest and multi-manifest endpoints alike.
Why: a client reading the manifest window or the URI path type from the manifest configuration (for example to build a child-manifest URL or to size its playback window) finds nothing where AWS reports a concrete value, and cannot tell a LEAF-relative endpoint from a ROOT-relative one.
Fix: the mediapackagev2 HLS-manifest serializer/projection used by the origin-endpoint create, read and update paths.
Traces: mediapackagev2-create-origin-endpoint-multiple-hls-manifests-url-uniqueness#task#3 and #task#4 (two manifests, both entries thin), mediapackagev2-multiple-origin-endpoints-isolation#task#6 (whole HlsManifests[0] element compared unequal because of the three absent members).

Cause 4 of 4: ListOriginEndpoints items drop StreamNameOutputMode and UriSeparator.
AWS: Smithy: ListOriginEndpoints output Items targets OriginEndpointsList of OriginEndpointListConfiguration, which binds UriSeparator (enum UriSeparator) and StreamNameOutputMode (enum StreamNameOutputMode) alongside the identity members. Recorded behavior only for the values: the list projection reports the same values the per-endpoint read reports ('UNDERSCORE' / 'INDEX' in the recording).
Emulator: both members are absent from every item of the returned list, so the list projection is inconsistent with what AWS reports even though the item set and ARNs are right.
Why: a client that enumerates endpoints and filters or displays them by stream-name output mode or URI separator must issue a GetOriginEndpoint per item, and on this emulator that read is equally empty (Cause 2).
Fix: the mediapackagev2 ListOriginEndpoints item projection, sharing the stored endpoint fields added by Cause 2.
Traces: mediapackagev2-multiple-origin-endpoints-isolation#task#5, mediapackagev2-origin-endpoint-lifecycle#task#7 (same cluster).

## Evidence
All clusters are status-correct (expected 200, actual 200) with body members absent.

Cause 1 — mediapackagev2-channel-lifecycle#task#2 CreateChannel: $.InputSwitchConfiguration missing (expected {'MQCSInputSwitching': False}, got None); $.OutputHeaderConfiguration missing (expected {'PublishMQCS': False}, got None). Same two on mediapackagev2-channel-lifecycle#task#3 GetChannel and mediapackagev2-error-paths#task#7 CreateChannel.

Cause 2 — mediapackagev2-create-origin-endpoint-multiple-hls-manifests-url-uniqueness#task#3 CreateOriginEndpoint: $.Segment.IncludeIframeOnlyStreams missing (expected False), $.Segment.SegmentDurationSeconds missing (expected 6), $.Segment.SegmentName missing (expected 'segment'), $.Segment.TsIncludeDvbSubtitles missing (expected True), $.Segment.TsUseAudioRenditionGroup missing (expected True), $.StartoverWindowSeconds missing (expected 900), $.StreamNameOutputMode missing (expected 'INDEX'), $.UriSeparator missing (expected 'UNDERSCORE'). Repeated verbatim on #task#4 GetOriginEndpoint, on mediapackagev2-multiple-origin-endpoints-isolation#task#3/#task#6, and (with StartoverWindowSeconds 3600) on mediapackagev2-origin-endpoint-lifecycle#task#6.

Cause 3 — same two fixtures: $.HlsManifests[0].ManifestWindowSeconds missing (expected 60), $.HlsManifests[0].UriPathType missing (expected 'LEAF'), $.HlsManifests[0].UrlEncodeChildManifest missing (expected False), and identically for $.HlsManifests[1] on the two-manifest fixture. On mediapackagev2-multiple-origin-endpoints-isolation#task#6 the whole element diverges: expected {'ManifestName': 'index-a', 'ManifestWindowSeconds': 60, 'UriPathType': 'LEAF', 'Url': ..., 'UrlEncodeChildManifest': False}, got {'ManifestName': 'index-a', 'Url': ...}.

Cause 4 — mediapackagev2-multiple-origin-endpoints-isolation#task#5 ListOriginEndpoints: for both items (…originEndpoint/…-ep-a-… and …-ep-b-…) .StreamNameOutputMode missing (expected 'INDEX') and .UriSeparator missing (expected 'UNDERSCORE').

Not treated as part of this defect, in the same clusters: the ingest/egress hostname components and the ETag token values differ per environment and are not requirements.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus); the trace set is simparity-wire-aws-v1.
2. Cause 1: fixture mediapackagev2-channel-lifecycle, step task#2 — CreateChannel in an existing channel group, then step task#3 GetChannel; inspect the 200 body for InputSwitchConfiguration and OutputHeaderConfiguration.
3. Cause 2/3: fixture mediapackagev2-create-origin-endpoint-multiple-hls-manifests-url-uniqueness, step task#3 — CreateOriginEndpoint with two HLS manifests, then step task#4 GetOriginEndpoint; inspect Segment, StartoverWindowSeconds, StreamNameOutputMode, UriSeparator and each HlsManifests entry.
4. Cause 4: fixture mediapackagev2-multiple-origin-endpoints-isolation, step task#5 — ListOriginEndpoints for the channel; inspect each Items entry for StreamNameOutputMode and UriSeparator.
