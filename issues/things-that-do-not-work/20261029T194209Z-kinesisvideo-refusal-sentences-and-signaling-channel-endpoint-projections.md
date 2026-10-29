Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-signaling-channel-ttl-update-version-guard
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-signaling-channel-ttl-update-version-guard kinesisvideo-signaling-channel-lifecycle kinesis-video-stream-arn-tag-resource-surface kinesis-video-stream-lifecycle-create-describe-delete kinesisvideo-stream-lifecycle kinesisvideo-signaling-channel-endpoints
Issue-Shape-Id: com.amazonaws.kinesisvideo#KinesisVideo_20170930
Issue-Key: kinesisvideo|UpdateSignalingChannel|4xx->4xx|$.Message
Issue-Key: kinesisvideo|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: kinesisvideo|CreateStream|4xx->4xx|$.Message
Issue-Key: kinesisvideo|GetSignalingChannelEndpoint|4xx->4xx|$.Message
Issue-Key: kinesisvideo|GetSignalingChannelEndpoint|2xx->2xx|$.ResourceEndpointList[*].Protocol
Issue-Key: kinesisvideo|GetSignalingChannelEndpoint|2xx->2xx|$.ResourceEndpointList[*].ResourceEndpoint
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# kinesisvideo refusal sentences and signaling-channel endpoint projections are emulator-authored

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateSignalingChannel

## Description
This issue bundles 5 net-new root causes where Kinesis Video answers with the right status but an emulator-authored payload: four refusal sentences are reworded (and one names the wrong resource kind), and GetSignalingChannelEndpoint returns the requested endpoints in the wrong order. All five are kinesisvideo and sit in the response/refusal rendering of the signaling-channel and stream surfaces.

Cause 1 of 5 - UpdateSignalingChannel version-mismatch refusal is worded as the stream message.
AWS: Smithy UpdateSignalingChannel (POST /updateSignalingChannel) takes required ChannelARN and required CurrentVersion and models VersionMismatchException (@httpError 400). When CurrentVersion is not the channel's current version the service refuses with HTTP 400 and Message 'The provided channel version does not match the existing channel version.' - fixed text, no request or state values interpolated.
Emulator: HTTP 400 (correct) with Message 'The version specified is not the latest version. To get the latest version, use the DescribeSignalingChannel API.' - the shape's generic documentation prose, not the channel sentence the service emits.
Why: a client matching on the service's version-conflict sentence (or presenting it to an operator) sees text that never appears in AWS, so optimistic-concurrency handling and log fingerprints diverge.
Fix: UpdateSignalingChannel's version guard - render the channel version-mismatch sentence.
Traces: kinesis-video-signaling-channel-ttl-update-version-guard#task#5 (also #task#6, #task#7, kinesisvideo-signaling-channel-lifecycle#task#5, #task#6).

Cause 2 of 5 - ListTagsForResource's not-found refusal always says 'channel', even for a stream ARN.
AWS: ListTagsForResource (POST /ListTagsForResource) takes a required ResourceARN matching the kinesisvideo ResourceARN pattern, whose resource segment names the resource kind (stream or channel), and models ResourceNotFoundException (@httpError 404, documented 'Amazon Kinesis Video Streams can't find the stream that you specified'). For a stream ARN that resolves to no active stream the refusal is HTTP 404 with Message 'The requested stream is not found or not active.'; the resource word in the sentence follows the ARN's resource segment.
Emulator: HTTP 404 (correct) with Message 'The requested channel is not found or not active.' - the channel wording is hardcoded regardless of what the ARN names.
Why: the refusal describes a resource kind the caller never asked about, which misleads an operator debugging a stream ARN and breaks message matching on the stream path.
Fix: ListTagsForResource's not-found path - derive the resource word from the ARN's resource segment instead of fixing it to 'channel'.
Traces: kinesis-video-stream-arn-tag-resource-surface#task#8.

Cause 3 of 5 - CreateStream's duplicate-name refusal appends prose AWS does not send.
AWS: CreateStream (POST /createStream) models ResourceInUseException (@httpError 400) for a StreamName already taken. The refusal is HTTP 400 with Message 'The stream <StreamName> already exists.' - fixed text 'The stream ' / ' already exists.' with the requested StreamName as its only variable part.
Emulator: HTTP 400 with Message 'The stream <StreamName> already exists in this account and region.' - the correct interpolation but a trailing clause AWS never emits.
Why: the sentence differs from AWS for every duplicate create, so idempotency logic and assertions keyed on the service's wording fail.
Fix: CreateStream's conflict path - emit AWS's sentence.
Traces: kinesis-video-stream-lifecycle-create-describe-delete#task#5 (also kinesisvideo-stream-lifecycle#task#3).

Cause 4 of 5 - GetSignalingChannelEndpoint's WEBRTC refusal is reworded.
AWS: GetSignalingChannelEndpoint (POST /getSignalingChannelEndpoint) takes SingleMasterChannelEndpointConfiguration.Protocols (ChannelProtocol: WSS, HTTPS, WEBRTC). Asking for the WEBRTC endpoint on a channel with no media-storage configuration is refused with HTTP 400 and Message 'MediaStorageConfiguration is required for WEBRTC protocol' - fixed text naming the missing configuration and the protocol.
Emulator: HTTP 400 with Message 'The WEBRTC protocol is available only for a signaling channel configured for ingestion. Use UpdateMediaStorageConfiguration to configure it.' - a different sentence of the emulator's own composition.
Why: a client branching on this refusal to call UpdateMediaStorageConfiguration cannot recognise it, and the text it surfaces to users differs from AWS.
Fix: GetSignalingChannelEndpoint's WEBRTC precondition check - emit AWS's sentence.
Traces: kinesisvideo-signaling-channel-endpoints#task#5 (also #task#6, #task#7).

Cause 5 of 5 - GetSignalingChannelEndpoint returns the ResourceEndpointList in the wrong order.
AWS: recorded behavior only (the model fixes neither order): ResourceEndpointList is a list of ResourceEndpointListItem {Protocol, ResourceEndpoint}, and for a multi-protocol request the entries come back in the order the request's SingleMasterChannelEndpointConfiguration.Protocols list names them - a request for [HTTPS, WSS] yields the HTTPS item first and the WSS item second.
Emulator: emits the entries in a fixed internal order (WSS first, then HTTPS), ignoring the requested order: $.ResourceEndpointList[0].Protocol is WSS where the recording has HTTPS and [1] is HTTPS where the recording has WSS.
Why: a client that reads the endpoints positionally (list[0] for the protocol it asked for first) hands a websocket URL to an HTTPS client and vice versa, so signalling connections are attempted against the wrong scheme.
Fix: GetSignalingChannelEndpoint's response projection - build ResourceEndpointList in the order of the requested Protocols.
Traces: kinesisvideo-signaling-channel-endpoints#task#4.

## Evidence
Replay of corpus simparity-wire-aws-v1 (emulator up; 147 steps, 85 passed).
Cause 1 - kinesis-video-signaling-channel-ttl-update-version-guard#task#5 (plus #task#6, #task#7, kinesisvideo-signaling-channel-lifecycle#task#5, #task#6): status 400 matches; $.Message expected 'The provided channel version does not match the existing channel version.', got 'The version specified is not the latest version. To get the latest version, use the DescribeSignalingChannel API.'
Cause 2 - kinesis-video-stream-arn-tag-resource-surface#task#8: status 404 matches; $.Message expected 'The requested stream is not found or not active.', got 'The requested channel is not found or not active.'
Cause 3 - kinesis-video-stream-lifecycle-create-describe-delete#task#5 (also kinesisvideo-stream-lifecycle#task#3): status 400 matches; $.Message expected 'The stream <name> already exists.', got 'The stream <name> already exists in this account and region.' (the fixture's name sim2real-probe-kv-life-e7f334d8 fills the template in both; only the trailing clause is the divergence).
Cause 4 - kinesisvideo-signaling-channel-endpoints#task#5 (plus #task#6, #task#7): status 400 matches; $.Message expected 'MediaStorageConfiguration is required for WEBRTC protocol', got 'The WEBRTC protocol is available only for a signaling channel configured for ingestion. Use UpdateMediaStorageConfiguration to configure it.'
Cause 5 - kinesisvideo-signaling-channel-endpoints#task#4: status 200 matches; $.ResourceEndpointList[0].Protocol expected 'HTTPS', got 'WSS'; $.ResourceEndpointList[1].Protocol expected 'WSS', got 'HTTPS'.
Excluded from this issue: the $.ResourceEndpoint / $.DataEndpoint value mismatches in the same clusters - those are host:port values of the local listener and differ per environment.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay the wire corpus simparity-wire-aws-v1.
2. Cause 1: CreateSignalingChannel, then POST /updateSignalingChannel with a CurrentVersion that is not the channel's current version (kinesis-video-signaling-channel-ttl-update-version-guard#task#5) - compare $.Message to 'The provided channel version does not match the existing channel version.'
3. Cause 2: POST /ListTagsForResource with a STREAM ARN that names no active stream (kinesis-video-stream-arn-tag-resource-surface#task#8) - the message says 'channel'.
4. Cause 3: CreateStream twice with the same StreamName (kinesis-video-stream-lifecycle-create-describe-delete#task#5) - the second refusal appends 'in this account and region'.
5. Cause 4: CreateSignalingChannel without a media-storage configuration, then POST /getSignalingChannelEndpoint asking for Protocols [WEBRTC] (kinesisvideo-signaling-channel-endpoints#task#5) - compare $.Message to 'MediaStorageConfiguration is required for WEBRTC protocol'.
6. Cause 5: POST /getSignalingChannelEndpoint with Protocols [HTTPS, WSS] (kinesisvideo-signaling-channel-endpoints#task#4) - read $.ResourceEndpointList[*].Protocol: the order is WSS then HTTPS.
