Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisvideoarchivedmedia-get-clip-data-retention-transition
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisvideoarchivedmedia-get-clip-data-retention-transition kinesisvideoarchivedmedia-get-hls-streaming-session-url-retention-transition
Issue-Shape-Id: com.amazonaws.kinesisvideoarchivedmedia#AWSAcuityReader
Issue-Key: kinesisvideo|DescribeStream|2xx->2xx|$.StreamInfo.DataRetentionInHours
Issue-Commit: a8a8dc0cc1ff1737eab503998484303fbbf09d5b

# Kinesis Video Streams UpdateDataRetention is not persisted: DescribeStream keeps reporting DataRetentionInHours 0

> _Note: this issue was identified against the code base at commit `a8a8dc0cc1ff1737eab503998484303fbbf09d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStream

## Description
Cause 1 of 1 — a successful UpdateDataRetention does not change the stream's retention as seen by DescribeStream.
AWS: Smithy UpdateDataRetention (POST /updateDataRetention) takes required CurrentVersion, required Operation (UpdateDataRetentionOperation enum: INCREASE_DATA_RETENTION / DECREASE_DATA_RETENTION) and required DataRetentionChangeInHours (@range min 1), and on success returns 200 with an empty body. DescribeStream (POST /describeStream) then returns StreamInfo whose DataRetentionInHours member reflects the new retention: after an INCREASE_DATA_RETENTION of N hours applied to a stream created with DataRetentionInHours 0, DescribeStream reports N. The rule is that the retention DescribeStream projects is the stream's current retention after every accepted UpdateDataRetention, not its creation-time value.
Emulator: UpdateDataRetention answers 200, but the following DescribeStream returns 200 with $.StreamInfo.DataRetentionInHours still 0 — the increase was accepted and silently discarded.
Why: a client that raises retention and then confirms it via DescribeStream sees its change lost, so it either loops forever waiting for the new value or concludes the stream cannot retain data. Worse, the mutation is reported as succeeding, so nothing signals the loss; any downstream behaviour gated on retention (archived-media playback, clip extraction) stays permanently in the zero-retention branch.
Fix: UpdateDataRetention's handler / the Kinesis Video Streams stream state — apply the INCREASE/DECREASE delta to the stored stream record (and bump its Version) so DescribeStream's StreamInfo projection reads the updated retention.
Traces: kinesisvideoarchivedmedia-get-clip-data-retention-transition#task#6, kinesisvideoarchivedmedia-get-hls-streaming-session-url-retention-transition#task#6.

## Evidence
Step kinesisvideoarchivedmedia-get-clip-data-retention-transition#task#6: DescribeStream returns HTTP 200 (as expected) but $.StreamInfo.DataRetentionInHours is 0 where the recording has 1, after the preceding UpdateDataRetention step returned success. The same divergence appears at kinesisvideoarchivedmedia-get-hls-streaming-session-url-retention-transition#task#6. The value 1 here is the fixture's requested DataRetentionChangeInHours; the requirement is that DescribeStream report the retention resulting from the accepted update, whatever its magnitude.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator. 2. Fixture kinesisvideoarchivedmedia-get-clip-data-retention-transition: CreateStream with DataRetentionInHours=0, then UpdateDataRetention with Operation=INCREASE_DATA_RETENTION and DataRetentionChangeInHours=1 (succeeds, 200). 3. Step task#6: call DescribeStream for the same stream and read $.StreamInfo.DataRetentionInHours — it is still 0 instead of 1.
