Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-video-stream-lifecycle-create-describe-delete
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-video-stream-lifecycle-create-describe-delete kinesis-video-stream-data-retention-version-guard kinesisvideo-stream-data-retention
Issue-Shape-Id: com.amazonaws.kinesisvideo#KinesisVideo_20170930
Issue-Key: kinesisvideo|DescribeStream|2xx->4xx|$.StreamInfo
Issue-Key: kinesisvideo|DescribeStream|2xx->4xx|status
Issue-Key: kinesisvideo|DeleteStream|4xx->4xx|$.Message
Issue-Key: kinesisvideo|DeleteStream|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|DeleteStream|4xx->4xx|status
Issue-Key: kinesisvideo|DescribeStream|2xx->2xx|$.StreamInfo.Version
Issue-Related: 20261029T173921Z-kinesisvideo-kinesis-video-streams-updatedataretention-is-not-persisted-describestream.md
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# kinesisvideo stream state transitions are not modeled: DeleteStream hard-deletes and UpdateDataRetention is dropped

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStream

## Description
This issue bundles 3 net-new root causes in the Kinesis Video Streams stream-state model: a deleted stream disappears instantly instead of entering DELETING, the follow-up delete is therefore mis-refused as not-found, and an accepted UpdateDataRetention is never reflected by DescribeStream. All three are kinesisvideo and live in the stream record / its DescribeStream projection.

Cause 1 of 3 - DescribeStream returns 404 for a stream that is still DELETING.
AWS: Smithy DeleteStream (POST /deleteStream) is documented as 'Deletes a Kinesis video stream and the data contained in the stream. This method marks the stream for deletion' - the stream is NOT removed synchronously. It moves to the DELETING status (StreamInfo.Status targets the Status enum), and DescribeStream (POST /describeStream) keeps answering HTTP 200 with the full StreamInfo for that stream - same StreamName, StreamARN, DeviceName, MediaType, KmsKeyId, DataRetentionInHours and Version as before, with Status DELETING - until the deletion completes.
Emulator: the DescribeStream that follows a successful DeleteStream answers HTTP 404 with no StreamInfo at all; the stream record is dropped the moment DeleteStream returns.
Why: a client that deletes a stream and then polls DescribeStream to watch the deletion settle (the pattern AWS's own docs push, and what every waiter does) gets ResourceNotFoundException on its first poll. Code that treats 404 as 'the call failed' aborts, and code that must observe the DELETING status can never see it, so the whole asynchronous-deletion contract is unobservable.
Fix: DeleteStream's handler / the stream state - mark the record DELETING rather than removing it, and let DescribeStream project it.
Traces: kinesis-video-stream-lifecycle-create-describe-delete#task#7.

Cause 2 of 3 - DeleteStream on a non-active stream answers 404 ResourceNotFoundException instead of 400 with the not-active refusal.
AWS: DeleteStream models ResourceInUseException (@httpError 400) for exactly this case - the shape's documentation covers an input StreamARN that 'is not in Active status' and points the caller at DescribeStream to check the status - alongside ResourceNotFoundException (@httpError 404) for a stream that does not exist. Deleting a stream that is already DELETING is the first case: HTTP 400 with Message 'The requested stream is not active.' (fixed text; no request values interpolated).
Emulator: HTTP 404 with x-amzn-errortype ResourceNotFoundException and Message 'The requested stream is not found or not active.' - it merges the two distinct conditions into one not-found refusal, with a sentence that fuses both AWS sentences.
Why: the status, the error class and the message are all wrong, so a client cannot distinguish 'this stream never existed' (retry is pointless) from 'this stream is mid-deletion' (retry/wait is correct). Retry logic keyed on ResourceInUseException never fires.
Fix: DeleteStream's state gate - classify a present-but-not-ACTIVE stream as ResourceInUseException/400 with AWS's sentence, and reserve 404 for an absent stream.
Traces: kinesis-video-stream-lifecycle-create-describe-delete#task#8.

Cause 3 of 3 - an accepted UpdateDataRetention is not reflected in DescribeStream's DataRetentionInHours.
AWS: Smithy UpdateDataRetention (POST /updateDataRetention) takes required CurrentVersion, required Operation (INCREASE_DATA_RETENTION / DECREASE_DATA_RETENTION) and required DataRetentionChangeInHours, and on success returns 200 with an empty body. StreamInfo.DataRetentionInHours is documented as 'How long the stream retains data, in hours', so after an accepted update DescribeStream reports the stream's CURRENT retention - the creation-time value adjusted by the accepted delta - not its creation-time value.
Emulator: UpdateDataRetention answers 200, but the following DescribeStream still reports the stream's creation-time retention (in this corpus 1 where the post-update retention is 25). The mutation is acknowledged and discarded.
Why: a client that raises retention and confirms it via DescribeStream sees its change lost with no error to signal it, so it loops waiting for a value that never arrives, and any behaviour gated on retention (archived-media playback, clip extraction, edge configuration, which the model refuses with NoDataRetentionException at zero retention) stays stuck on the stale value.
Fix: UpdateDataRetention's handler / the stream state - apply the INCREASE/DECREASE delta to the stored record (and bump its Version) so DescribeStream's StreamInfo projection reads the current retention.
Note: open issue 20261029T173921Z-kinesisvideo-kinesis-video-streams-updatedataretention-is-not-persisted-describestream reports the same root cause from the kinesisvideoarchivedmedia fixtures (retention stuck at 0). It does not carry this fixture/step pair, so this cause is filed with its own replay key; fixing either should close both, but this cluster must be re-replayed to confirm.
Traces: kinesis-video-stream-data-retention-version-guard#task#4 (also #task#8, kinesisvideo-stream-data-retention#task#4, #task#8).

## Evidence
Replay of corpus simparity-wire-aws-v1 (emulator up; 147 steps, 85 passed).
Cause 1 - kinesis-video-stream-lifecycle-create-describe-delete#task#7: expected HTTP 200 with $.StreamInfo present (Status 'DELETING', with the stream's StreamName, StreamARN, DeviceName, MediaType, KmsKeyId and DataRetentionInHours carried over); got HTTP 404 and $.StreamInfo missing. The particular ARN/creation-time in the recording is this run's generated identity; the requirement is that the stream remain describable with Status DELETING.
Cause 2 - kinesis-video-stream-lifecycle-create-describe-delete#task#8: expected HTTP 400 with $.Message 'The requested stream is not active.'; got HTTP 404 with $.Message 'The requested stream is not found or not active.' and $headers.x-amzn-errortype 'ResourceNotFoundException' (the recording's errortype has a different shape).
Cause 3 - kinesis-video-stream-data-retention-version-guard#task#4 (and #task#8, kinesisvideo-stream-data-retention#task#4/#task#8): status 200 matches, $.StreamInfo.DataRetentionInHours expected 25, got 1. 25 and 1 are this fixture's post- and pre-update retentions; the requirement is that the projected value be the retention resulting from the accepted update.
Excluded from this issue: the $.StreamInfo.Version / $.ChannelInfo.Version shape mismatches reported in the same clusters - Version is a server-generated id that differs per run.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay the wire corpus simparity-wire-aws-v1.
2. Causes 1-2, fixture kinesis-video-stream-lifecycle-create-describe-delete: CreateStream, then DeleteStream with the stream's CurrentVersion (succeeds). Step task#7: POST /describeStream for that stream - expect 200 with StreamInfo.Status DELETING, observe 404. Step task#8: POST /deleteStream again - expect 400 with Message 'The requested stream is not active.', observe 404 ResourceNotFoundException 'The requested stream is not found or not active.'.
3. Cause 3, fixture kinesis-video-stream-data-retention-version-guard: CreateStream with DataRetentionInHours=1, then UpdateDataRetention with Operation=INCREASE_DATA_RETENTION and DataRetentionChangeInHours=24 (returns 200). Step task#4: POST /describeStream and read $.StreamInfo.DataRetentionInHours - it is still 1 instead of 25.
