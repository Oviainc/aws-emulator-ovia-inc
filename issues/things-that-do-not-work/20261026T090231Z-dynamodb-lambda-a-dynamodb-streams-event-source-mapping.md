Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records
Issue-Key: dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: ac01852cc17d3270a54b28950effcfcf66838250

# dynamodb__lambda: a DynamoDB-Streams event source mapping with MaximumRecordAgeInSeconds never delivers anything on either leg, so the age-filter semantics are unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `ac01852cc17d3270a54b28950effcfcf66838250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: a DynamoDB Streams -> Lambda event source mapping configured with MaximumRecordAgeInSeconds produces no invocation at all, on neither the age-filtered leg nor the infinite-age (-1) control leg, so the `MaximumRecordAgeInSeconds` semantics can never be observed and the check is killed at the harness cap with no assertion reached.

AWS: Smithy models the field as `com.amazonaws.lambda#CreateEventSourceMappingRequest$MaximumRecordAgeInSeconds` -> `com.amazonaws.lambda#MaximumRecordAgeInSeconds`, an integer with @range min -1, max 604800, documented "(Kinesis, DynamoDB Streams, Amazon MSK, and self-managed Apache Kafka) Discard records older than the specified age. The default value is infinite (-1)." The model says nothing about delivery timing, so for the behavior itself this is recorded behavior only. The recording (real AWS, us-west-2) shows two otherwise identical TRIM_HORIZON DynamoDB-stream ESMs on two twin stream-enabled tables that were handed the same aged batch in the same instant: (a) the control leg, MaximumRecordAgeInSeconds=-1, delivers the whole aged backlog AND a record written after the mapping was created, within a couple of minutes; (b) the subject leg, MaximumRecordAgeInSeconds=60 with a shard whose oldest available record is already older than 60s at mapping-creation time, never invokes the handler again for the rest of the window — not the aged records and not the later fresh record — while GetEventSourceMapping keeps reporting State 'Enabled', LastProcessingResult 'No records processed', and echoes MaximumRecordAgeInSeconds=60. The source tables themselves are unaffected: a strongly-consistent GetItem on each twin returns the last written version. So AWS's requirement here is three-part: the -1 leg must deliver; the aged leg must stall while staying Enabled; and GetEventSourceMapping must echo the configured age back.

Emulator: the check never reaches any of its assertions — the harness killed it at its 60.0 s cap (verdict: mismatch, 'Check timed out after 60.0s'). The run blocks before the discriminating read: the terminal observable is a strongly-consistent GetItem on an audit table the Lambda writes, and nothing is ever written there, so neither leg of the pair delivered a batch. Because the kill is a timeout and not an assertion, the exact blocking call is not recorded in the replay output. Hypothesis: the same blocking point as the already-filed sibling checks with the identical 60 s-timeout symptom on this wiring — either a StreamSpecification CreateTable that never becomes visible to DescribeTable/never mints a usable LatestStreamArn (open issues 20261023T110802Z-dynamodb-stream-enablement-is-unimplemented-createtable-with-streamspecification.md and 20261025T011017Z-dynamodb-createtable-with-streamspecification-leaves-the-table-invisible-to.md), hanging the table_exists waiter, or the DynamoDB-Streams ESM poller hop itself never invoking the function (open issues 20261025T192630Z-dynamodb-lambda-s3-a-dynamodb-streams-event-source.md, 20261026T005239Z-dynamodb-kinesis-lambda-a-dynamodb-streams-event-source.md, 20261024T150703Z-apigateway-dynamodb-lambda-logs-a-dynamodb-streams-event.md). None of those open issues carries this check's replay key — the suite reported this check unowned — and none of them covers the age-filter cause: even once delivery works, an emulator that ignores MaximumRecordAgeInSeconds (delivers the aged backlog on the subject leg) or that treats -1 as zero (delivers nothing on the control leg) still diverges here. That hypothesis must be confirmed by re-replaying this check.

Why: a client relying on AWS's behavior gets its poison-pill containment knob silently wrong. MaximumRecordAgeInSeconds is the only DynamoDB-stream tuning knob that bounds a shard's progress by wall clock rather than attempt count. Against the emulator nothing is delivered on either setting, so a consumer cannot tell a permanently stalled shard from a shard with no delivery path at all, and cannot validate either outcome: a team that sets a maximum age and expects a backlogged shard to behave one way gets no change-data-capture whatsoever, with no error anywhere to observe.

Fix: the DynamoDB-Streams event-source poller behind lambda CreateEventSourceMapping (the component that opens shard iterators on the source stream ARN, applies the per-record age filter derived from MaximumRecordAgeInSeconds / treats -1 as infinite, and invokes the function with the batch), its State/LastProcessingResult bookkeeping and its MaximumRecordAgeInSeconds echo in GetEventSourceMapping; plus whatever blocks a StreamSpecification CreateTable from becoming visible in the dynamodb table state if the hypothesis above holds.

Traces: dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records

## Evidence
Check dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records — verdict: mismatch.
  Error: Check timed out after 60.0s (harness cap; no assertion reached, so no frame is reported).

Expected (recorded flow, real AWS):
  - Two twin tables created with StreamSpecification {StreamEnabled: true, StreamViewType: NEW_AND_OLD_IMAGES}; CreateTable returns a LatestStreamArn for each.
  - The same 4 versions (v=0..3) are written to both twins before any mapping exists; the subject stream's oldest record's own dynamodb.ApproximateCreationDateTime, read via dynamodbstreams GetRecords, becomes >= 100 s old (demonstrably older than the 60 s filter).
  - CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<subject stream arn>, Enabled=true, StartingPosition="TRIM_HORIZON", BatchSize=9, MaximumBatchingWindowInSeconds=0, MaximumRecordAgeInSeconds=60) and an identical mapping on the control stream with MaximumRecordAgeInSeconds=-1.
  - A fresh record (v=99) is written to both twins after both mappings exist.
  - CONTROL (-1): the audit table accumulates entries covering {0,1,2,3} and 99 for the control table within the allowed window.
  - SUBJECT (60): the audit table never contains a single entry for the subject table — not the aged versions, not v=99 — held for at least max(180 s, 5x the control's demonstrated latency).
  - GetEventSourceMapping(UUID=<subject>) reports State in ('Enabled','Updating') and MaximumRecordAgeInSeconds == 60.
  - GetItem(ConsistentRead=True) on each twin returns {pk: 'aging-item', v: 99}.

Actual (emulator): the run is killed at 60 s before any of the above assertions is evaluated. The audit table receives nothing from either leg, so the control leg (MaximumRecordAgeInSeconds=-1), which AWS delivers on, delivered nothing either — the pair's discriminating comparison cannot be made at all.

Smithy cited: com.amazonaws.lambda#CreateEventSourceMappingRequest$MaximumRecordAgeInSeconds -> com.amazonaws.lambda#MaximumRecordAgeInSeconds, integer @range(min=-1, max=604800); doc "(Kinesis, DynamoDB Streams, Amazon MSK, and self-managed Apache Kafka) Discard records older than the specified age. The default value is infinite (-1)."

Related-but-not-covering open issues (named per instructions; none carries this check's replay key): 20261023T110802Z-dynamodb-stream-enablement-is-unimplemented-createtable-with-streamspecification.md, 20261025T011017Z-dynamodb-createtable-with-streamspecification-leaves-the-table-invisible-to.md, 20261025T192630Z-dynamodb-lambda-s3-a-dynamodb-streams-event-source.md, 20261026T005239Z-dynamodb-kinesis-lambda-a-dynamodb-streams-event-source.md, 20261024T150703Z-apigateway-dynamodb-lambda-logs-a-dynamodb-streams-event.md.

## Steps to Reproduce
1. CreateTable a plain audit table; PutItem {pk: 'log', entries: {L: []}} into it.
2. CreateTable two twin source tables, each with StreamSpecification {StreamEnabled: true, StreamViewType: 'NEW_AND_OLD_IMAGES'}, BillingMode PAY_PER_REQUEST, key schema pk (S). Assert each CreateTable response carries TableDescription.LatestStreamArn and wait for table_exists.
3. Create an IAM role trusted by lambda.amazonaws.com allowing dynamodb:GetRecords/GetShardIterator/DescribeStream/ListStreams on <each source arn>/stream/* and dynamodb:UpdateItem on the audit table arn; CreateFunction (python3.12) with a handler that appends {src: <source table name from eventSourceARN>, b: [NewImage.v as int ...]} to the audit item's `entries` list, and wait for State Active.
4. PutItem {pk: 'aging-item', v: 0} then UpdateItem v=1,2,3 on BOTH source tables — before any mapping exists.
5. Poll dynamodbstreams DescribeStream + GetShardIterator(TRIM_HORIZON) + GetRecords on the subject stream until the oldest record's dynamodb.ApproximateCreationDateTime is at least 100 s in the past.
6. CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<subject LatestStreamArn>, Enabled=true, StartingPosition='TRIM_HORIZON', BatchSize=9, MaximumBatchingWindowInSeconds=0, MaximumRecordAgeInSeconds=60). Create the same mapping on the control stream with MaximumRecordAgeInSeconds=-1.
7. UpdateItem v=99 on both source tables.
8. Poll GetItem(audit table, {pk:'log'}, ConsistentRead=True) -> against AWS the control (-1) leg's entries cover {0,1,2,3} and 99 within a couple of minutes; against the emulator nothing ever appears for either leg and the check is killed at the 60 s cap.
9. Then assert the subject leg contributed no entries for its table over a hold window of max(180 s, 5x the control's latency), that GetEventSourceMapping(UUID=<subject>) reports State 'Enabled' and MaximumRecordAgeInSeconds == 60, and that GetItem(ConsistentRead=True) on each twin returns v=99 — none of which the emulator reaches.
