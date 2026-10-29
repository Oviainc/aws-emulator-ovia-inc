Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-media-get-media-before-endpoint-discovery
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-media-get-media-before-endpoint-discovery
Issue-Shape-Id: com.amazonaws.kinesisvideomedia#AWSAcuityInletService
Issue-Key: kinesisvideo|GetMedia|4xx->4xx|$.Output
Issue-Key: kinesisvideo|GetMedia|4xx->4xx|$.Version
Issue-Key: kinesisvideo|GetMedia|4xx->4xx|status
Issue-Commit: a8a8dc0cc1ff1737eab503998484303fbbf09d5b

# kinesisvideo front door answers an unroutable operation with a bare 404 instead of 400 UnknownOperationException

> _Note: this issue was identified against the code base at commit `a8a8dc0cc1ff1737eab503998484303fbbf09d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMedia

## Description
Cause 1 of 1: a request for an operation the kinesisvideo control-plane endpoint does not host (here POST /getMedia, sent before the caller resolved the media data endpoint with GetDataEndpoint) is answered by the emulator with a bare HTTP 404 and no body, where AWS answers HTTP 400 with the front door's unknown-operation envelope.

AWS: recorded behavior only — the kinesisvideo Smithy model (services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930, service KinesisVideo_20170930, sdkId 'Kinesis Video', endpointPrefix/sigv4Name 'kinesisvideo', protocol restJson1, 32 bound operations) binds no /getMedia route and models no UnknownOperationException; GetMedia is bound only on the Kinesis Video Media service (sdkId 'Kinesis Video Media', POST /getMedia), which is reachable only at the endpoint GetDataEndpoint hands back. The recording shows the real kinesisvideo front door rejecting the unroutable request before any operation handler with HTTP 400 and the body {"Output":{"__type":"com.amazon.kinesis.video.v20170930#UnknownOperationException","Message":"Unable to determine operation name"},"Version":"1.0"}. Fixed text in that body: the wrapper members Output and Version, the Version value '1.0', the __type value 'com.amazon.kinesis.video.v20170930#UnknownOperationException' and the message 'Unable to determine operation name'. Nothing in it is taken from the request or from emulator state — this error class carries no variable part (no operation name, path or request id is interpolated), so the whole body is the template.

Emulator: the step is answered with HTTP 404 and a response body that carries neither $.Output nor $.Version (replay reports '$.Output: missing', '$.Version: missing'), i.e. the generic no-such-route reply of the shared front door rather than the service's unknown-operation fault.

Why: a client that reaches the control-plane endpoint for a data-plane operation is told the wrong thing twice. The status is 404 rather than 400, so SDK/retry logic that classifies 4xx faults by status plus __type sees an unmodelled, type-less failure; and because the body is empty there is no __type at all, so a caller cannot distinguish 'this endpoint does not serve this operation, go call GetDataEndpoint' from kinesisvideo's own 404 ResourceNotFoundException about a missing stream. Endpoint-discovery flows that key on the UnknownOperationException fault to re-resolve the data endpoint therefore never re-resolve, and error logs lose the fault type entirely.

Fix: the kinesisvideo service's request routing / front-door fallback — the path that decides what to answer when an incoming method+URI matches no operation bound to the resolved service. It must emit HTTP 400 with the restJson1 unknown-operation envelope above (Output.__type + Output.Message, Version '1.0') instead of falling through to a bodyless 404. Note the open issue 20261022T180000Z-kinesis-video-media-getmedia-cannot-see-a-stream-createstream-made.md is about a DIFFERENT cause (the split stream registries making GetMedia 404 a stream CreateStream made, on the media data plane); it does not cover this unroutable-operation reply on the control-plane endpoint and does not carry this fixture step.

Traces: kinesis-video-media-get-media-before-endpoint-discovery#task#1.

## Evidence
Replay step kinesis-video-media-get-media-before-endpoint-discovery / task#1, service kinesisvideo, operation GetMedia, verdict mismatch.

Expected (recorded AWS): status 400, body {"Output": {"__type": "com.amazon.kinesis.video.v20170930#UnknownOperationException", "Message": "Unable to determine operation name"}, "Version": "1.0"}.

Actual (emulator): status 404; $.Output missing (expected the UnknownOperationException object, got None); $.Version missing (expected '1.0', got None).

Divergence detail verbatim: "status: expected 400, got 404 / $.Output: missing (expected {'__type': 'com.amazon.kinesis.video.v20170930#UnknownOperationException', 'Message': 'Unable to determine operation name'}, got None) / $.Version: missing (expected '1.0', got None)". Divergence key: service=kinesisvideo|GetMedia|status=400->404|keys=$.Output,$.Version,status.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Without calling kinesisvideo:GetDataEndpoint first, send the Kinesis Video Media GetMedia request to the kinesisvideo control-plane endpoint: POST /getMedia with a sigv4 'kinesisvideo' signature and a JSON body naming a StreamName and a StartSelector.
3. Observe HTTP 404 with no Output/Version body.
4. AWS answers HTTP 400 with {"Output":{"__type":"com.amazon.kinesis.video.v20170930#UnknownOperationException","Message":"Unable to determine operation name"},"Version":"1.0"}.
5. Equivalent: POST any other URI the kinesisvideo service does not bind to the same endpoint — the reply must be the same 400 unknown-operation envelope, not a bodyless 404.
