Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object events__kinesis.kinesis-target-partition-key-path-routes-by-event-field
Issue-Key: events|GetShardIterator|NotImplemented
Issue-Key: events__kinesis|GetShardIterator|NotImplemented
Issue-Commit: f3bbb57dafae938115053b9b00b150b1f9a288bb

# events kinesis-target checks cannot be graded: kinesis GetShardIterator answers 501 NotImplemented, so no delivered record can be read

> _Note: this issue was identified against the code base at commit `f3bbb57dafae938115053b9b00b150b1f9a288bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetShardIterator

## Description
Cause 1 of 1: the Kinesis read path that every EventBridge Kinesis-target behavioural check terminates on is an unimplemented scaffold stub, so both kinesis-target checks abort before any EventBridge behaviour is observed.

AWS: Smithy (services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json) models kinesis GetShardIterator with required input members ShardId (ShardId, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) and ShardIteratorType (one of AFTER_SEQUENCE_NUMBER | AT_SEQUENCE_NUMBER | AT_TIMESTAMP | LATEST | TRIM_HORIZON), optional StreamName/StreamARN, and output member ShardIterator (ShardIterator, len 1..512) on HTTP 200. Its ONLY modelled failures are AccessDeniedException (400), DryRunOperationException (400), InvalidArgumentException (400), ProvisionedThroughputExceededException (400), ResourceNotFoundException (400) and InternalFailureException (500). So a GetShardIterator naming a stream that exists, a shard of that stream and ShardIteratorType TRIM_HORIZON must answer 200 with a ShardIterator that GetRecords then accepts; the only refusal AWS gives for a stream or shard that does not exist is 400 ResourceNotFoundException whose message is the modelled fixed text for that class with the offending stream name/ARN taken from the request. NotImplemented is not a member of this operation's error set at all.

Emulator: the call is refused with HTTP 501 and error code `NotImplemented`, message `operation GetShardIterator is scaffolded but not implemented in this emulator` — fixed stub text with only the operation name varying, carrying no request or state value. botocore surfaces it as `ClientError: An error occurred (NotImplemented) when calling the GetShardIterator operation: operation GetShardIterator is scaffolded but not implemented in this emulator`. The handler is still the generated scaffold: services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/get_shard_iterator.rs carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker (line 4) and its `handle` body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("GetShardIterator"))` (line 226).

Why: GetShardIterator is the only way to begin reading a Kinesis stream, so a client that writes to a stream — directly or by having EventBridge deliver to a Kinesis target — can never read anything back, and receives an error code its SDK error model does not list for this operation (so retry/except clauses keyed on ResourceNotFoundException or ProvisionedThroughputExceededException never match). Concretely, the two EventBridge behaviours these checks exist to grade are left entirely unverified: (1) KinesisParameters.PartitionKeyPath must put each delivered record under the JSON text of the value the path selects (a number 7 becomes the key `7`, an object becomes its compact JSON text, and a path selecting nothing still delivers the record, under the key `{}`, rather than failing the delivery or dead-lettering it); (2) a target carrying PartitionKeyPath routes by the selected envelope field while a sibling target with no KinesisParameters falls back to the event id. Both terminal reads are at the stream, so neither EventBridge rendering nor the no-dead-letter guarantee can be observed while this operation refuses.

Fix: implement GetShardIterator in its own handler file, services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/get_shard_iterator.rs, over the kinesis crate's stream/shard state (resolving StreamName or StreamARN, validating ShardId against that stream's shards, honouring every modelled ShardIteratorType, and refusing an unknown stream or shard with 400 ResourceNotFoundException instead of 501), and delete the SCAFFOLDED marker. The GetRecords read path the iterator feeds belongs in the same component and must be reachable for these checks to complete.

Note on existing issues: 20261025T070938Z-logs-cloudwatch-logs-6-net-new-root-causes-kinesis.md and 20261025T094106Z-kms-logs-the-kinesis-getsharditerator-stub-now-blocks.md already name this same stub, but for CloudWatch Logs subscription-filter checks; neither carries either EventBridge kinesis-target check id, so neither covers these replay keys. The replay report did not mark these two checks ALREADY FILED or OWNED, and no `Implement kinesis GetShardIterator` stub issue exists. The second check id in trace_ids belongs to the events__kinesis suite grouping, whose kinesis leg is still pending bootstrap (`Bootstrap the Kinesis emulator`, 20261021T210901Z-bootstrap-service-com-amazonaws-kinesis-kinesis-20131202.md, open); it is listed only so a fixer re-replays both recordings of this one cause.

## Evidence
Expected (AWS / Smithy): GetShardIterator on an existing stream+shard with ShardIteratorType=TRIM_HORIZON -> HTTP 200, body containing ShardIterator (len 1..512) that a subsequent GetRecords accepts; the only refusals possible are the modelled 400/500 errors listed above.

Actual (emulator replay): HTTP 501, error code `NotImplemented`, message `operation GetShardIterator is scaffolded but not implemented in this emulator`.

Replay excerpts:
- [error] events.kinesis-partition-key-path-renders-numbers-and-objects-as-json-and-an-absent-field-as-empty-object
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetShardIterator operation: operation GetShardIterator is scaffolded but not implemented in this emulator
- [error] events__kinesis.kinesis-target-partition-key-path-routes-by-event-field
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetShardIterator operation: operation GetShardIterator is scaffolded but not implemented in this emulator

Source evidence: services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/get_shard_iterator.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED:`) and line 226 (`Err(crate::scaffolded::Scaffolded::not_implemented("GetShardIterator"))`).

Message-shape note: the diverging message is 100% fixed stub text parameterised only by the operation name; AWS's message for the nearest legitimate refusal (ResourceNotFoundException) is that error class's fixed text with the stream name or ARN taken from the request, and no legitimate response here is an error at all for the streams these checks create.

## Steps to Reproduce
1. Create a Kinesis stream and wait for it to become ACTIVE.
2. Create a custom event bus, a rule matching the test source, and PutTargets with the stream ARN plus KinesisParameters.PartitionKeyPath (e.g. `$.detail.n`, `$.detail.o`, `$.detail.absent`), and a sibling target on the same rule/stream with no KinesisParameters.
3. PutEvents an event matching the rule.
4. Read the stream back: ListShards, then GetShardIterator(StreamName=<stream>, ShardId=<shard>, ShardIteratorType=TRIM_HORIZON).
5. Observed: step 4 fails with HTTP 501 `NotImplemented` (`operation GetShardIterator is scaffolded but not implemented in this emulator`). Expected: HTTP 200 with a ShardIterator, and a following GetRecords returning one record per target with the partition keys described above.

Replay the two checks in trace_ids to confirm a fix.
