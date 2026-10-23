Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis-video-media.get-media-after-fragment-number-non-numeric-rejected kinesis-video-media.get-media-by-stream-arn-succeeds kinesis-video-media.get-media-concurrent-connection-limit-enforced kinesis-video-media.get-media-continuation-token-invalid-rejected kinesis-video-media.get-media-continuation-token-selector-missing-token-rejected kinesis-video-media.get-media-deleted-stream-rejected kinesis-video-media.get-media-earliest-selector-returns-stream kinesis-video-media.get-media-fragment-number-missing-after-rejected kinesis-video-media.get-media-malformed-stream-arn-rejected kinesis-video-media.get-media-malformed-stream-name-rejected kinesis-video-media.get-media-nonexistent-stream-rejected kinesis-video-media.get-media-now-selector-succeeds kinesis-video-media.get-media-producer-timestamp-missing-start-rejected kinesis-video-media.get-media-server-timestamp-missing-start-rejected kinesis-video-media.get-media-unauthorized-role-denied-rejected
Issue-Shape-Id: com.amazonaws.kinesisvideomedia#AWSAcuityInletService
Issue-Key: kinesis-video-media|CreateStream|AmbiguousService
Issue-Commit: dd79a542b09a035cf711a2d0c52670e9a3e4e70a

# Router rejects every kinesisvideo-prefix request with AmbiguousService: the 5 services sharing endpointPrefix 'kinesisvideo' are not disambiguated by URI path

> _Note: this issue was identified against the code base at commit `dd79a542b09a035cf711a2d0c52670e9a3e4e70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStream

## Description
Cause 1 of 1: The emulator's front-door request router dispatches a restJson1 request to a service crate using only the SigV4 credential-scope service name / endpoint prefix. Five emulated services legitimately share the prefix 'kinesisvideo', so the router finds five candidates, cannot pick one, and rejects the request with a non-AWS error code AmbiguousService instead of serving it. This makes the entire Kinesis Video family (control plane AND the kinesis-video-media data plane) unreachable: every one of the 15 kinesis-video-media checks dies on its very first setup call, CreateStream, so GetMedia is never even exercised.

AWS: In AWS these services deliberately share one endpoint prefix and one SigV4 signing name, and are disambiguated by HTTP method + request URI, as the models declare. Smithy: service-com-amazonaws-kinesisvideo-kinesisvideo-20170930 — awsService.endpointPrefix = 'kinesisvideo', sigv4Name = 'kinesisvideo', protocol restJson1; CreateStream @http {method: POST, uri: /createStream, code: 200}, DescribeStream POST /describeStream, GetDataEndpoint POST /getDataEndpoint. Smithy: service-com-amazonaws-kinesisvideomedia-awsacuityinletservice (sdkId 'Kinesis Video Media') — awsService.endpointPrefix = 'kinesisvideo', sigv4Name = 'kinesisvideo', its single bound operation GetMedia @http {method: POST, uri: /getMedia}. The same shared prefix/signing name holds for kinesisvideoarchivedmedia, kinesisvideowebrtcstorage and kinesisvideosignaling. Because the URI paths are disjoint across these five services, POST /createStream is unambiguously KinesisVideo_20170930.CreateStream and POST /getMedia is unambiguously AWSAcuityInletService.GetMedia. AWS returns HTTP 200 with StreamARN for a valid CreateStream and never emits an error code named AmbiguousService — that code is not in any of these models (CreateStream models only AccountStreamLimitExceededException, ClientLimitExceededException, DeviceStreamLimitExceededException, InvalidArgumentException, InvalidDeviceException, ResourceInUseException, TagsPerResourceExceededLimitException, all httpError 400).

Emulator: CreateStream against the emulator fails with a ClientError whose code is AmbiguousService and whose message is e.g. "5 emulated services all claim requests for 'kinesisvideo': kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo; the request is rejected rather than served by an arbitrary one" (message quoted as an example only). It is returned for every attempt and is retried to exhaustion by botocore ("reached max retries: 3"), so it is surfaced as a terminal, retryable-looking failure. Note the candidate list prints the same token five times, so the router's candidate identity is the shared prefix rather than each service's distinct sdkId/model.

Why: A client cannot create, describe or read a Kinesis Video stream at all. No kinesisvideo-family operation is reachable, the control plane needed to obtain a data endpoint (GetDataEndpoint) cannot be called, and the failure is an unmodeled error code so SDK error handling, retry policy and exception-type matching all behave wrongly — callers see an unknown retryable server-side condition instead of a successful 200 or a modeled 4xx.

Fix: the front-door service-resolution/routing layer that maps an inbound signed request to a registered service crate. Candidate selection must not stop at the endpoint prefix / sigv4 signing name: when several registered restJson1 services share a prefix, the router must then match the request's HTTP method and URI against each candidate's @http bindings (and fall back to the distinct sdkId/model identity) and dispatch to the single service that owns that route, rejecting only when no candidate route matches. AmbiguousService must never be emitted for the kinesisvideo prefix, because the five services' URI spaces are disjoint.

Traces: all 15 kinesis-video-media checks fail identically on the CreateStream setup call — get-media-after-fragment-number-non-numeric-rejected, get-media-by-stream-arn-succeeds, get-media-concurrent-connection-limit-enforced, get-media-continuation-token-invalid-rejected, get-media-continuation-token-selector-missing-token-rejected, get-media-deleted-stream-rejected, get-media-earliest-selector-returns-stream, get-media-fragment-number-missing-after-rejected, get-media-malformed-stream-arn-rejected, get-media-malformed-stream-name-rejected, get-media-nonexistent-stream-rejected, get-media-now-selector-succeeds, get-media-producer-timestamp-missing-start-rejected, get-media-server-timestamp-missing-start-rejected, get-media-unauthorized-role-denied-rejected.

Note: no open issue covers this. The only related open issue is 20261021T214806Z-bootstrap-service-com-amazonaws-kinesisvideosignaling-awsacuitysignalingservice.md (bootstrap of Kinesis Video Signaling); that is a separate bootstrap gap, and both kinesisvideo and kinesisvideomedia are already bootstrapped and answering here.

## Evidence
Expected (AWS): botocore `kinesisvideo.create_stream(StreamName=..., DataRetentionInHours=1, Tags={...})` -> HTTP 200 with {"StreamARN": "arn:aws:kinesisvideo:<region>:<account>:stream/<name>/<ts>"}; then `kinesisvideo.get_data_endpoint(APIName='GET_MEDIA', StreamName=...)` -> 200 with DataEndpoint; then `kinesis-video-media.get_media(...)` -> 200 streaming Payload. Error code AmbiguousService does not exist in the AWS API surface; CreateStream's modeled errors are all HTTP 400 client errors (ResourceInUseException, InvalidArgumentException, ...).

Actual (emulator): every CreateStream call raises botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateStream operation (reached max retries: 3): 5 emulated services all claim requests for 'kinesisvideo': kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo, kinesisvideo; the request is rejected rather than served by an arbitrary one.

Suite result: kinesis-video-media 0/15 (0%), 15 errored, 0 mismatched — all 15 with the identical AmbiguousService ClientError on the CreateStream setup call, so GetMedia itself was never reached.

Model evidence for the shared prefix being legitimate and the routes being disjoint: kinesisvideo endpointPrefix/sigv4Name = 'kinesisvideo' with POST /createStream, /describeStream, /getDataEndpoint; kinesisvideomedia endpointPrefix/sigv4Name = 'kinesisvideo' with POST /getMedia.

## Steps to Reproduce
1. Boot the emulator from the checkout (as the behavioral suite does).
2. Create a botocore client for 'kinesisvideo' pointed at the emulator endpoint.
3. Call create_stream(StreamName='kv-probe-1', DataRetentionInHours=1).
4. Observe ClientError with Error.Code == 'AmbiguousService' instead of a 200 response carrying StreamARN. Any kinesisvideo-prefix call (describe_stream, get_data_endpoint, and POST /getMedia on the media data plane) fails the same way.
5. Equivalently, run any kinesis-video-media behavioral check, e.g. kinesis-video-media.get-media-earliest-selector-returns-stream; it errors in its _create_active_stream setup helper.
