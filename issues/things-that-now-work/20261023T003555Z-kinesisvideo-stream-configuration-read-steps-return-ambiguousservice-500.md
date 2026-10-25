Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-stream-configuration-defaults
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-stream-configuration-defaults kinesisvideo-stream-listing-and-unconfigured-reads kinesisvideo-stream-lifecycle
Issue-Shape-Id: com.amazonaws.kinesisvideo#KinesisVideo_20170930
Issue-Key: kinesisvideo|DescribeStreamStorageConfiguration|2xx->5xx|$
Issue-Key: kinesisvideo|DescribeStreamStorageConfiguration|2xx->5xx|$.StreamARN
Issue-Key: kinesisvideo|DescribeStreamStorageConfiguration|2xx->5xx|$.StreamName
Issue-Key: kinesisvideo|DescribeStreamStorageConfiguration|2xx->5xx|$.StreamStorageConfiguration
Issue-Key: kinesisvideo|DescribeStreamStorageConfiguration|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|DescribeStreamStorageConfiguration|2xx->5xx|status
Issue-Key: kinesisvideo|DescribeNotificationConfiguration|2xx->5xx|$
Issue-Key: kinesisvideo|DescribeNotificationConfiguration|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|DescribeNotificationConfiguration|2xx->5xx|status
Issue-Key: kinesisvideo|DescribeImageGenerationConfiguration|2xx->5xx|$
Issue-Key: kinesisvideo|DescribeImageGenerationConfiguration|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|DescribeImageGenerationConfiguration|2xx->5xx|status
Issue-Key: kinesisvideo|DescribeMappedResourceConfiguration|2xx->5xx|$
Issue-Key: kinesisvideo|DescribeMappedResourceConfiguration|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|DescribeMappedResourceConfiguration|2xx->5xx|status
Issue-Key: kinesisvideo|DescribeEdgeConfiguration|4xx->5xx|$.Message
Issue-Key: kinesisvideo|DescribeEdgeConfiguration|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|DescribeEdgeConfiguration|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|DescribeEdgeConfiguration|4xx->5xx|status
Issue-Key: kinesisvideo|DescribeStreamStorageConfiguration|4xx->5xx|$.Message
Issue-Key: kinesisvideo|DescribeStreamStorageConfiguration|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|DescribeStreamStorageConfiguration|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|DescribeStreamStorageConfiguration|4xx->5xx|status
Issue-Key: kinesisvideo|ListStreams|2xx->5xx|$
Issue-Key: kinesisvideo|ListStreams|2xx->5xx|$.NextToken
Issue-Related: 20261023T003555Z-kinesisvideo-stream-lifecycle-wire-steps-all-return-ambiguousservice.md
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# kinesisvideo stream configuration read steps return AmbiguousService 500

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStreamStorageConfiguration

## Description
This issue bundles 10 causes, all of the kinesisvideo stream-configuration read surface, which share ONE emulator defect. A fixer must close all 10 (the representative in Issue-Trace/Issue-Step plus every fixture#step named under the causes below) before submitting.

SHARED ROOT CAUSE: the emulator's front-door router resolves a restJson1 request to a registered service crate by SigV4 credential-scope service name / endpoint prefix alone. Five registered services share endpointPrefix 'kinesisvideo', so the router finds five candidates, declines to choose, and answers HTTP 500 with the unmodeled error code 'AmbiguousService' rather than dispatching on HTTP method + URI. Since the five services' URI spaces are disjoint, each POST route below identifies exactly one service.

Smithy (services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930): endpointPrefix/sigv4Name 'kinesisvideo', protocol restJson1; DescribeStreamStorageConfiguration POST /describeStreamStorageConfiguration (code 200), DescribeNotificationConfiguration POST /describeNotificationConfiguration (200), DescribeImageGenerationConfiguration POST /describeImageGenerationConfiguration (200), DescribeMappedResourceConfiguration POST /describeMappedResourceConfiguration (200, paginated), DescribeEdgeConfiguration POST /describeEdgeConfiguration (200), ListStreams POST /listStreams (200, paginated). No kinesisvideo shape defines 'AmbiguousService'; all modeled errors on these operations are 4xx.

Same-origin wire defect on every response here: the error envelope is awsJson1.1 ({"__type": ...}, Content-Type 'application/x-amz-json-1.1') though kinesisvideo is restJson1, for which AWS returns Content-Type 'application/json' and sets x-amzn-errortype to a modeled error name. Recorded messages are quoted as EXAMPLES only; the requirements are status, error code and content type.

Why (all 10 causes): a client cannot read any stream configuration. The modeled 'unconfigured' shape of a fresh stream -- the distinction between a 200 carrying an explicit null/empty configuration and a 404 for a configuration that does not exist -- is entirely unobservable, so SDK callers that branch on it break. 500 with an error code absent from every AWS model also defeats exception-type matching and is retried as a server fault, and fixture polls waiting for a 200 never terminate.

Fix (all 10 causes): the front-door service-resolution/routing layer. When several registered restJson1 services share an endpoint prefix, candidate selection must continue by matching the request's HTTP method and URI against each candidate's @http bindings and dispatch to the single route owner, rejecting only when no candidate route matches; and the generic error serializer must emit restJson1 errors for this service. The operation handlers below are never reached today.

Cause 1 of 10 -- DescribeStreamStorageConfiguration on a fresh stream. AWS: 200 with StreamName, StreamARN and StreamStorageConfiguration (recorded default DefaultStorageTier 'HOT'). Emulator: 500 AmbiguousService, all three members missing; the fixture's until_status [200] poll cannot be satisfied. Traces: kinesis-video-stream-configuration-defaults#task#2.

Cause 2 of 10 -- DescribeNotificationConfiguration on an unconfigured stream. AWS: 200 with NotificationConfiguration absent/null. Emulator: 500 AmbiguousService body instead of the modeled 200 shape. Traces: kinesis-video-stream-configuration-defaults#task#3 (also kinesisvideo-stream-listing-and-unconfigured-reads#task#8).

Cause 3 of 10 -- DescribeImageGenerationConfiguration on an unconfigured stream. AWS: 200 with ImageGenerationConfiguration absent/null. Emulator: 500 AmbiguousService. Traces: kinesis-video-stream-configuration-defaults#task#4 (also kinesisvideo-stream-listing-and-unconfigured-reads#task#9).

Cause 4 of 10 -- DescribeMappedResourceConfiguration on an unmapped stream. AWS: 200 with an empty MappedResourceConfigurationList and no NextToken (Smithy: list @length 0..1, paginated on NextToken). Emulator: 500 AmbiguousService. Traces: kinesis-video-stream-configuration-defaults#task#5 (also kinesisvideo-stream-listing-and-unconfigured-reads#task#10).

Cause 5 of 10 -- DescribeEdgeConfiguration with no edge configuration. AWS: 404 StreamEdgeConfigurationNotFoundException (Smithy: DescribeEdgeConfiguration errors include StreamEdgeConfigurationNotFoundException @httpError 404 and ResourceNotFoundException @httpError 404; recorded Message example: 'No edge configuration exist for the stream'). Emulator: 500 AmbiguousService. Traces: kinesis-video-stream-configuration-defaults#task#6 (also kinesisvideo-stream-listing-and-unconfigured-reads#task#11).

Cause 6 of 10 -- DescribeStreamStorageConfiguration for an unknown stream. AWS: 404 ResourceNotFoundException (recorded Message example: 'The requested stream is not found or not active.'). Emulator: 500 AmbiguousService. Traces: kinesis-video-stream-configuration-defaults#task#7.

Cause 7 of 10 -- DescribeStreamStorageConfiguration read-back inside the stream lifecycle fixture. AWS: 200 with StreamName, StreamARN and StreamStorageConfiguration. Emulator: 500 AmbiguousService. Traces: kinesisvideo-stream-lifecycle#task#7.

Cause 8 of 10 -- ListStreams returning two ACTIVE streams. AWS: 200 with a StreamInfoList entry per stream, Status ACTIVE. Emulator: 500 AmbiguousService, StreamInfoList missing, so the poll for StreamInfoList[1].Status in ['ACTIVE'] never completes. Traces: kinesisvideo-stream-listing-and-unconfigured-reads#task#3.

Cause 9 of 10 -- ListStreams paginated with MaxResults. AWS: 200 with one StreamInfoList entry and a NextToken (Smithy: NextToken @length 0..1024, @pattern ^[a-zA-Z0-9+/=]*$). Emulator: 500 AmbiguousService, NextToken and StreamInfoList missing. Traces: kinesisvideo-stream-listing-and-unconfigured-reads#task#4.

Cause 10 of 10 -- ListStreams with a name condition matching nothing. AWS: 200 with an empty StreamInfoList and no NextToken. Emulator: 500 AmbiguousService. Traces: kinesisvideo-stream-listing-and-unconfigured-reads#task#6.

Related open issue (does NOT cover these steps): 20261022T145739Z-kinesis-video-media-router-rejects-every-kinesisvideo-prefix-request-with-ambiguousservice.md reports the same router fault from the behavioral corpus and carries no wire fixture/step linkage; no cluster here is owned by it.

## Evidence
Replay summary (simparity-wire-aws-v1): emulator_up=true, bootstrap_signal=false, state_reset_failures=0, steps_total=147, steps_passed=0, steps_mismatched=87, steps_blocked=60, score_pct=0.0; all 43 divergences on service 'kinesisvideo'.

Expected (AWS / recorded): DescribeStreamStorageConfiguration -> 200 {"StreamName": ..., "StreamARN": ..., "StreamStorageConfiguration": {"DefaultStorageTier": "HOT"}}; DescribeNotificationConfiguration -> 200 {"NotificationConfiguration": null}; DescribeImageGenerationConfiguration -> 200 {"ImageGenerationConfiguration": null}; DescribeMappedResourceConfiguration -> 200 {"MappedResourceConfigurationList": [], "NextToken": null}; DescribeEdgeConfiguration (none configured) -> 404 StreamEdgeConfigurationNotFoundException; DescribeStreamStorageConfiguration (unknown stream) -> 404 ResourceNotFoundException; ListStreams -> 200 with StreamInfoList / NextToken. All Content-Type 'application/json'.

Actual (emulator): HTTP 500 on all of them, body {"__type": "AmbiguousService", "message": "5 emulated services all claim requests for 'kinesisvideo': kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo; the request is rejected rather than served by an arbitrary one"} (example message), x-amzn-errortype: AmbiguousService, Content-Type 'application/x-amz-json-1.1'. Cluster detail lines include 'status: expected 200, got 500', 'status: expected 404, got 500', '$headers.content-type: value_mismatch (expected application/json, got application/x-amz-json-1.1)' and 'poll ended: 500 AmbiguousService cannot satisfy until_status [200] after 1 attempt(s)'.

## Steps to Reproduce
1. Boot the emulator built from the checkout (health endpoint answers 200; the service is bootstrapped, so this is not a bootstrap gap).
2. Replay fixture kinesis-video-stream-configuration-defaults, step task#2 (DescribeStreamStorageConfiguration) -- or call botocore kinesisvideo.describe_stream_storage_configuration(StreamName=<a created stream>) against the emulator.
3. Observe HTTP 500 Error.Code 'AmbiguousService' with Content-Type application/x-amz-json-1.1 instead of 200 + StreamStorageConfiguration with Content-Type application/json.
4. Repeat for every bundled cause: kinesis-video-stream-configuration-defaults#task#3/#task#4/#task#5/#task#6/#task#7, kinesisvideo-stream-lifecycle#task#7, kinesisvideo-stream-listing-and-unconfigured-reads#task#3/#task#4/#task#6. Each must return its modeled status (200 or 404) and modeled body/error code before closing.
5. Re-run the corpus and confirm these fixtures no longer diverge.
