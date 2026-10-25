Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodbstreams.closed-shard-get-records-ends-with-a-null-next-shard-iterator dynamodbstreams.disable-closes-shards-and-reenable-mints-new-stream dynamodbstreams.foreign-sequence-number-rejected dynamodbstreams.get-records-limit-pages-one-shard-without-gaps-or-repeats dynamodbstreams.iterator-types-select-expected-records dynamodbstreams.keys-only-view-omits-both-images dynamodbstreams.malformed-shard-iterator-rejected dynamodbstreams.new-image-view-omits-old-image dynamodbstreams.stream-record-creation-time-tracks-the-live-clock dynamodbstreams.describe-stream-shard-pagination-covers-all-shards
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodb|PutItem|ResourceNotFoundException
Issue-Key: dynamodbstreams.describe-stream-shard-pagination-covers-all-shards|run#7|AssertionError|assert desc["StreamStatus"] == "ENABLED", (
Issue-Commit: a0c9c8ccc8bc5abababeef7ae6993b0ce3338eb3

# A stream-enabled table is never registered on the DynamoDB plane (PutItem/DescribeTable answer ResourceNotFoundException) and its stream is stuck in StreamStatus ENABLING (2 causes)

> _Note: this issue was identified against the code base at commit `a0c9c8ccc8bc5abababeef7ae6993b0ce3338eb3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
This bundle has 2 causes; both must be closed before submitting.

Context: the open issue 20261023T110802Z-dynamodb-stream-enablement-is-unimplemented-createtable-with-streamspecification.md reported that CreateTable with StreamSpecification answered NotImplemented. That refusal is gone in this run — CreateTable now returns 200 with a LatestStreamArn and the dynamodbstreams surface resolves the ARN — so these are the next-layer defects behind it, not that refusal. The suite marked NONE of the ten checks below as ALREADY FILED or OWNED.

Cause 1 of 2: CreateTable with StreamSpecification mints and registers the STREAM but does not register the TABLE on the DynamoDB plane, so every subsequent data-plane/control-plane call for that table name is refused ResourceNotFoundException.
AWS: Smithy (com.amazonaws.dynamodb#DynamoDB_20120810): CreateTable returns 200 with a TableDescription; the named table is then addressable by PutItem (input member TableName required) and DescribeTable. ResourceNotFoundException (httpError 400) is modeled on both, but AWS raises it only for a table that does not exist — a table whose stream is already serving DescribeStream and GetShardIterator is, by construction, past CREATING, and real AWS answers PutItem/DescribeTable for it (the checks measured this on us-west-2, 2026-09-10: shards appear ~8 s after CreateTable, by which time the table is ACTIVE).
Emulator: in each of the nine checks below, CreateTable(StreamSpecification={StreamEnabled:true,StreamViewType:<view>}) succeeds and returns a LatestStreamArn, DescribeStream on that ARN reports shards, and GetShardIterator(TRIM_HORIZON) returns a usable iterator for every shard — and then the very first PutItem against the same TableName fails 400 ResourceNotFoundException ('Requested resource not found'). The stream half of the table exists while the table half does not. Hypothesis: the stream-enabled CreateTable path creates the stream registry entry and returns a TableDescription without inserting the table into the table/item store the data plane reads.
Why: a stream-enabled table is write-only-unusable — no client can put, update or read an item in it, and no change-data-capture topology can be exercised at all, because there is no way to produce a mutation to capture. Worse, the error code is the one clients treat as 'this resource does not exist', so SDK retry/creation logic concludes the CreateTable it just saw succeed did not happen, while DescribeStream simultaneously reports the table by name.
Fix: the DynamoDB CreateTable handler and the table state it writes — the StreamSpecification path must register the table itself (and its items store) exactly as the non-stream path does, so PutItem/UpdateItem/DescribeTable resolve it, and the table must reach ACTIVE rather than being resolvable only through the streams registry.
Traces: dynamodbstreams.closed-shard-get-records-ends-with-a-null-next-shard-iterator, dynamodbstreams.disable-closes-shards-and-reenable-mints-new-stream, dynamodbstreams.foreign-sequence-number-rejected, dynamodbstreams.get-records-limit-pages-one-shard-without-gaps-or-repeats, dynamodbstreams.iterator-types-select-expected-records, dynamodbstreams.keys-only-view-omits-both-images, dynamodbstreams.malformed-shard-iterator-rejected, dynamodbstreams.new-image-view-omits-old-image, dynamodbstreams.stream-record-creation-time-tracks-the-live-clock.

Cause 2 of 2: DescribeStream reports StreamStatus ENABLING for a stream that is already serving shard iterators, and never transitions to ENABLED.
AWS: Smithy (com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810): StreamDescription.StreamStatus targets the enum StreamStatus with values ENABLING | ENABLED | DISABLING | DISABLED, documented as 'ENABLING - Streams is currently being enabled on the DynamoDB table' and 'ENABLED - the stream is enabled'. ENABLING is the transient creation state; once the stream exposes shards and hands out iterators it is enabled and DescribeStream reports ENABLED (recorded on us-west-2, 2026-09-10: a freshly enabled stream on an empty PAY_PER_REQUEST table reports StreamStatus ENABLED with four shards).
Emulator: DescribeStream on a stream that just returned Shards and a working TRIM_HORIZON ShardIterator for every shard answers StreamStatus 'ENABLING'. The check polls DescribeStream until iterators are obtainable, so the value is not a race it observed too early — the status simply never becomes ENABLED.
Why: every stream consumer gates on StreamStatus before it starts reading (KCL, Lambda ESM creation, and hand-rolled pollers all treat ENABLING as 'not ready yet'). A status that never leaves ENABLING makes a readable stream look permanently unavailable, so consumers either spin in a readiness loop or refuse to attach, and code that branches ENABLED vs DISABLED cannot tell an enabling stream from a healthy one.
Fix: the DynamoDB Streams state that backs DescribeStream — the stream's status must advance to ENABLED when the stream is registered and serving (and, symmetrically, DISABLING/DISABLED on disable), instead of being pinned at its initial value.
Traces: dynamodbstreams.describe-stream-shard-pagination-covers-all-shards.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): Total 0/17 passed (0.0%); Mismatched 7, Errored 10, Skipped 0, Unloadable 0. Per-service: dynamodbstreams 0/11. No bootstrap signal — the emulator was serving (CreateTable, DescribeStream, GetShardIterator all answered 200).

Cause 1 — identical error on 9 dynamodbstreams checks, on the first PutItem after the stream was confirmed readable:
  File "context.py", line 123, in wrapper -> File "client.py", line 1094, in _make_api_call
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the PutItem operation: Requested resource not found
  Expected: PutItem 200 (the table was created in the same check and its stream is already serving DescribeStream + GetShardIterator).
  Actual: 400 ResourceNotFoundException.
  Sequence in each check (e.g. dynamodbstreams.iterator-types-select-expected-records): create_table(KeySchema [{pk,HASH}], AttributeDefinitions [{pk,S}], BillingMode PAY_PER_REQUEST, StreamSpecification {StreamEnabled:true, StreamViewType:NEW_IMAGE}) -> 200 with LatestStreamArn; _wait_stream_usable() returns, i.e. DescribeStream listed shards AND GetShardIterator(TRIM_HORIZON) succeeded for every shard; put_item(TableName=<that table>) -> ResourceNotFoundException.

Cause 2 — dynamodbstreams.describe-stream-shard-pagination-covers-all-shards:
  File "check_dynamodbstreams_describe_stream_shard_pagination_covers_all_shards.py", line 47, in run
    assert desc["StreamStatus"] == "ENABLED"
  AssertionError: a stream that hands out iterators must report ENABLED, got 'ENABLING'
  Expected: StreamStatus ENABLED. Actual: 'ENABLING', after the check's own poll loop had already obtained a TRIM_HORIZON iterator on every shard.

Table names, stream ARNs, stream labels, shard ids and sequence numbers are generated per run and are examples only; the requirements are PutItem/DescribeTable resolving a stream-enabled table (200, not 400 ResourceNotFoundException) and DescribeStream reporting StreamStatus ENABLED for a serving stream.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Cause 1: call DynamoDB CreateTable with KeySchema [{pk,HASH}], AttributeDefinitions [{pk,S}], BillingMode PAY_PER_REQUEST and StreamSpecification {StreamEnabled:true, StreamViewType:NEW_IMAGE}. Take LatestStreamArn from the response, call dynamodbstreams DescribeStream on it and GetShardIterator(ShardIteratorType=TRIM_HORIZON) on a returned ShardId (both succeed). Now call PutItem on the same TableName with Item {pk:{S:'iter-item'}} and observe 400 ResourceNotFoundException instead of 200. DescribeTable on the same name is refused the same way. Re-replay the nine check ids listed for Cause 1 to confirm.
3. Cause 2: with the same stream ARN from step 2, call DescribeStream and read StreamDescription.StreamStatus. Observe 'ENABLING' even though GetShardIterator already serves every shard; expected 'ENABLED'. Re-replay dynamodbstreams.describe-stream-shard-pagination-covers-all-shards.
4. After fixing, re-replay the full trace_ids list.
