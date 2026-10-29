Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisvideoarchivedmedia-get-clip-data-retention-transition
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisvideoarchivedmedia-get-clip-data-retention-transition kinesisvideoarchivedmedia-get-clip-reversed-range-rejected kinesisvideoarchivedmedia-get-dash-streaming-session-url-live-and-ondemand kinesisvideoarchivedmedia-get-media-for-fragment-list-validation kinesisvideoarchivedmedia-list-fragments-addressing-and-validation kinesisvideoarchivedmedia-get-hls-streaming-session-url-retention-transition
Issue-Shape-Id: com.amazonaws.kinesisvideoarchivedmedia#AWSAcuityReader
Issue-Key: kinesis-video-archived-media|GetClip|4xx->4xx|$.Message
Issue-Key: kinesis-video-archived-media|GetClip|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kinesis-video-archived-media|GetClip|4xx->4xx|status
Issue-Key: kinesis-video-archived-media|GetClip|4xx->4xx|$
Issue-Key: kinesis-video-archived-media|ListFragments|2xx->4xx|$.Fragments
Issue-Key: kinesis-video-archived-media|ListFragments|2xx->4xx|status
Issue-Key: kinesis-video-archived-media|GetHLSStreamingSessionURL|4xx->4xx|$.Message
Issue-Key: kinesis-video-archived-media|GetHLSStreamingSessionURL|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kinesis-video-archived-media|GetHLSStreamingSessionURL|4xx->4xx|status
Issue-Key: kinesis-video-archived-media|GetHLSStreamingSessionURL|4xx->4xx|$
Issue-Key: kinesis-video-archived-media|GetMediaForFragmentList|4xx->4xx|$.Message
Issue-Key: kinesis-video-archived-media|GetMediaForFragmentList|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kinesis-video-archived-media|GetMediaForFragmentList|4xx->4xx|status
Issue-Commit: a8a8dc0cc1ff1737eab503998484303fbbf09d5b

# Kinesis Video Archived Media reads do not resolve streams from Kinesis Video Streams state, so every archived-media call 404s ResourceNotFoundException (8 causes)

> _Note: this issue was identified against the code base at commit `a8a8dc0cc1ff1737eab503998484303fbbf09d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetClip

## Description
Shared fault: the kinesis-video-archived-media (KVAM) reader never resolves a stream that exists in the Kinesis Video Streams (kinesisvideo) control-plane state. Every KVAM operation that addresses a stream by StreamName short-circuits to 404 ResourceNotFoundException with the emulator-authored message 'No such stream: StreamName <name>' instead of reaching the per-operation behaviour AWS exhibits (retention check, fragment-index lookup, fragment-number validation, empty-page success). Because the refusal is raised before any of those, the operation's real contract is never exercised and even the correct 404 cases carry the wrong message and error class. All 8 causes below are one root cause with per-operation symptoms; a fixer must close all 8.

Cause 1 of 8 — GetClip on a zero-retention stream.
AWS: Smithy GetClip (POST /getClip) models NoDataRetentionException, errorKind client, httpError 400, doc 'requested for a stream that does not retain data (that is, has a DataRetentionInHours of 0)'. A stream created with DataRetentionInHours=0 is resolved, found, and refused 400 NoDataRetentionException with message 'The stream data retention is set to 0 hours.' — that message is entirely fixed text, with no variable part.
Emulator: answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and Message 'No such stream: StreamName <streamName>' (fixed text 'No such stream: StreamName ' plus the StreamName taken from the request). Expected status 400, got 404.
Why: a client that distinguishes 'stream exists but retains nothing' (fix the retention, retry) from 'stream does not exist' (create it) takes the wrong branch, and retries a 404 it should never see for a stream it just created.
Fix: GetClip's handler — the stream-resolution step must look the stream up in the shared Kinesis Video Streams registry before raising ResourceNotFoundException, and only then apply the retention gate.
Traces: kinesisvideoarchivedmedia-get-clip-data-retention-transition#task#4 (also kinesisvideoarchivedmedia-get-clip-reversed-range-rejected#task#5).

Cause 2 of 8 — GetClip on a retaining stream with no fragments.
AWS: ResourceNotFoundException, httpError 404, message 'No fragments found in the stream for the clip request.' — entirely fixed text. The status is 404 and matches; only the class/message diverge.
Emulator: 404 with Message 'No such stream: StreamName <streamName>'.
Why: the client cannot tell 'no media in the requested range yet' (keep polling/ingesting) from 'stream absent' (provision it).
Fix: GetClip's handler — once the stream resolves, the empty-fragment-index path must author AWS's no-fragments message.
Traces: kinesisvideoarchivedmedia-get-clip-data-retention-transition#task#7.

Cause 3 of 8 — GetClip polled after retention is returned to 0.
AWS: 400 NoDataRetentionException, message 'The stream data retention is set to 0 hours.' (fixed text).
Emulator: 404 ResourceNotFoundException 'No such stream: StreamName <streamName>' for every attempt; the fixture's poll for error code NoDataRetentionException never converges and is capped after 61 attempts.
Why: a client waiting on the retention transition to take effect never observes it and hangs until its own timeout.
Fix: GetClip's handler — same resolution step; the retention gate must be re-read from current stream state on each call.
Traces: kinesisvideoarchivedmedia-get-clip-data-retention-transition#task#9.

Cause 4 of 8 — ListFragments on an existing stream with no fragments.
AWS: Smithy ListFragments (POST /listFragments, code 200) returns output member Fragments (FragmentList). A resolvable stream with no fragments in the selector window is a SUCCESS: HTTP 200 with Fragments present and empty, not an error.
Emulator: HTTP 404 and no Fragments member at all (expected [], got absent).
Why: this is a success path turned into a client error — every pagination/discovery client over archived media fails instead of seeing an empty page.
Fix: ListFragments' handler — resolve the stream, then serialize an empty Fragments list on the no-match path.
Traces: kinesisvideoarchivedmedia-get-clip-reversed-range-rejected#task#6 (also kinesisvideoarchivedmedia-get-dash-streaming-session-url-live-and-ondemand#task#7, kinesisvideoarchivedmedia-get-media-for-fragment-list-validation#task#7, kinesisvideoarchivedmedia-list-fragments-addressing-and-validation#task#4, #task#5, #task#6).

Cause 5 of 8 — GetHLSStreamingSessionURL on a zero-retention stream.
AWS: Smithy GetHLSStreamingSessionURL models NoDataRetentionException, httpError 400; refusal message 'The stream data retention is set to 0 hours.' (fixed text).
Emulator: 404 ResourceNotFoundException 'No such stream: StreamName <streamName>'. Expected 400, got 404.
Why: playback clients cannot distinguish a misconfigured retention from a missing stream.
Fix: GetHLSStreamingSessionURL's handler — stream resolution before the retention gate.
Traces: kinesisvideoarchivedmedia-get-hls-streaming-session-url-retention-transition#task#4.

Cause 6 of 8 — GetHLSStreamingSessionURL on a retaining stream with no fragments.
AWS: ResourceNotFoundException, httpError 404, message 'No fragments found in the stream for the streaming request.' — entirely fixed text (Smithy ResourceNotFoundException doc: thrown when a session with PlaybackMode ON_DEMAND/LIVE_REPLAY is requested for a stream with no fragments in the requested range).
Emulator: 404 with Message 'No such stream: StreamName <streamName>'.
Why: a player that retries while ingestion warms up instead treats the stream as nonexistent and gives up.
Fix: GetHLSStreamingSessionURL's handler — author AWS's no-fragments message on the empty-index path.
Traces: kinesisvideoarchivedmedia-get-hls-streaming-session-url-retention-transition#task#7.

Cause 7 of 8 — GetHLSStreamingSessionURL polled after retention returns to 0.
AWS: 400 NoDataRetentionException, message 'The stream data retention is set to 0 hours.'
Emulator: 404 ResourceNotFoundException 'No such stream: StreamName <streamName>' on every attempt; poll for NoDataRetentionException capped after 61 attempts.
Why: the retention transition is never observable, so the client blocks.
Fix: GetHLSStreamingSessionURL's handler — re-read retention from current stream state per call.
Traces: kinesisvideoarchivedmedia-get-hls-streaming-session-url-retention-transition#task#9.

Cause 8 of 8 — GetMediaForFragmentList with out-of-range fragment numbers.
AWS: Smithy GetMediaForFragmentList models InvalidArgumentException, errorKind client, httpError 400. Fragment numbers that pass the member constraints (FragmentNumberString length 1..128, pattern ^[0-9]+$) but name no fragment on the stream are refused 400 InvalidArgumentException with message 'Fragment numbers are invalid : [<fragmentNumbers>]' — fixed text 'Fragment numbers are invalid : [' ... ']', the bracketed part being the offending fragment numbers from the request's Fragments member.
Emulator: HTTP 404 with x-amzn-errortype: ResourceNotFoundException and Message 'No such stream: StreamName <streamName>'. Expected 400, got 404.
Why: the request is rejected for the wrong reason and with the wrong class, so a client cannot learn which fragment numbers to drop.
Fix: GetMediaForFragmentList's handler — resolve the stream, then run the fragment-number existence check and raise InvalidArgumentException.
Traces: kinesisvideoarchivedmedia-get-media-for-fragment-list-validation#task#6.

## Evidence
Cause 1: expected 400 NoDataRetentionException 'The stream data retention is set to 0 hours.'; actual 404 ResourceNotFoundException 'No such stream: StreamName sim2real-probe-kvam-clip-ace56b76' (x-amzn-errortype: ResourceNotFoundException). Cause 2: expected 404 'No fragments found in the stream for the clip request.'; actual 404 'No such stream: StreamName ...'. Cause 3: expected 400 NoDataRetentionException; actual 404 ResourceNotFoundException on all 61 poll attempts ('error code ResourceNotFoundException not in [NoDataRetentionException]'). Cause 4: expected 200 with $.Fragments == []; actual 404 with $.Fragments missing. Cause 5: expected 400 NoDataRetentionException 'The stream data retention is set to 0 hours.'; actual 404 ResourceNotFoundException 'No such stream: StreamName sim2real-probe-kvam-hls-e4b68e5d'. Cause 6: expected 404 'No fragments found in the stream for the streaming request.'; actual 404 'No such stream: StreamName ...'. Cause 7: expected 400 NoDataRetentionException; actual 404 ResourceNotFoundException, poll capped at 60s / 61 attempts. Cause 8: expected 400 InvalidArgumentException 'Fragment numbers are invalid : [99999999999999999999999999999999999999]'; actual 404 ResourceNotFoundException 'No such stream: StreamName sim2real-probe-kvam-gmffl-151d2161'. The stream names above are per-run probe names and are not part of any requirement; the fixed message text and the HTTP status/error class are.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator. 2. Fixture kinesisvideoarchivedmedia-get-clip-data-retention-transition, steps task#4, task#7, task#9: a stream is created through kinesisvideo CreateStream (DataRetentionInHours=0), then GetClip is called; every call answers 404 'No such stream'. 3. Fixture kinesisvideoarchivedmedia-get-hls-streaming-session-url-retention-transition, steps task#4, task#7, task#9: same shape for GetHLSStreamingSessionURL. 4. Fixture kinesisvideoarchivedmedia-get-clip-reversed-range-rejected, step task#6: ListFragments on the created stream answers 404 instead of 200 with an empty Fragments list. 5. Fixture kinesisvideoarchivedmedia-get-media-for-fragment-list-validation, step task#6: GetMediaForFragmentList with an out-of-range fragment number answers 404 ResourceNotFoundException instead of 400 InvalidArgumentException.
