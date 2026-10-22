Issue-Source: trace-sweep-divergence
Issue-Trace: logs-log-group-query-discovery
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-log-group-query-discovery logspc-put-log-events-rejected-timestamps-and-ignored-sequence-token logsrd-describe-log-streams-golden logsrd-get-log-events-out-of-window-read
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|PutLogEvents|2xx->2xx|$.rejectedLogEventsInfo
Issue-Key: logs|DescribeLogStreams|2xx->2xx|$.logStreams[*].firstEventTimestamp
Issue-Key: logs|DescribeLogStreams|2xx->2xx|$.logStreams[*].lastEventTimestamp
Issue-Key: logs|DescribeLogStreams|2xx->2xx|$.logStreams[*].lastIngestionTime
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[*].storedBytes
Issue-Key: logs|GetLogEvents|2xx->2xx|$.events[*]
Issue-Key: logs|GetLogEvents|2xx->2xx|$.nextBackwardToken
Issue-Key: logs|GetLogEvents|2xx->2xx|$.nextForwardToken
Issue-Related: 20261022T220153Z-logs-describeloggroups-omits-bearertokenauthenticationenabled-and-kmskeyid-from-every-log.md
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# PutLogEvents accepts out-of-window log events without reporting rejectedLogEventsInfo

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLogEvents

## Description
CloudWatch Logs silently drops log events whose timestamp falls outside the ingestion window and reports which ones it dropped in the PutLogEvents response's rejectedLogEventsInfo. The emulator ingests every event unconditionally and never populates rejectedLogEventsInfo, so the rejected events become readable, the stream's event timestamps are set from them, and the log group's storedBytes grows. All four causes are the same root defect observed on PutLogEvents, DescribeLogStreams and GetLogEvents. This bundle has 4 causes; all 4 must be closed.

Cause 1 of 4 -- PutLogEvents does not report rejected events.
AWS: Smithy -- PutLogEvents output carries rejectedLogEventsInfo: RejectedLogEventsInfo, whose documented members are tooNewLogEventStartIndex ("The index of the first log event that is too new. This field is inclusive."), tooOldLogEventEndIndex ("The index of the last log event that is too old. This field is exclusive.") and expiredLogEventEndIndex. Recorded: a batch containing events older than the ingestion window returns 200 with rejectedLogEventsInfo = {tooOldLogEventEndIndex: <index>} -- the accepted events are stored, the out-of-window ones are not.
Emulator: 200 with rejectedLogEventsInfo absent; every event in the batch is accepted.
Why: the modeled feedback channel for partial rejection is dead. A producer cannot detect that part of its batch was dropped, cannot adjust timestamps, and cannot alarm on backfill being refused -- it believes a batch landed that on AWS did not. This cluster spans seven fixture steps.
Fix: the PutLogEvents handler -- apply the ingestion-window check per event, store only accepted events, and populate rejectedLogEventsInfo with the tooOld / tooNew / expired boundary indexes.
Traces: logs-log-group-query-discovery#task#3 (also logspc-put-log-events-rejected-timestamps-and-ignored-sequence-token#task#3, #task#4, #task#5 and #task#6, logsrd-describe-log-streams-golden#task#4, logsrd-get-log-events-out-of-window-read#task#3).

Cause 2 of 4 -- DescribeLogStreams reports event timestamps for a stream whose only events were rejected.
AWS: Smithy -- LogStream.firstEventTimestamp, lastEventTimestamp and lastIngestionTime are optional members set from stored events. Recorded: because the submitted events were all out of window and therefore not stored, the described stream carries NONE of the three members.
Emulator: all three members are present -- firstEventTimestamp and lastEventTimestamp set from the rejected event's timestamp, lastIngestionTime set from the write.
Why: a client reading stream metadata concludes the stream has data and sits inside a time range that AWS would report as empty, so freshness checks and time-window queries built on those members target the wrong range.
Fix: the DescribeLogStreams projection follows the stream state written by PutLogEvents -- once cause 1 stops storing rejected events, these members must remain unset.
Traces: logsrd-describe-log-streams-golden#task#5.

Cause 3 of 4 -- the log group's storedBytes counts rejected events.
AWS: Smithy -- LogGroup.storedBytes targets StoredBytes @range min 0, "The number of bytes stored" (note LogStream.storedBytes is deprecated and always zero, but the log group member is live). Recorded: the log group's storedBytes is 0, because nothing was stored.
Emulator: storedBytes is non-zero -- the rejected payload was billed into the log group. bearerTokenAuthenticationEnabled is also absent on this object; that member is filed under the DescribeLogGroups projection issue.
Why: storage accounting is wrong in the direction that matters -- a client validating that out-of-window backfill was refused sees bytes stored and concludes the write succeeded.
Fix: the log-group storedBytes accounting in the PutLogEvents handler -- only accepted events contribute.
Traces: logsrd-describe-log-streams-golden#task#6.

Cause 4 of 4 -- GetLogEvents serves the out-of-window events back.
AWS: Smithy -- GetLogEvents output events: OutputLogEvents, nextForwardToken, nextBackwardToken. Recorded: a read of the stream returns NO events, because both the too-old and the too-new event were rejected at ingest.
Emulator: returns both of them -- the too-old event and the too-new event, each with an ingestionTime.
Why: this is the read-side proof of cause 1 and the most damaging consequence: data AWS would never have retained is served to readers, so any consumer replaying the stream processes events that do not exist on the real service.
Fix: GetLogEvents reads stream state; once cause 1 stops storing rejected events this read returns empty. (The token values on this step also diverge; that is filed separately as the GetLogEvents token-envelope issue.)
Traces: logsrd-get-log-events-out-of-window-read#task#4 (also the same fixture's task#5).

## Evidence
Cause 1 (logs-log-group-query-discovery#task#3, 7 affected steps, 200 on both sides): $.rejectedLogEventsInfo expected {'tooOldLogEventEndIndex': 2}, got None.
Cause 2 (logsrd-describe-log-streams-golden#task#5, 200 on both sides): $.logStreams[0].firstEventTimestamp expected None, got an event timestamp; $.logStreams[0].lastEventTimestamp expected None, got the same timestamp; $.logStreams[0].lastIngestionTime expected None, got the write time.
Cause 3 (logsrd-describe-log-streams-golden#task#6, 200 on both sides): $.logGroups[0].storedBytes expected 0, got 76.
Cause 4 (logsrd-get-log-events-out-of-window-read#task#4, 2 affected steps, 200 on both sides): $.events[0] expected None, got the too-old event ('sim2real out-of-window event: too old (2001-09-09)'); $.events[1] expected None, got the too-new event ('too new (2100-01-01)').
Smithy citations: PutLogEventsResponse.rejectedLogEventsInfo -> RejectedLogEventsInfo {tooNewLogEventStartIndex, tooOldLogEventEndIndex, expiredLogEventEndIndex}; LogGroup.storedBytes -> StoredBytes @range min 0; LogStream.firstEventTimestamp / lastEventTimestamp / lastIngestionTime; LogStream.storedBytes is @deprecated and always zero.
The specific byte count, timestamps and ingestion times are environment-dependent; the requirement is that out-of-window events are not stored, are reported in rejectedLogEventsInfo, and are not readable.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay logs-log-group-query-discovery step task#3: PutLogEvents with a batch whose leading events predate the ingestion window. The emulator answers 200 with no rejectedLogEventsInfo; the recording answers 200 with tooOldLogEventEndIndex set.
3. Replay logsrd-describe-log-streams-golden#task#5 -- the described stream has firstEventTimestamp / lastEventTimestamp / lastIngestionTime set where the recording has none.
4. Replay logsrd-describe-log-streams-golden#task#6 -- the log group's storedBytes is non-zero where the recording is 0.
5. Replay logsrd-get-log-events-out-of-window-read#task#4 -- GetLogEvents returns the too-old and too-new events where the recording returns an empty events list.
