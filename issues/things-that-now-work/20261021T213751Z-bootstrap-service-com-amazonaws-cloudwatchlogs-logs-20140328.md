Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the CloudWatch Logs emulator

_Filed: 2026-10-21T21:37:51Z_
_Operation: service-com-amazonaws-cloudwatchlogs-logs-20140328 | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 118 operations in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/` is an unmodified scaffold (`//! SCAFFOLDED, NOT IMPLEMENTED:` header, `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501), so no client workflow works at all. Implement a FIRST, SMALL, stateful slice: the log-group/log-stream/log-event core loop, in this order.

1. **CreateLogGroup** — input `logGroupName` (required, len 1..512, pattern `^[\.\-_/#A-Za-z0-9]+$`), optional `kmsKeyId`, `tags`, `logGroupClass` (STANDARD default), `deletionProtectionEnabled`. Empty output. Must insert a LogGroup record (with `creationTime` = now ms, computed `arn`/`logGroupArn`) and return `ResourceAlreadyExistsException` (HTTP 400) when the name already exists.
2. **CreateLogStream** — input `logGroupName` + `logStreamName` (both required; stream pattern `^[^:*]*$`). Empty output. Must return `ResourceNotFoundException` when the log group does not exist, `ResourceAlreadyExistsException` when the stream already exists, otherwise persist a LogStream with `creationTime` and an `arn`.
3. **PutLogEvents** — input `logGroupName`, `logStreamName`, `logEvents` (list of `InputLogEvent{timestamp (required, >=0), message (required, len>=1)}`, 1..10000), optional `sequenceToken`. Output `nextSequenceToken` (must be a freshly generated token persisted on the stream), optional `rejectedLogEventsInfo`. Must append the events to the stream's stored event list sorted by `timestamp`, stamp each with an `ingestionTime` (now ms), update the stream's `firstEventTimestamp`/`lastEventTimestamp`/`lastIngestionTime` and the group's `storedBytes`, and return `ResourceNotFoundException` if the group or stream is unknown. (Real CloudWatch Logs now ignores an invalid `sequenceToken` — accept the call regardless; do not fail on it.)
4. **GetLogEvents** — input `logStreamName` (required) plus `logGroupName`/`logGroupIdentifier`, optional `startTime`, `endTime`, `limit` (1..10000), `startFromHead`. Output `events` (list of `OutputLogEvent{timestamp, message, ingestionTime}`), `nextForwardToken`, `nextBackwardToken`. Must read the persisted events for that stream, filter to `startTime <= timestamp < endTime` when given, order ascending by timestamp (descending/tail-first when `startFromHead` is false/absent), truncate to `limit`, and return tokens that actually resume the scan on the next call. `ResourceNotFoundException` for unknown group/stream.
5. **DescribeLogStreams** — input `logGroupName` (or `logGroupIdentifier`), optional `logStreamNamePrefix`, `orderBy` (LogStreamName | LastEventTime), `descending`, `limit` (1..50), `nextToken`. Output `logStreams` (list<LogStream>) + `nextToken`. Must list the streams actually created in that group, with the live `creationTime`/`firstEventTimestamp`/`lastEventTimestamp`/`lastIngestionTime`/`uploadSequenceToken`/`arn` values maintained by PutLogEvents, honouring prefix, ordering and `limit`/`nextToken`.
6. **DescribeLogGroups** — optional `logGroupNamePrefix`, `logGroupNamePattern`, `limit` (1..50), `nextToken`, `logGroupClass`, `logGroupIdentifiers`. Output `logGroups` (list<LogGroup>) + `nextToken`. Must enumerate created groups with `logGroupName`, `creationTime`, `arn`, `logGroupArn`, `storedBytes`, `metricFilterCount` (0), `logGroupClass`, `retentionInDays` when set, honouring the prefix filter and the `limit`/`nextToken` page bounds.
7. **DeleteLogGroup** — input `logGroupName` (required). Must remove the group and ALL of its streams and events, so a following DescribeLogGroups does not list it and GetLogEvents/CreateLogStream on it fail with `ResourceNotFoundException`. Deleting an unknown group returns `ResourceNotFoundException` (HTTP 400).

Every one of these must compute its response from state persisted in the shared engine store (`crate::state::service_state(&*self.engine)`) — no canned, hardcoded or echoed responses, and no generated ids/tokens that are not stored. A create followed by a describe must return exactly what was created; a PutLogEvents followed by a GetLogEvents must return exactly the messages and timestamps that were written.

## State Model

In `ComAmazonawsCloudwatchlogsLogs20140328State`:

- `log_groups: BTreeMap<String /* logGroupName */, LogGroup>` where
  `LogGroup { name: String, arn: String /* arn:aws:logs:<region>:<account>:log-group:<name>:* */, log_group_arn: String /* same without trailing :* */, creation_time_ms: i64, retention_in_days: Option<i32>, kms_key_id: Option<String>, log_group_class: String /* STANDARD|INFREQUENT_ACCESS|DELIVERY */, deletion_protection_enabled: bool, stored_bytes: i64, metric_filter_count: i32, tags: BTreeMap<String,String>, streams: BTreeMap<String /* logStreamName */, LogStream> }`
- `LogStream { name: String, arn: String, creation_time_ms: i64, first_event_timestamp: Option<i64>, last_event_timestamp: Option<i64>, last_ingestion_time: Option<i64>, upload_sequence_token: String, events: Vec<StoredEvent> }` with `events` kept sorted by `(timestamp, ingestion_seq)`
- `StoredEvent { timestamp: i64, message: String, ingestion_time: i64, ingestion_seq: u64 }`
- `next_ingestion_seq: u64` — monotonic counter used to make the event order and the GetLogEvents / DescribeLogStreams / FilterLogEvents pagination tokens stable (a token encodes the stream name plus the last `ingestion_seq` returned).

Out of scope for this first slice and expected to stay 501 (filed later as individual gap issues): FilterLogEvents, PutRetentionPolicy/DeleteRetentionPolicy, subscription/metric filters, Insights queries (StartQuery/GetQueryResults), deliveries, integrations, anomaly detectors, export/import tasks, tagging, StartLiveTail.

## Evidence

Spec: `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json` via OPERATIONS.md — awsJson1_1, `X-Amz-Target: Logs_20140328.<Operation>`; CreateLogGroup (lines 75-79), CreateLogStream (81-85), DeleteLogGroup (159-163), DescribeLogGroups (296-301, @paginated items=logGroups), DescribeLogStreams (303-308, @paginated items=logStreams), GetLogEvents (409-414, @paginated outputToken=nextForwardToken items=events), PutLogEvents (610-614). Shape members for LogGroup/LogStream/InputLogEvent/OutputLogEvent taken from the model via smithy_query. All handlers confirmed scaffolds: `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/create_log_group.rs` matches, and the README documents that marker as the stub census. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. POST / with X-Amz-Target: Logs_20140328.CreateLogGroup, body {"logGroupName":"/aws/emulator/demo"} -> expect HTTP 200 with empty JSON body (today: 501 NotImplemented)
2. CreateLogGroup again with the same name -> expect HTTP 400 ResourceAlreadyExistsException
3. CreateLogStream {"logGroupName":"/aws/emulator/demo","logStreamName":"stream-1"} -> 200
4. CreateLogStream on a nonexistent group -> 400 ResourceNotFoundException
5. PutLogEvents {"logGroupName":"/aws/emulator/demo","logStreamName":"stream-1","logEvents":[{"timestamp":1700000000000,"message":"hello"},{"timestamp":1700000001000,"message":"world"}]} -> 200 with a nextSequenceToken
6. GetLogEvents {"logGroupName":"/aws/emulator/demo","logStreamName":"stream-1","startFromHead":true} -> 200 with events [hello, world] in timestamp order, each carrying timestamp, message and a nonzero ingestionTime, plus nextForwardToken/nextBackwardToken
7. GetLogEvents with {"limit":1,"startFromHead":true} then re-call with the returned nextForwardToken -> second page returns the "world" event only
8. DescribeLogStreams {"logGroupName":"/aws/emulator/demo"} -> 200 listing stream-1 with lastEventTimestamp == 1700000001000 and a nonzero creationTime
9. DescribeLogGroups {"logGroupNamePrefix":"/aws/emulator"} -> 200 listing /aws/emulator/demo with its arn/logGroupArn and creationTime
10. DeleteLogGroup {"logGroupName":"/aws/emulator/demo"} -> 200; then DescribeLogGroups no longer lists it and GetLogEvents on stream-1 returns 400 ResourceNotFoundException
11. DeleteLogGroup again -> 400 ResourceNotFoundException
