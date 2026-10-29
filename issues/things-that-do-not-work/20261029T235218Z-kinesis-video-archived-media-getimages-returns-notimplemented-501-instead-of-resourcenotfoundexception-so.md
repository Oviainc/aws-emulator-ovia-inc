Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis-video-archived-media.concurrent-get-images-returns-consistent-results
Issue-Shape-Id: com.amazonaws.kinesisvideoarchivedmedia#AWSAcuityReader
Issue-Key: kinesis-video-archived-media.concurrent-get-images-returns-consistent-results|run#13|AssertionError|assert first == "ResourceNotFoundException", (
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# GetImages returns NotImplemented (501) instead of ResourceNotFoundException, so concurrent identical reads never reach the deterministic fragment-absence refusal

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetImages

## Description
Cause 1 of 1: GetImages on a retention-enabled stream that has never ingested media is answered with the emulator's scaffold refusal instead of the modeled fragment-absence error, and that refusal is what every one of N concurrent identical GetImages calls receives.

Related open issue: 20261029T202349Z-kinesis-video-archived-media-getimages-stub.md names the same handler, but its Issue-Trace-Ids cover only kinesis-video-archived-media.get-images-no-fragments-not-found, .get-images-reversed-timestamp-range-rejected, .get-images-timestamp-window-exceeds-300-seconds-rejected and .get-images-width-height-pixels-accepted-on-empty-stream — it does NOT carry the concurrency check's replay key, so this cause is filed separately and must be re-replayed against the check id listed below once GetImages is implemented.

AWS: Smithy (com.amazonaws.kinesisvideoarchivedmedia#AWSAcuityReader) models GetImages as POST /getImages with success code 200 and client errors ClientLimitExceededException (400), InvalidArgumentException (400), NoDataRetentionException (400), NotAuthorizedException (401) and ResourceNotFoundException (404). For a stream whose retention is enabled but which holds no fragments in the requested [StartTimestamp, EndTimestamp] window, AWS refuses the call wholesale with HTTP 404, error code 'ResourceNotFoundException' and message 'No fragments found in the stream for the GetImages request.' — the whole message is fixed text for this error class for GetImages (the per-operation phrase 'for the GetImages request' is the only part that varies by operation; it carries no request- or state-derived values). It does NOT return 200 with per-image NO_MEDIA markers. Because that refusal is a pure function of stream state and the requested window, repeating the identical call concurrently must yield the identical error class every time.

Emulator: every GetImages call is refused with the scaffold stub's HTTP 501, error code 'NotImplemented' and message 'operation GetImages is scaffolded but not implemented in this emulator'. All 8 concurrent calls agree with each other, so the determinism part of the check holds, but the agreed-on answer is the wrong status, the wrong code and an emulator-authored message that is not an AWS error at all. A positive control in the same check — ListFragments on the same stream via the same data-plane endpoint — returns 200 with an empty Fragments list, so the stream, the endpoint and the data-plane wiring are fine; only GetImages is unserved.

Why: a client that catches ResourceNotFoundException to mean 'nothing recorded in this window' (and retries or falls back accordingly) instead sees a 501 with an unmodeled code. botocore surfaces it as a generic ClientError that no GetImages-specific except-clause matches, and 501 is commonly treated as a non-retryable server/endpoint fault, so callers conclude the service is broken rather than that the window is empty. Any concurrency or load test built on GetImages also cannot distinguish 'consistently correct' from 'consistently unimplemented'.

Fix: the GetImages handler, services/service-com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader/src/ops/get_images.rs — it must look up the stream's fragments for the requested selector/window in the service state and, when none exist, return ResourceNotFoundException (404) with the AWS message above, rather than the scaffold's not_implemented refusal. The handler must be free of shared-state races so that concurrent identical calls stay identical once it is implemented.

Traces: kinesis-video-archived-media.concurrent-get-images-returns-consistent-results.

## Evidence
Check kinesis-video-archived-media.concurrent-get-images-returns-consistent-results, [mismatch]:

  AssertionError: expected all concurrent calls to consistently raise ResourceNotFoundException, got 'NotImplemented'

Expected (AWS): HTTP 404, Error.Code 'ResourceNotFoundException', Error.Message 'No fragments found in the stream for the GetImages request.' — returned identically to each of the 8 concurrent identical GetImages calls.
Actual (emulator): HTTP 501, Error.Code 'NotImplemented', Error.Message 'operation GetImages is scaffolded but not implemented in this emulator' — returned identically to all 8 calls.

Positive control in the same check passed: ListFragments (SERVER_TIMESTAMP selector over the same window, same stream, same data-plane endpoint) returned 200 with Fragments == [], proving the stream existed, was ACTIVE, had retention enabled and the data-plane endpoint resolved — the divergence is specific to GetImages.

Smithy evidence: GetImages errors = [ClientLimitExceededException 400, InvalidArgumentException 400, NoDataRetentionException 400, NotAuthorizedException 401, ResourceNotFoundException 404]; http = POST /getImages, code 200. 'NotImplemented' is not among the modeled errors and 501 is not a modeled status.

## Steps to Reproduce
1. Boot the kinesis-video-archived-media emulator (and kinesisvideo, for the control plane).
2. kinesisvideo.CreateStream with DataRetentionInHours=1 and MediaType=video/x-matroska; poll DescribeStream until StreamInfo.Status == ACTIVE. Ingest no media.
3. kinesisvideo.GetDataEndpoint(StreamName=<name>, APIName=LIST_FRAGMENTS) and call ListFragments with FragmentSelectorType=SERVER_TIMESTAMP over [now-10, now]; observe 200 with Fragments == [] (control).
4. kinesisvideo.GetDataEndpoint(StreamName=<name>, APIName=GET_IMAGES); issue 8 concurrent identical GetImages calls with ImageSelectorType=PRODUCER_TIMESTAMP, StartTimestamp=now-10, EndTimestamp=now, Format=JPEG, SamplingInterval=1000.
5. Observe every call return HTTP 501 / 'NotImplemented' instead of HTTP 404 / ResourceNotFoundException with message 'No fragments found in the stream for the GetImages request.'
