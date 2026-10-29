Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodbstreams-get-shard-iterator-at-timestamp-bounds
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodbstreams-get-shard-iterator-at-timestamp-bounds dynamodbstreams-get-shard-iterator-sequence-number-length-bounds dynamodbstreams-shard-iterator-request-validation dynamodbstreams-stream-disable-closes-shard dynamodbstreams-stream-enable-describe-and-iterate dynamodbstreams-get-records-idempotent-replay-and-limit-floor dynamodbstreams-get-shard-iterator-sequence-number-positive-path dynamodbstreams-stream-shard-page-walk
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodbstreams|DescribeStream|2xx->2xx|$.StreamDescription.KeySchema[*].__type
Issue-Key: dynamodbstreams|DescribeStream|2xx->2xx|$.StreamDescription.LastEvaluatedShardId
Issue-Key: dynamodbstreams|DescribeStream|2xx->2xx|$
Issue-Commit: 9ba218dfac78ec8aaaf32d0b8457457c6bd04b91

# DescribeStream projection omits KeySchema __type and never emits LastEvaluatedShardId

> _Note: this issue was identified against the code base at commit `9ba218dfac78ec8aaaf32d0b8457457c6bd04b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStream

## Description
Every successful DescribeStream response the emulator serves is structurally short of the AWS one in the same two ways, and the shard-paging member it omits never appears no matter how long a client polls. Status is 200 on both sides; the defect is in the StreamDescription projection. Three causes, all must be closed.

Cause 1 of 3 - KeySchema elements carry no __type discriminator.
AWS: recorded behavior only - the Smithy model declares KeySchemaElement with AttributeName and KeyType and no discriminator trait, but DynamoDB Streams' awsJson responses tag each KeySchema entry with a __type member whose value is the shape id in DynamoDB's own namespace, 'com.amazonaws.dynamodb.v20120810#KeySchemaElement'. Every recorded DescribeStream response in the corpus carries it on every element. The member name and its value are fixed text; only the number of elements varies with the table's key schema.
Emulator: serializes each KeySchema entry as {"AttributeName": ..., "KeyType": ...} with the __type member absent.
Why: a client that dispatches on the __type discriminator (or byte-compares/schema-validates the response document) sees a different document than AWS returns for the same stream.
Fix: DescribeStream's StreamDescription serializer (nested KeySchemaElement projection).
Traces: dynamodbstreams-get-shard-iterator-at-timestamp-bounds#task#2 (cluster also covers dynamodbstreams-get-shard-iterator-sequence-number-length-bounds#task#2, dynamodbstreams-shard-iterator-request-validation#task#2, dynamodbstreams-stream-disable-closes-shard#task#2 and #task#4, dynamodbstreams-stream-enable-describe-and-iterate#task#3); also dynamodbstreams-get-records-idempotent-replay-and-limit-floor#task#4

Cause 2 of 3 - StreamDescription.LastEvaluatedShardId is never served.
AWS: Smithy: StreamDescription.LastEvaluatedShardId targets ShardId (@length min 28 max 65), documented as "The shard ID of the item where the operation stopped, inclusive of the previous result set. Use this value to start a new operation, excluding this value in the new request" and "If LastEvaluatedShardId is empty, then the 'last page' of results has been processed". The recorded DescribeStream responses carry the member (its value is a generated shard id and therefore not itself a requirement).
Emulator: the member is absent from the StreamDescription on every DescribeStream response.
Why: absence is AWS's signal for "this was the last page", so a client paging shards with ExclusiveStartShardId concludes it has seen every shard after one call and can never walk a stream whose shard list spans pages - shards, and the records in them, are silently lost.
Fix: DescribeStream's handler shard-paging logic plus its StreamDescription serializer (emit the member when a further page exists).
Traces: dynamodbstreams-get-shard-iterator-at-timestamp-bounds#task#2, dynamodbstreams-stream-shard-page-walk#task#2

Cause 3 of 3 - the shard page walk never converges, so the omission is not a timing artifact.
AWS: the page-walk recording reaches a DescribeStream response that carries LastEvaluatedShardId alongside the stream's shard list.
Emulator: the fixture polls DescribeStream for LastEvaluatedShardId to appear and it is still absent after 61 attempts / a 60s cap, with the served document otherwise complete (StreamArn, StreamLabel, StreamStatus ENABLED, StreamViewType, CreationRequestDateTime, TableName, KeySchema, one Shard). So the member is structurally never produced rather than eventually consistent.
Why: a client that waits for the paging cursor before advancing hangs for its whole timeout and then proceeds as if the stream had a single page of shards.
Fix: same component as cause 2 - the shard-paging state that should produce the cursor; re-replay the page-walk fixture to confirm the poll converges.
Traces: dynamodbstreams-stream-shard-page-walk#task#2

## Evidence
Cause 1 - fixture dynamodbstreams-get-shard-iterator-at-timestamp-bounds step task#2, DescribeStream, status 200 both sides: $.StreamDescription.KeySchema[0].__type missing (expected 'com.amazonaws.dynamodb.v20120810#KeySchemaElement', got None). Cluster affects 6 steps (dynamodbstreams-get-shard-iterator-sequence-number-length-bounds#task#2, dynamodbstreams-shard-iterator-request-validation#task#2, dynamodbstreams-stream-disable-closes-shard#task#2, dynamodbstreams-stream-disable-closes-shard#task#4, dynamodbstreams-stream-enable-describe-and-iterate#task#3). A second cluster reports the same missing __type at dynamodbstreams-get-records-idempotent-replay-and-limit-floor#task#4 and dynamodbstreams-get-shard-iterator-sequence-number-positive-path#task#4.

Cause 2 - same step: $.StreamDescription.LastEvaluatedShardId missing (expected '<shaped-not-asserted>', got None).

Cause 3 - fixture dynamodbstreams-stream-shard-page-walk step task#2, DescribeStream: '$: poll capped at 60s' and '$.StreamDescription.LastEvaluatedShardId: poll: still absent after 61 attempt(s)'. Served body: {StreamArn, StreamLabel, StreamStatus ENABLED, StreamViewType KEYS_ONLY, CreationRequestDateTime, TableName, KeySchema [{AttributeName, KeyType}], Shards [{ShardId, SequenceNumberRange}]} - no LastEvaluatedShardId, no KeySchema __type.

Smithy: StreamDescription.LastEvaluatedShardId -> ShardId @length 28..65, documented as the shard-paging cursor; KeySchemaElement = {AttributeName, KeyType}.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Create a table with a stream enabled, then call DescribeStream on its StreamArn (fixture dynamodbstreams-get-shard-iterator-at-timestamp-bounds, step task#2).
3. Causes 1 and 2: inspect $.StreamDescription.KeySchema[0] for the __type member and $.StreamDescription for LastEvaluatedShardId - both are absent.
4. Cause 3: replay dynamodbstreams-stream-shard-page-walk step task#2, which polls DescribeStream for LastEvaluatedShardId; it never appears within the 60s cap.
