Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-signaling-channel-tag-resource-surface
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-signaling-channel-tag-resource-surface kinesis-video-signaling-channel-ttl-update-version-guard kinesis-video-stream-arn-tag-resource-surface kinesisvideo-signaling-channel-lifecycle kinesisvideo-signaling-channel-endpoints
Issue-Shape-Id: com.amazonaws.kinesisvideo#KinesisVideo_20170930
Issue-Key: kinesisvideo|CreateSignalingChannel|2xx->5xx|status
Issue-Key: kinesisvideo|ListTagsForResource|4xx->5xx|$.Message
Issue-Key: kinesisvideo|ListTagsForResource|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|ListTagsForResource|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|ListTagsForResource|4xx->5xx|status
Issue-Key: kinesisvideo|DescribeSignalingChannel|2xx->5xx|$
Issue-Key: kinesisvideo|DescribeSignalingChannel|2xx->5xx|$.ChannelInfo
Issue-Key: kinesisvideo|DescribeSignalingChannel|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|DescribeSignalingChannel|2xx->5xx|status
Issue-Key: kinesisvideo|CreateSignalingChannel|2xx->5xx|$.ChannelARN
Issue-Key: kinesisvideo|CreateSignalingChannel|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|GetSignalingChannelEndpoint|4xx->5xx|$.Message
Issue-Key: kinesisvideo|GetSignalingChannelEndpoint|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|GetSignalingChannelEndpoint|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|GetSignalingChannelEndpoint|4xx->5xx|status
Issue-Key: kinesisvideo|DescribeMediaStorageConfiguration|2xx->5xx|$.MediaStorageConfiguration
Issue-Key: kinesisvideo|DescribeMediaStorageConfiguration|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|DescribeMediaStorageConfiguration|2xx->5xx|status
Issue-Key: kinesisvideo|CreateSignalingChannel|4xx->5xx|$.Message
Issue-Key: kinesisvideo|CreateSignalingChannel|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|CreateSignalingChannel|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|CreateSignalingChannel|4xx->5xx|status
Issue-Key: kinesisvideo|DescribeSignalingChannel|4xx->5xx|$.Message
Issue-Key: kinesisvideo|DescribeSignalingChannel|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|DescribeSignalingChannel|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|DescribeSignalingChannel|4xx->5xx|status
Issue-Key: kinesisvideo|ListSignalingChannels|2xx->5xx|$.ChannelInfoList
Issue-Key: kinesisvideo|ListSignalingChannels|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|ListSignalingChannels|2xx->5xx|status
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# kinesisvideo signaling channel operations return AmbiguousService 500 across the WebRTC control surface

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSignalingChannel

## Description
This issue bundles 9 causes, all of the kinesisvideo signaling-channel (WebRTC control plane) surface, which share ONE emulator defect. A fixer must close all 9 (the representative in Issue-Trace/Issue-Step plus every fixture#step named under the causes below) before submitting.

SHARED ROOT CAUSE: the emulator's front-door router resolves an inbound restJson1 request to a registered service crate using only the SigV4 credential-scope service name / endpoint prefix. Five registered services share endpointPrefix 'kinesisvideo', so the router finds five candidates, declines to pick one, and answers HTTP 500 with the unmodeled error code 'AmbiguousService' instead of dispatching on HTTP method + request URI. The candidate list prints 'kinesisvideo' five times, i.e. candidate identity is the shared prefix rather than each service's distinct sdkId/model. These operations belong to the Kinesis Video control plane (sdkId 'Kinesis Video'), which IS bootstrapped and answering -- this is a routing fidelity defect, not the separate pending bootstrap of the Kinesis Video Signaling data plane.

Smithy (services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930): endpointPrefix/sigv4Name 'kinesisvideo', protocol restJson1; CreateSignalingChannel POST /createSignalingChannel (code 200), DescribeSignalingChannel POST /describeSignalingChannel (200), ListSignalingChannels POST /listSignalingChannels (200, paginated), GetSignalingChannelEndpoint POST /getSignalingChannelEndpoint (200), DescribeMediaStorageConfiguration POST /describeMediaStorageConfiguration (200), ListTagsForResource POST /ListTagsForResource (200). Routes are disjoint from the other four same-prefix services. 'AmbiguousService' is defined in no kinesisvideo shape; modeled errors here are AccessDeniedException 401, AccountChannelLimitExceededException 400, ClientLimitExceededException 400, InvalidArgumentException 400, ResourceInUseException 400, ResourceNotFoundException 404, TagsPerResourceExceededLimitException 400.

Same-origin wire defect on every response: the error envelope is awsJson1.1 ({"__type": ...}, Content-Type 'application/x-amz-json-1.1') while kinesisvideo is restJson1, where AWS returns Content-Type 'application/json' with x-amzn-errortype naming a modeled error. Recorded messages are EXAMPLES; status code, error code and content type are the requirements.

Why (all 9 causes): no WebRTC signaling channel can be created, described, listed or endpoint-resolved, so the whole WebRTC flow is unreachable -- a client cannot even obtain the signaling endpoints it must connect to. The 500 carries an error code in no AWS model, so SDK error handling, retry policy and exception-type matching all misbehave, and fixture polls for ChannelInfo.ChannelStatus ACTIVE never terminate.

Fix (all 9 causes): the front-door service-resolution/routing layer -- disambiguate same-prefix restJson1 candidates by matching the request's HTTP method and URI against each candidate's @http bindings, dispatch to the single route owner, and reject only when no candidate route matches; the generic error serializer must also emit restJson1 errors for this service. The signaling-channel handlers and state are never reached today.

Cause 1 of 9 -- CreateSignalingChannel setup. AWS: 200 with ChannelARN (Smithy: ChannelName required, @length 1..256, @pattern ^[a-zA-Z0-9_.-]+$). Emulator: 500 AmbiguousService. Traces: kinesis-video-signaling-channel-tag-resource-surface#setup#1 (cluster also covers kinesis-video-signaling-channel-ttl-update-version-guard#setup#1).

Cause 2 of 9 -- ListTagsForResource for a channel ARN that is not found. AWS: 404 ResourceNotFoundException (Smithy: ListTagsForResource errors include ResourceNotFoundException @httpError 404; recorded Message example: 'The requested channel is not found or not active.'). Emulator: 500 AmbiguousService, Message absent. Traces: kinesis-video-signaling-channel-tag-resource-surface#task#8 (also kinesis-video-stream-arn-tag-resource-surface#task#8).

Cause 3 of 9 -- DescribeSignalingChannel ACTIVE read-back. AWS: 200 with ChannelInfo (ChannelARN, ChannelName, ChannelStatus ACTIVE, ChannelType SINGLE_MASTER, SingleMasterConfiguration.MessageTtlSeconds, Version). Emulator: 500 AmbiguousService, ChannelInfo missing, so the poll for ChannelInfo.ChannelStatus in ['ACTIVE'] can never be satisfied. Traces: kinesis-video-signaling-channel-ttl-update-version-guard#task#2 (cluster also covers that fixture's task#4 and kinesisvideo-signaling-channel-lifecycle#task#2/#task#4).

Cause 4 of 9 -- CreateSignalingChannel asserting the returned ARN shape. AWS: 200 with ChannelARN matching ^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$. Emulator: 500 AmbiguousService, ChannelARN missing. Traces: kinesisvideo-signaling-channel-endpoints#task#1 (also kinesisvideo-signaling-channel-lifecycle#task#1).

Cause 5 of 9 -- GetSignalingChannelEndpoint for a channel that does not exist. AWS: 404 ResourceNotFoundException (recorded Message example: 'The requested channel is not found or not active.'). Emulator: 500 AmbiguousService. Traces: kinesisvideo-signaling-channel-endpoints#task#8.

Cause 6 of 9 -- DescribeMediaStorageConfiguration on a channel with no media storage. AWS: 200 with MediaStorageConfiguration {Status DISABLED, StreamARN null}. Emulator: 500 AmbiguousService, MediaStorageConfiguration missing. Traces: kinesisvideo-signaling-channel-endpoints#task#9.

Cause 7 of 9 -- CreateSignalingChannel with an existing channel name. AWS: 400 ResourceInUseException (Smithy: ResourceInUseException @httpError 400; recorded Message example: 'The channel <name> already exists.'). Emulator: 500 AmbiguousService. Traces: kinesisvideo-signaling-channel-lifecycle#task#7.

Cause 8 of 9 -- DescribeSignalingChannel after the channel is deleted. AWS: 404 ResourceNotFoundException (recorded Message example: 'The requested channel is not found or not active.'). Emulator: 500 AmbiguousService. Traces: kinesisvideo-signaling-channel-lifecycle#task#8.

Cause 9 of 9 -- ListSignalingChannels. AWS: 200 with ChannelInfoList entries (ChannelStatus ACTIVE, SingleMasterConfiguration, Version). Emulator: 500 AmbiguousService, ChannelInfoList missing. Traces: kinesisvideo-signaling-channel-lifecycle#task#9.

Related open issues (neither covers these steps): 20261022T145739Z-kinesis-video-media-router-rejects-every-kinesisvideo-prefix-request-with-ambiguousservice.md reports the same router fault from the behavioral corpus with no wire fixture/step linkage; 20261021T214806Z-bootstrap-service-com-amazonaws-kinesisvideosignaling-awsacuitysignalingservice.md is the separate bootstrap gap of the Kinesis Video Signaling data plane, whereas every operation here belongs to the already-bootstrapped kinesisvideo control plane.

## Evidence
Replay summary (simparity-wire-aws-v1): emulator_up=true, bootstrap_signal=false, state_reset_failures=0, steps_total=147, steps_passed=0, steps_mismatched=87, steps_blocked=60, score_pct=0.0; all 43 divergences on 'kinesisvideo'.

Expected (AWS / recorded): CreateSignalingChannel -> 200 {"ChannelARN": "arn:aws:kinesisvideo:us-west-2:000000000000:channel/<name>/<ts>"}; DescribeSignalingChannel -> 200 {"ChannelInfo": {... "ChannelStatus": "ACTIVE", "ChannelType": "SINGLE_MASTER", "SingleMasterConfiguration": {"MessageTtlSeconds": 60} ...}}; ListSignalingChannels -> 200 {"ChannelInfoList": [...]}; DescribeMediaStorageConfiguration -> 200 {"MediaStorageConfiguration": {"Status": "DISABLED", "StreamARN": null}}; CreateSignalingChannel on an existing name -> 400 ResourceInUseException; DescribeSignalingChannel / GetSignalingChannelEndpoint / ListTagsForResource for a missing channel -> 404 ResourceNotFoundException. All Content-Type 'application/json'.

Actual (emulator): HTTP 500 on every step, body {"__type": "AmbiguousService", "message": "5 emulated services all claim requests for 'kinesisvideo': kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo; the request is rejected rather than served by an arbitrary one"} (example message), x-amzn-errortype: AmbiguousService, Content-Type 'application/x-amz-json-1.1'. Cluster details include 'status: expected 200, got 500', 'status: expected 400, got 500', 'status: expected 404, got 500', '$headers.content-type: value_mismatch (expected application/json, got application/x-amz-json-1.1)' and 'poll ended: 500 AmbiguousService cannot satisfy until $.ChannelInfo.ChannelStatus in [ACTIVE] after 1 attempt(s)'.

## Steps to Reproduce
1. Boot the emulator built from the checkout.
2. Replay fixture kinesis-video-signaling-channel-tag-resource-surface, step setup#1 (CreateSignalingChannel) -- or call botocore kinesisvideo.create_signaling_channel(ChannelName='kv-chan-probe') against the emulator.
3. Observe HTTP 500 Error.Code 'AmbiguousService' with Content-Type application/x-amz-json-1.1 instead of 200 + ChannelARN with Content-Type application/json.
4. Repeat for every bundled cause: kinesis-video-signaling-channel-tag-resource-surface#task#8, kinesis-video-signaling-channel-ttl-update-version-guard#task#2, kinesisvideo-signaling-channel-endpoints#task#1/#task#8/#task#9, kinesisvideo-signaling-channel-lifecycle#task#7/#task#8/#task#9. Each must return its modeled status (200/400/404) and modeled body or error code before closing.
5. Re-run the corpus and confirm the signaling-channel fixtures pass.
