Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object|run#19|Timeout|time.sleep(2)
Issue-Commit: a44b8b30e2b00548c30690158e9adbfe16c98744

# events: an EventBridge rule's Kinesis-data-stream targets never yield a readable record, so the PartitionKeyPath rendering contract stays ungraded (now a hang, not a stub error)

> _Note: this issue was identified against the code base at commit `a44b8b30e2b00548c30690158e9adbfe16c98744`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: a rule target whose Arn is a Kinesis data stream produces no record a consumer can read, so EventBridge's PartitionKeyPath stringification rules are never exercised ===

AWS: Smithy (services/service-com-amazonaws-eventbridge-awsevents/smithy-model/com-amazonaws-eventbridge-awsevents.json) models com.amazonaws.eventbridge#Target with required Id (@length 1..64, pattern ^[\.\-_A-Za-z0-9]+$) and Arn (@length 1..1600), optional RoleArn, DeadLetterConfig, RetryPolicy, and optional KinesisParameters. com.amazonaws.eventbridge#KinesisParameters has exactly one required member, PartitionKeyPath (target TargetPartitionKeyPath, @length 0..256), documented as "This object enables you to specify a JSON path to extract from the event and use as the partition key for the Amazon Kinesis data stream, so that you can control the shard to which the event goes. If you do not include this parameter, the default is to use the eventId as the partition key." So for every event a rule matches, each Kinesis target must result in one PutRecord onto the named stream, keyed by the value its PartitionKeyPath selects from the delivered envelope. Recorded behavior only, for the stringification rules the model does not state: the key is the JSON TEXT of the selected value — a numeric 7 becomes the key `7`; an object {"a": 1, "b": [2]} becomes the COMPACT JSON text `{"a":1,"b":[2]}` with no spaces; and a path that selects nothing does NOT fail the delivery — the record is written under the key `{}`. Each record's Data is the full delivered envelope (the published source, detail-type and detail), and the targets' DeadLetterConfig queue receives nothing. PutTargets itself answers 200 with FailedEntryCount 0; the delivery is asynchronous and observable only through the stream.

Emulator: the check is cut off by the suite's 60 s cap — verdict `mismatch`, detail `Check timed out after 60.0s`. The check's own loop re-publishes the event every 20 s and reads the shard every 2 s for up to 180 s, and it only exits that loop once one event id is present under all three partition keys; being cut at 60 s means no such record set ever appeared. Note this is a CHANGED failure mode for the same check: it previously terminated with `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetRecords operation: operation GetRecords is scaffolded but not implemented in this emulator` (HTTP 501), which is what the open issue 20261025T171339Z-events-the-eventbridge-kinesis-target-partitionkeypath-rendering-contract.md keys (Issue-Key `events|GetRecords|NotImplemented`). That refusal is no longer what the replay reports, so that issue's key no longer matches this failure and the replay report marked this check neither ALREADY FILED nor OWNED. Hypothesis (neither the model nor this recording states which half is at fault): either the eventbridge Kinesis target branch performs no PutRecord at all, or the kinesis read path still returns nothing for records EventBridge wrote; the recording only establishes that no record is ever readable.

Why: a client relying on AWS's behavior loses the entire reason to point a rule at a stream instead of a queue. Shard affinity is chosen by PartitionKeyPath, and a consumer that reads the stream to partition work by tenant/key sees nothing arrive; worse, because PutTargets and PutEvents both answer success (FailedEntryCount 0) and no message appears on the target's dead-letter queue, there is no signal anywhere that the delivery was dropped. An implementation that only handles string-valued paths — formatting a number with language-native text, serialising an object with spaces, or treating an absent path as a delivery failure that dead-letters — is likewise indistinguishable from a correct one while the data plane yields nothing.

Fix: the eventbridge crate's target-delivery path for Kinesis targets — the Target/KinesisParameters branch that must issue one PutRecord per matched event with PartitionKey = the compact JSON text of the value PartitionKeyPath selects (`{}` when the path selects nothing), Data = the delivered envelope, and no dead-letter message on success — together with whatever in the kinesis data-plane read path still prevents those records from being returned. File and line citations belong here only as pointers; see Traces for the failing check.

Related open issues that do NOT cover this failure: 20261025T171339Z-events-the-eventbridge-kinesis-target-partitionkeypath-rendering-contract.md names this same check but keys the GetRecords NotImplemented refusal, which the current replay no longer shows; 20261025T151735Z-events-kinesis-target-checks-cannot-be-graded-kinesis.md keys a GetShardIterator stub; 20261025T002357Z-events-eventbridge-target-delivery-never-completes-for-sns-step.md covers SNS, Step Functions, cross-region bus, input-transformer, trace-header and fan-out targets, not the Kinesis target branch.

Traces: events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object.

## Evidence
events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object [mismatch]:

  Check timed out after 60.0s

Setup the check performs: one Kinesis stream; an IAM role trusting events.amazonaws.com with kinesis:PutRecord/PutRecords on the stream ARN; an SQS dead-letter queue with an events.amazonaws.com sqs:SendMessage policy; CreateEventBus; PutRule with EventPattern {"source":[<unique source>]}; PutTargets with THREE targets, all Arn=<stream ARN>, same RoleArn, same DeadLetterConfig and RetryPolicy{MaximumRetryAttempts: 0, MaximumEventAgeInSeconds: 60}, differing only in KinesisParameters.PartitionKeyPath = "$.detail.n", "$.detail.o", "$.detail.absent" (the check asserts PutTargets returned FailedEntryCount 0). PutEvents then publishes Detail {"n": 7, "o": {"a": 1, "b": [2]}, "m": <marker>} and asserts FailedEntryCount 0.

Expected (AWS): within the check's 180 s budget the stream holds three records for one published event id, under partition keys exactly {`7`, `{"a":1,"b":[2]}`, `{}`} — the number as its JSON text, the object as compact JSON text with no spaces, the absent path as `{}` rather than a dead-lettered failure — each record's Data being the published envelope (source, detail-type "keyed", detail as published), and the targets' dead-letter queue holding no message for that event.
Actual (emulator): the check never leaves its publish/read loop and is terminated by the suite's 60 s cap; no record is ever observed under any of the three keys, so none of the three rendering rules is graded. The variable parts above (the stream name, bus name, source, event ids and the `m` marker) are per-run values, not part of the requirement; the fixed requirement is the three partition-key texts, the envelope as Data, and an empty DLQ.

Failure-mode change for the same check: the previous replay of this check ended in `An error occurred (NotImplemented) when calling the GetRecords operation: operation GetRecords is scaffolded but not implemented in this emulator` (HTTP 501, error code NotImplemented, message fixed stub text parameterised only by the operation name). The current replay shows no such error — the check hangs instead — so the already-open issue keyed on that refusal does not describe what now fails.

## Steps to Reproduce
1. kinesis.create_stream(StreamName=s, ShardCount=1); wait for the stream to exist; kinesis.describe_stream_summary(StreamName=s) -> StreamARN.
2. iam.create_role trusting events.amazonaws.com; iam.put_role_policy allowing kinesis:PutRecord and kinesis:PutRecords on that stream ARN.
3. sqs.create_queue (DLQ); sqs.set_queue_attributes with a Policy allowing Principal Service events.amazonaws.com to sqs:SendMessage on the queue ARN.
4. events.create_event_bus(Name=B); events.put_rule(Name=r, EventBusName=B, EventPattern='{"source":["<S>"]}').
5. events.put_targets(Rule=r, EventBusName=B, Targets=[three entries, all Arn=<stream ARN>, same RoleArn, same DeadLetterConfig{Arn:<dlq arn>} and RetryPolicy{MaximumRetryAttempts:0, MaximumEventAgeInSeconds:60}, with KinesisParameters.PartitionKeyPath = '$.detail.n', '$.detail.o' and '$.detail.absent']) -> expect FailedEntryCount 0.
6. events.put_events(Entries=[{EventBusName:B, Source:'<S>', DetailType:'keyed', Detail:'{"n": 7, "o": {"a": 1, "b": [2]}, "m": "<marker>"}'}]) -> expect FailedEntryCount 0. Re-publish every ~20 s while polling, to absorb rule/role activation.
7. kinesis.list_shards / get_shard_iterator(ShardIteratorType='TRIM_HORIZON') then get_records in a loop for up to 180 s.
8. Observe: no record for the published event id ever appears under any partition key, so the assertion that the key set equals {'7', '{"a":1,"b":[2]}', '{}'} is never reached and the check is cut off by the suite's 60 s cap. On AWS the three records are present and carry exactly those keys, with the full envelope as Data and nothing on the DLQ.

Replay events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object to confirm a fix.
