Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-stream-arn-tag-resource-surface
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-stream-arn-tag-resource-surface kinesis-video-stream-configuration-defaults kinesis-video-stream-data-retention-version-guard kinesis-video-stream-get-data-endpoint kinesis-video-stream-lifecycle-create-describe-delete kinesis-video-stream-tags-lifecycle kinesisvideo-get-data-endpoint kinesisvideo-stream-data-retention kinesisvideo-stream-lifecycle kinesisvideo-stream-listing-and-unconfigured-reads kinesisvideo-stream-tags
Issue-Shape-Id: com.amazonaws.kinesisvideo#KinesisVideo_20170930
Issue-Key: kinesisvideo|CreateStream|2xx->5xx|status
Issue-Key: kinesisvideo|CreateStream|2xx->5xx|$.StreamARN
Issue-Key: kinesisvideo|CreateStream|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|CreateStream|4xx->5xx|$.Message
Issue-Key: kinesisvideo|CreateStream|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|CreateStream|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|CreateStream|4xx->5xx|status
Issue-Key: kinesisvideo|DescribeStream|2xx->5xx|$
Issue-Key: kinesisvideo|DescribeStream|2xx->5xx|$.StreamInfo
Issue-Key: kinesisvideo|DescribeStream|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|DescribeStream|2xx->5xx|status
Issue-Key: kinesisvideo|DescribeStream|4xx->5xx|$.Message
Issue-Key: kinesisvideo|DescribeStream|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|DescribeStream|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|DescribeStream|4xx->5xx|status
Issue-Key: kinesisvideo|DescribeStream|4xx->5xx|$
Issue-Key: kinesisvideo|DescribeStream|4xx->5xx|$.message
Issue-Key: kinesisvideo|ListStreams|2xx->5xx|$.StreamInfoList
Issue-Key: kinesisvideo|ListStreams|2xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|ListStreams|2xx->5xx|status
Issue-Key: kinesisvideo|ListStreams|4xx->5xx|$.Message
Issue-Key: kinesisvideo|ListStreams|4xx->5xx|$headers.content-type
Issue-Key: kinesisvideo|ListStreams|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|ListStreams|4xx->5xx|status
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# kinesisvideo stream lifecycle wire steps all return AmbiguousService 500

## Resolution

FIXED by the request-claim rule, which each of the five services that share the SigV4
signing name AND the endpointPrefix `kinesisvideo` (Kinesis Video, Kinesis Video Media,
Kinesis Video Archived Media, Kinesis Video WebRTC Storage, Kinesis Video Signaling) now
applies in its `can_handle`: the request's resolved routing key must name this endpoint AND
the request must address this service's OWN modelled URI path space — the framework's
`emulator_server::can_handle_based_on_route_key_and_path` over each crate's generated
`ROUTE_PATH_SEGMENTS`, matched by exact segment equality — instead of claiming every
`kinesisvideo`-signed request on the signing name alone. The five models' path spaces are
disjoint at the first segment, so exactly one service claims each request, the router no
longer refuses any of them as `500 AmbiguousService`, and every answer is rendered in the
owning service's own `restJson1` envelope (`application/json`, with the modelled code in
`x-amzn-errortype`) rather than the router's `awsJson1.1` one. That claim conjunct landed
on mainline from the sibling report
`20261022T145739Z-kinesis-video-media-router-rejects-every-kinesisvideo-prefix-request-with-ambiguousservice.md`,
which is the same emulator defect seen from the behavioural corpus; this change keeps it and
adds the coverage the wire causes above need, in
`core/router/tests/all/kinesisvideo_signing_name_routing.rs`: that every answer from the
assembled router carries the `restJson1` envelope (over a success, a handler's modelled 404,
a boundary refusal and a scaffolded operation's 501), and that a stream created through the
shared endpoint reads back BY THE `StreamARN` IT RETURNED through `DescribeStream` and
`ListStreams`, with that ARN checked against the modelled `ResourceARN` form.

ONE CAUSE IS DECLINED, and is recorded here so it is not lost: cause 5, `DescribeStream`
after a delete reporting `200` with `StreamInfo.Status` `DELETING`
(`kinesis-video-stream-lifecycle-create-describe-delete#task#7`). Its `500 AmbiguousService`
is gone, but that step now answers `404 ResourceNotFoundException`, because `DeleteStream`
removes the record when the call returns rather than holding it in a transient `DELETING`
status for a clock-owed span. That is `DeleteStream`'s own behaviour — not a routing
failure — and it is asserted as it stands by that crate's landed
`tests/all/stream_lifecycle.rs` (describe-after-delete is `404`, a second delete of the same
ARN is `404`, and the freed quota slot is reusable), so changing it belongs to its own issue
for the transient `DELETING` state rather than to this one.

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStream

## Description
This issue bundles 10 causes, all of the kinesisvideo stream lifecycle surface, which share ONE emulator defect. A fixer must close all 10 (every fixture#step listed under each cause, plus the representative in Issue-Trace/Issue-Step) before submitting.

SHARED ROOT CAUSE: the emulator's front-door request router resolves an inbound restJson1 request to a registered service crate using only the SigV4 credential-scope service name / endpoint prefix. Five registered services legitimately share endpointPrefix 'kinesisvideo' (Kinesis Video, Kinesis Video Media, Kinesis Video Archived Media, Kinesis Video WebRTC Storage, Kinesis Video Signaling), so the router finds five candidates, refuses to pick one, and answers HTTP 500 with an unmodeled error code 'AmbiguousService' instead of dispatching on HTTP method + request URI. Because the five services' URI spaces are disjoint, the prefix collision is resolvable by route. Consequence on this replay: 0 of 147 corpus steps passed (87 mismatched, 60 blocked) across all 15 kinesisvideo fixtures.

Smithy (services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930): awsService.endpointPrefix = 'kinesisvideo', sigv4Name = 'kinesisvideo', protocol restJson1. The operations below are disjoint POST routes: CreateStream POST /createStream (code 200), DescribeStream POST /describeStream (200), ListStreams POST /listStreams (200). 'AmbiguousService' appears in no kinesisvideo shape; every modeled error on these operations is a 4xx client error.

A second, same-origin wire defect rides along on every one of these responses: the router's generic error path serializes the failure in awsJson1.1 form -- body {"__type": "AmbiguousService", "message": ...} with Content-Type 'application/x-amz-json-1.1' -- while kinesisvideo is restJson1, for which AWS returns Content-Type 'application/json' and names the modeled error in the x-amzn-errortype header. Recorded error messages below are quoted as EXAMPLES; the requirements are the status code, the error code, and the restJson1 content type.

Why (applies to all 10 causes): no kinesisvideo stream can be created, described, listed or deleted at all. Clients see HTTP 500 with an error code that exists in no AWS model, so SDK exception-type matching fails, 500 is treated as a retryable server fault and retried to exhaustion, and waiters that poll for StreamInfo.Status never terminate. Not one modeled success body or modeled 4xx is observable.

Fix (applies to all 10 causes): the front-door service-resolution/routing layer that maps a signed request to a registered service crate. Candidate selection must not stop at the endpoint prefix / sigv4 signing name: when several registered restJson1 services share a prefix, match the request's HTTP method and URI against each candidate's @http bindings and dispatch to the single owner of that route, rejecting only when no candidate route matches. The generic error serializer must also emit restJson1 errors (Content-Type application/json + x-amzn-errortype) for this service, not awsJson1.1. No per-operation handler change is expected; the operation handlers below are simply never reached.

Cause 1 of 10 -- CreateStream, success path. AWS: POST /createStream returns 200 with StreamARN matching ^arn:[a-z\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$ (Smithy: CreateStream.StreamName required, @length 1..256, @pattern ^[a-zA-Z0-9_.-]+$; errors AccountStreamLimitExceededException/ClientLimitExceededException/DeviceStreamLimitExceededException/InvalidArgumentException/InvalidDeviceException/ResourceInUseException/TagsPerResourceExceededLimitException, all httpError 400). Emulator: 500, error code AmbiguousService, no StreamARN. Traces: kinesis-video-stream-arn-tag-resource-surface#setup#1 (cluster also covers 6 fixtures' setup steps, incl. kinesis-video-stream-configuration-defaults, -data-retention-version-guard, -get-data-endpoint, -lifecycle-create-describe-delete, -tags-lifecycle).

Cause 2 of 10 -- CreateStream, success path asserting the ARN shape. AWS: 200 with StreamARN of the modeled ARN pattern. Emulator: 500 AmbiguousService, StreamARN missing. Traces: kinesisvideo-get-data-endpoint#task#1 (cluster covers 6 steps across kinesisvideo-stream-data-retention, -stream-lifecycle, -stream-listing-and-unconfigured-reads, -stream-tags).

Cause 3 of 10 -- CreateStream, duplicate-name rejection. AWS: recreating an existing stream is ResourceInUseException, httpError 400 (recorded Message example: 'The stream <name> already exists.'). Emulator: 500 AmbiguousService. Traces: kinesis-video-stream-lifecycle-create-describe-delete#task#5 (also kinesisvideo-stream-lifecycle#task#3).

Cause 4 of 10 -- DescribeStream, ACTIVE read-back. AWS: 200 with StreamInfo (Status ACTIVE, DataRetentionInHours, KmsKeyId default alias/aws/kinesisvideo, StreamARN, Version). Emulator: 500 AmbiguousService, StreamInfo missing; the fixture's poll for StreamInfo.Status in ['ACTIVE'] can never be satisfied. Traces: kinesis-video-stream-data-retention-version-guard#task#2 (cluster covers 8 steps incl. kinesisvideo-stream-data-retention#task#2/#4/#8, kinesisvideo-stream-lifecycle#task#2).

Cause 5 of 10 -- DescribeStream after delete, DELETING state. AWS: 200 with StreamInfo.Status 'DELETING'. Emulator: 500 AmbiguousService. Traces: kinesis-video-stream-lifecycle-create-describe-delete#task#7.

Cause 6 of 10 -- DescribeStream for an unknown stream. AWS: ResourceNotFoundException, httpError 404 (Smithy: DescribeStream errors include ResourceNotFoundException @httpError 404; recorded Message example: 'The requested stream is not found or not active.'). Emulator: 500 AmbiguousService. Traces: kinesis-video-stream-lifecycle-create-describe-delete#task#4 (also kinesisvideo-stream-lifecycle#task#4).

Cause 7 of 10 -- DescribeStream polled until the stream is gone. AWS: 404 ResourceNotFoundException, which is the terminal condition the fixture waits on. Emulator: 500 AmbiguousService, so until_error ['ResourceNotFoundException'] is never reached. Traces: kinesis-video-stream-lifecycle-create-describe-delete#task#9.

Cause 8 of 10 -- DescribeStream with a malformed StreamARN. AWS: 400 InvalidArgumentException; the rule is the modeled member constraint, so the message is built from the request (recorded example: "1 validation error detected: Value at 'streamARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+"). Emulator: 500 AmbiguousService with the router's message. Traces: kinesisvideo-stream-lifecycle#task#6.

Cause 9 of 10 -- ListStreams. AWS: 200 with StreamInfoList (Smithy: ListStreams POST /listStreams, paginated NextToken/StreamInfoList). Emulator: 500 AmbiguousService, StreamInfoList missing. Traces: kinesis-video-stream-lifecycle-create-describe-delete#task#3.

Cause 10 of 10 -- ListStreams with an invalid StreamNameCondition.ComparisonOperator. AWS: 400 InvalidArgumentException naming the enum constraint (recorded example: "1 validation error detected: Value at 'streamNameCondition.comparisonOperator' failed to satisfy constraint: Member must satisfy enum value set: [BEGINS_WITH]"). Emulator: 500 AmbiguousService. Traces: kinesisvideo-stream-listing-and-unconfigured-reads#task#7.

Related open issue (does NOT cover these steps): 20261022T145739Z-kinesis-video-media-router-rejects-every-kinesisvideo-prefix-request-with-ambiguousservice.md reports the same router fault from the BEHAVIORAL corpus (kinesis-video-media checks) and carries no wire fixture/step linkage, so none of the clusters here are owned by it.

## Evidence
Replay summary (trace set simparity-wire-aws-v1): emulator_up=true, bootstrap_signal=false, state_reset_failures=0, steps_total=147, steps_passed=0, steps_mismatched=87, steps_blocked=60, score_pct=0.0, divergence_count=43 -- every divergence on service 'kinesisvideo'.

Expected (AWS / recorded): CreateStream -> 200 {"StreamARN": "arn:aws:kinesisvideo:us-west-2:000000000000:stream/<name>/<ts>"}; DescribeStream -> 200 {"StreamInfo": {... "Status": "ACTIVE" ...}}; DescribeStream on a deleted stream -> 404 ResourceNotFoundException; CreateStream on an existing name -> 400 ResourceInUseException; ListStreams -> 200 {"StreamInfoList": [...]}; malformed streamARN / bad comparisonOperator -> 400 InvalidArgumentException. All with Content-Type 'application/json' and, on errors, x-amzn-errortype naming the modeled error.

Actual (emulator): HTTP 500 on every step, body {"__type": "AmbiguousService", "message": "5 emulated services all claim requests for 'kinesisvideo': kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo; the request is rejected rather than served by an arbitrary one"} (message quoted as an example), header x-amzn-errortype: AmbiguousService, Content-Type 'application/x-amz-json-1.1'. Per-cluster detail lines: 'status: expected 200, got 500', 'status: expected 404, got 500', 'status: expected 400, got 500', '$headers.content-type: value_mismatch (expected application/json, got application/x-amz-json-1.1)', and polls reported as 'poll ended: 500 AmbiguousService cannot satisfy until $.StreamInfo.Status in [ACTIVE] after 1 attempt(s)'.

Note the candidate list prints the token 'kinesisvideo' five times, i.e. the router's candidate identity is the shared prefix, not each service's distinct sdkId/model.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from the checkout (admin /__admin/health answers 200 and the data listener accepts, so this is not a bootstrap gap).
2. Replay fixture kinesis-video-stream-arn-tag-resource-surface, step setup#1 (CreateStream) -- or point a botocore 'kinesisvideo' client at the emulator endpoint and call create_stream(StreamName='kv-probe-1', DataRetentionInHours=1).
3. Observe HTTP 500 with Error.Code 'AmbiguousService' and Content-Type application/x-amz-json-1.1 instead of 200 + StreamARN with Content-Type application/json.
4. Repeat for each bundled cause: kinesisvideo-get-data-endpoint#task#1, kinesis-video-stream-lifecycle-create-describe-delete#task#5/#task#4/#task#7/#task#9/#task#3, kinesis-video-stream-data-retention-version-guard#task#2, kinesisvideo-stream-lifecycle#task#6, kinesisvideo-stream-listing-and-unconfigured-reads#task#7. Each must return its modeled status (200/400/404) and a modeled error code before this issue is closed.
5. Re-run the whole corpus: all 15 kinesisvideo fixtures currently fail, so a fix should move score_pct off 0.0.
