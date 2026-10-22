Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-channel-lifecycle-tags-logs
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-channel-lifecycle-tags-logs mediapackage-origin-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage|DescribeChannel|2xx->2xx|$.description
Issue-Key: mediapackage|DescribeChannel|2xx->2xx|$.egressAccessLogs.logGroupName
Issue-Key: mediapackage|DescribeChannel|2xx->2xx|$.ingressAccessLogs.logGroupName
Issue-Key: mediapackage|DescribeChannel|2xx->2xx|$.tags.owner
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.description
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.hlsPackage.adMarkers
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.hlsPackage.adTriggers
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.hlsPackage.adsOnDeliveryRestrictions
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.hlsPackage.includeDvbSubtitles
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.hlsPackage.includeIframeOnlyStream
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.hlsPackage.playlistWindowSeconds
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.hlsPackage.programDateTimeIntervalSeconds
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.hlsPackage.segmentDurationSeconds
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.hlsPackage.useAudioRenditionGroup
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.origination
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.startoverWindowSeconds
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.timeDelaySeconds
Issue-Related: 20261022T124635Z-mediapackage-generated-ingest-endpoint-usernames-have-the-wrong.md
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# MediaPackage mutating operations do not persist channel and endpoint changes

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateChannel

## Description
This issue bundles 3 causes that share ONE fault: the emulator accepts MediaPackage's mutating operations with a 2xx but does not write their effect into state, so the following read returns the pre-mutation resource. A fixer must close all 3 causes below before submitting.

Cause 1 of 3 - UpdateChannel's description is dropped (fixture mediapackage-channel-lifecycle-tags-logs, step task#5, DescribeChannel after an UpdateChannel).
AWS: Smithy: UpdateChannel PUT /channels/{Id}, input UpdateChannelRequest.Description (jsonName 'description'), output UpdateChannelResponse.Description; doc 'Updates an existing Channel.' A subsequent DescribeChannel returns the updated description.
Emulator: DescribeChannel still returns the create-time description after the update (replay: $.description value_mismatch, expected the updated text, got the original text), status 200 either way.
Why: a client that updates a channel's description and reads it back - or any config-reconciliation loop that compares desired vs actual - sees its write silently lost, with no error to retry on.
Fix: the UpdateChannel handler must apply Description to the stored channel (channel state), not just echo it in the response.
Traces: mediapackage-channel-lifecycle-tags-logs#task#5.

Cause 2 of 3 - ConfigureLogs access-log settings and resource tags are dropped (fixture mediapackage-channel-lifecycle-tags-logs, step task#11, DescribeChannel after ConfigureLogs + TagResource).
AWS: Smithy: ConfigureLogs PUT /channels/{Id}/configure_logs with IngressAccessLogs / EgressAccessLogs (jsonName 'ingressAccessLogs' / 'egressAccessLogs'), doc 'Changes the Channel's properities to configure log subscription'; its output and DescribeChannelResponse both carry those members plus Tags (jsonName 'tags'), and TagResource POST /tags/{ResourceArn} (204) adds tags that DescribeChannel then reports.
Emulator: after ConfigureLogs and TagResource, DescribeChannel omits $.ingressAccessLogs.logGroupName and $.egressAccessLogs.logGroupName entirely and omits the tag key ('owner'), and still reports the stale description from Cause 1.
Why: a client cannot confirm that log subscription or tagging took effect; infrastructure tooling that reads tags/log groups back to decide whether to re-apply will loop forever or report drift that does not exist.
Fix: the ConfigureLogs handler and the tagging handler must write IngressAccessLogs/EgressAccessLogs and Tags into channel state, and the DescribeChannel projection must emit them when set.
Traces: mediapackage-channel-lifecycle-tags-logs#task#11.

Cause 3 of 3 - UpdateOriginEndpoint's changes are dropped (fixture mediapackage-origin-endpoint-lifecycle, step task#6, DescribeOriginEndpoint after an UpdateOriginEndpoint).
AWS: Smithy: UpdateOriginEndpoint PUT /origin_endpoints/{Id}, input members Description, StartoverWindowSeconds, HlsPackage (HlsPackage.SegmentDurationSeconds, HlsPackage.PlaylistWindowSeconds), doc 'Updates an existing OriginEndpoint.'; DescribeOriginEndpoint returns the updated values.
Emulator: DescribeOriginEndpoint after the update still returns the create-time values - $.description (original instead of updated), $.startoverWindowSeconds (create value instead of the updated one), $.hlsPackage.segmentDurationSeconds and $.hlsPackage.playlistWindowSeconds (both create values) - at status 200.
Why: packaging configuration a client changes never takes effect in the emulator's view, so any test or tool that edits an endpoint and verifies it fails against the emulator while passing on AWS.
Fix: the UpdateOriginEndpoint handler must merge its input members into the stored origin endpoint (origin-endpoint state).
Traces: mediapackage-origin-endpoint-lifecycle#task#6. (The same step also omits origination/timeDelaySeconds/hlsPackage defaults - that omission is the separate defaults issue filed alongside this one; here the cause is the lost mutation.)

## Evidence
Replay (trace set simparity-wire-aws-v1), every step 200 expected / 200 actual:
- mediapackage-channel-lifecycle-tags-logs#task#5 DescribeChannel: $.description value_mismatch (expected 'updated description', got 'initial description').
- mediapackage-channel-lifecycle-tags-logs#task#11 DescribeChannel: $.description value_mismatch (expected 'updated description', got 'initial description'); $.tags.owner missing (expected 'probe', got None); $.ingressAccessLogs.logGroupName missing (expected a '/aws/MediaPackage/<channel>-ingress' group, got None); $.egressAccessLogs.logGroupName missing (expected the matching '-egress' group, got None).
- mediapackage-origin-endpoint-lifecycle#task#6 DescribeOriginEndpoint: $.description value_mismatch (expected 'updated endpoint', got 'initial endpoint'); $.startoverWindowSeconds value_mismatch (expected 600, got 300); $.hlsPackage.segmentDurationSeconds value_mismatch (expected 10, got 6); $.hlsPackage.playlistWindowSeconds value_mismatch (expected 120, got 60).
The recorded strings/numbers are the fixture's inputs echoed back; the requirement is that the read reflects the preceding write, not these particular values.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. CreateChannel, then UpdateChannel with a new description, then DescribeChannel (fixture mediapackage-channel-lifecycle-tags-logs, steps up to task#5): the description is the original.
3. ConfigureLogs with ingress/egress log group names and TagResource with a tag, then DescribeChannel (task#11): logGroupName members and the tag are absent.
4. CreateOriginEndpoint, UpdateOriginEndpoint changing description/startoverWindowSeconds/hlsPackage, then DescribeOriginEndpoint (fixture mediapackage-origin-endpoint-lifecycle, task#6): all create-time values are returned.
