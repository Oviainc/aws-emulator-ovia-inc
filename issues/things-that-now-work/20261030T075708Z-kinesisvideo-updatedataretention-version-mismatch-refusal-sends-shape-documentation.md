Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-stream-data-retention-version-guard
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-stream-data-retention-version-guard kinesisvideo-stream-data-retention
Issue-Shape-Id: com.amazonaws.kinesisvideo#KinesisVideo_20170930
Issue-Key: kinesisvideo|UpdateDataRetention|4xx->4xx|$.Message
Issue-Commit: 8b001d263b344ae25624bdf4c2d07c4c668637f4

# kinesisvideo UpdateDataRetention version-mismatch refusal sends shape documentation prose instead of AWS refusal sentence

> _Note: this issue was identified against the code base at commit `8b001d263b344ae25624bdf4c2d07c4c668637f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDataRetention

## Description
This issue carries 1 net-new root cause on the kinesisvideo stream data-retention surface: the optimistic-concurrency refusal of UpdateDataRetention is rendered from the error shape's documentation paragraph rather than the sentence the service actually sends.

Cause 1 of 1 - UpdateDataRetention's VersionMismatchException message is reworded.
AWS: Smithy UpdateDataRetention (POST /updateDataRetention) takes a required CurrentVersion (target Version, @length 1..64, @pattern ^[a-zA-Z0-9]+$), a required Operation and a required DataRetentionChangeInHours, and models VersionMismatchException (@httpError 400). When CurrentVersion is not the stream's current version the service refuses with HTTP 400 and Message 'The provided stream version does not match the existing stream version.' - that text is entirely fixed: no request value, stream name, ARN or version string is interpolated into it. (Smithy: UpdateDataRetention.CurrentVersion required; error VersionMismatchException @httpError 400, documented 'The stream version that you specified is not the latest version. To get the latest version, use the DescribeStream API.' - documentation prose, which is NOT the wire message.)
Emulator: HTTP 400 and the x-amzn-errortype are correct, but $.Message is 'The stream version specified is not the latest version. To get the latest version, use the DescribeStream API.' - i.e. the shape's documentation paragraph reused as the wire message instead of the service's refusal sentence. The same substitution already exists for UpdateSignalingChannel (open issue 20261029T194209Z-kinesisvideo-refusal-sentences-and-signaling-channel-endpoint-projections.md, Cause 1); that issue does not cover the stream data-retention path, so this one is filed separately.
Why: a client doing optimistic concurrency on a stream (read Version via DescribeStream, retry on version conflict) that recognises the conflict by AWS's sentence - or an operator reading it in a log - sees text that never appears in AWS, so conflict fingerprints and retry branches diverge even though the status and error code match.
Fix: UpdateDataRetention's version guard in the kinesisvideo handler/validation layer - emit the service's fixed VersionMismatchException sentence for the stream path rather than the error shape's documentation text.
Traces: kinesis-video-stream-data-retention-version-guard#task#5 (also #task#6, kinesisvideo-stream-data-retention#task#5, #task#6).

## Evidence
Replay of corpus simparity-wire-aws-v1 against the emulator built from the checkout (emulator up; 147 steps, 89 passed, 58 mismatched, 24 divergence clusters).
Cause 1 - cluster service=kinesisvideo|UpdateDataRetention|status=400->400|keys=$.Message, representative kinesis-video-stream-data-retention-version-guard#task#5, 4 affected steps (kinesis-video-stream-data-retention-version-guard#task#5, #task#6, kinesisvideo-stream-data-retention#task#5, kinesisvideo-stream-data-retention#task#6):
  expected status 400, actual status 400 (match)
  $.Message value_mismatch: expected 'The provided stream version does not match the existing stream version.', got 'The stream version specified is not the latest version. To get the latest version, use the DescribeStream API.'
Both strings are wholly fixed text - neither interpolates the stream name, ARN or the rejected version - so the divergence is the wording itself, not a per-run value. No other response key in this cluster diverges.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay the wire corpus simparity-wire-aws-v1 (every step of fixture kinesis-video-stream-data-retention-version-guard).
2. CreateStream, then DescribeStream to read $.StreamInfo.Version.
3. POST /updateDataRetention with the stream's StreamName/StreamARN, Operation INCREASE_DATA_RETENTION, a DataRetentionChangeInHours >= 1, and a CurrentVersion that is NOT the stream's current version (step task#5 of that fixture).
4. Observe HTTP 400 with $.Message 'The stream version specified is not the latest version. To get the latest version, use the DescribeStream API.'; AWS answers HTTP 400 with $.Message 'The provided stream version does not match the existing stream version.'
5. Repeat at kinesis-video-stream-data-retention-version-guard#task#6 and kinesisvideo-stream-data-retention#task#5/#task#6 for the same divergence.
