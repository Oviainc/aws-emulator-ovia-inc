Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|GetRecords|NotImplemented
Issue-Commit: b3c7a1d3b5e40780898598751dac20435d474802

# events: the EventBridge Kinesis-target PartitionKeyPath rendering contract is unreachable — kinesis GetRecords answers NotImplemented, so no delivered record's partition key can ever be read

> _Note: this issue was identified against the code base at commit `b3c7a1d3b5e40780898598751dac20435d474802`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecords

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: the Kinesis data-plane read path is a scaffold stub, so EventBridge's PartitionKeyPath rendering for non-string and missing-field values is ungraded ===

AWS: Smithy (com.amazonaws.eventbridge#KinesisParameters) models a single required member PartitionKeyPath (@length 0..256) and documents it as "The JSON path to be extracted from the event and used as the partition key for the Amazon Kinesis data stream, so that you can control the shard to which the event goes. If you do not include this parameter, the default is to use the eventId as the partition key." Recorded behavior for the rendering rules the check grades (the model states the path semantics but not the stringification): the delivered record's PartitionKey is the JSON TEXT of the value the path selects from the envelope — a numeric 7 becomes the key `7`, an object {"a": 1, "b": [2]} becomes the COMPACT JSON text `{"a":1,"b":[2]}` (no spaces), and a path selecting nothing does not fail the delivery at all: the record is written under the key `{}`. Every record's Data is the full event envelope (source, detail-type and detail as published), and nothing reaches the targets' DeadLetterConfig queue. Observing any of that requires the Kinesis read path: GetShardIterator then GetRecords.
Emulator: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetRecords operation: operation GetRecords is scaffolded but not implemented in this emulator`. The check gets through kinesis CreateStream, the stream_exists waiter, AddTagsToStream, DescribeStreamSummary, ListShards and GetShardIterator, and through the whole EventBridge setup (CreateEventBus, PutRule, PutTargets with three KinesisParameters targets, FailedEntryCount 0) and PutEvents; it then fails on the first read of the stream, so no PartitionKey is ever observed and the three rendering cases cannot be distinguished from each other or from a delivery that never happened.
Why: PartitionKeyPath is how a producer controls shard affinity — it is the whole point of pointing an EventBridge rule at a stream rather than a queue. An implementation that only handles string-valued paths (stringifying a number with language-native formatting, serialising an object with spaces, or treating a missing path as a delivery failure that dead-letters) would be silently wrong here and no client could detect it: the control plane accepts the configuration and the data plane cannot be read.
Fix: the kinesis data-plane read path — GetRecords (and the GetShardIterator that feeds it) in services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/. Note that the kinesis service is still in this project's PENDING bootstrap set, so its bootstrap may resolve the blocking call; the EventBridge-side contract this check grades (PartitionKey == the compact JSON text of the selected value, `{}` for an absent path, the full envelope as Data, nothing dead-lettered) belongs to the eventbridge crate's Kinesis target branch and still needs a re-replay afterwards — which is why this is recorded against the `events` group, whose token the failing check carries, rather than left implicit. Precedent for recording a kinesis-stub-blocked contract against the consuming group: the open issue 20261025T094106Z-kms-logs-the-kinesis-getsharditerator-stub-now-blocks.md does exactly this for GetShardIterator under kms__logs.
Traces: events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object.

## Evidence
events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object [error]:

  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetRecords operation: operation GetRecords is scaffolded but not implemented in this emulator

Raised from the check's shard reader, on the first GetRecords(ShardIterator=<iterator from GetShardIterator TRIM_HORIZON>, Limit=100) — i.e. after the stream, the IAM role, the DLQ, the bus, the rule and the three targets were all created successfully and PutEvents returned FailedEntryCount 0.

Setup that succeeded: one rule on a custom bus with three targets on the SAME stream, same RoleArn, same DeadLetterConfig and RetryPolicy{MaximumRetryAttempts: 0, MaximumEventAgeInSeconds: 60}, differing ONLY in KinesisParameters.PartitionKeyPath — `$.detail.n`, `$.detail.o`, `$.detail.absent`. One event published with Detail {"n": 7, "o": {"a": 1, "b": [2]}, "m": <marker>}.

Expected: the stream holds three records for that event id, under partition keys exactly {`7`, `{"a":1,"b":[2]}`, `{}`} — the number as its JSON text, the object as COMPACT JSON text with no spaces, and the absent path as `{}` rather than a failed delivery — each record's Data being the published envelope with source, detail-type 'keyed' and detail equal to what was published, and the targets' DLQ holding no message for that event.
Actual: HTTP error code NotImplemented from GetRecords; no record is read, so no partition key is ever observed and none of the three rendering rules is graded.

## Steps to Reproduce
1. kinesis.create_stream(StreamName=s, ShardCount=1); wait for the stream_exists waiter; kinesis.describe_stream_summary(StreamName=s) -> StreamARN.
2. iam.create_role trusting events.amazonaws.com and iam.put_role_policy allowing kinesis:PutRecord/kinesis:PutRecords on that stream ARN.
3. sqs.create_queue for a DLQ; sqs.set_queue_attributes with a Policy allowing events.amazonaws.com sqs:SendMessage with Condition StringEquals aws:SourceAccount = <account>.
4. events.create_event_bus(Name=B); events.put_rule(Name=r, EventBusName=B, EventPattern='{"source":["<S>"]}').
5. events.put_targets(Rule=r, EventBusName=B, Targets=[three entries, all Arn=<stream ARN>, same RoleArn, same DeadLetterConfig and RetryPolicy{MaximumRetryAttempts:0, MaximumEventAgeInSeconds:60}, with KinesisParameters.PartitionKeyPath = '$.detail.n', '$.detail.o' and '$.detail.absent']) -> FailedEntryCount 0.
6. events.put_events(Entries=[{EventBusName:B, Source:'<S>', DetailType:'keyed', Detail:'{"n": 7, "o": {"a": 1, "b": [2]}, "m": "<marker>"}'}]) -> FailedEntryCount 0.
7. kinesis.list_shards(StreamName=s); kinesis.get_shard_iterator(StreamName=s, ShardId=<shard>, ShardIteratorType='TRIM_HORIZON') -> returns an iterator.
8. kinesis.get_records(ShardIterator=<iterator>, Limit=100) -> the emulator answers NotImplemented; AWS returns the three records. (Then: assert the partition keys are exactly {'7', '{"a":1,"b":[2]}', '{}'}, that each record's Data is the published envelope, and that the DLQ holds nothing for that event id.)

Replay events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object to confirm.
