Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodbstreams-get-shard-iterator-at-timestamp-bounds
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodbstreams-get-shard-iterator-at-timestamp-bounds dynamodbstreams-shard-iterator-request-validation dynamodbstreams-stream-shard-page-walk
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodbstreams|GetShardIterator|4xx->2xx|$.__type
Issue-Key: dynamodbstreams|GetShardIterator|4xx->2xx|$.message
Issue-Key: dynamodbstreams|GetShardIterator|4xx->2xx|status
Issue-Key: dynamodbstreams|GetRecords|4xx->2xx|$.__type
Issue-Key: dynamodbstreams|GetRecords|4xx->2xx|$.message
Issue-Key: dynamodbstreams|GetRecords|4xx->2xx|status
Issue-Key: dynamodbstreams|DescribeStream|4xx->2xx|$.__type
Issue-Key: dynamodbstreams|DescribeStream|4xx->2xx|$.message
Issue-Key: dynamodbstreams|DescribeStream|4xx->2xx|status
Issue-Commit: 9ba218dfac78ec8aaaf32d0b8457457c6bd04b91

# DescribeStream, GetRecords and GetShardIterator skip documented parameter upper-bound refusals

> _Note: this issue was identified against the code base at commit `9ba218dfac78ec8aaaf32d0b8457457c6bd04b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetShardIterator

## Description
Three request-parameter checks that DynamoDB Streams enforces are absent from the emulator: each request AWS refuses with HTTP 400 ValidationException is instead served a normal HTTP 200 success body. The three causes share one fault — the per-operation input-validation layer implements only the modeled lower bounds and none of the service-enforced upper bounds / temporal bounds, so the whole 400 ValidationException path of these operations is unreachable. All three must be closed before submitting.

Cause 1 of 3 - GetShardIterator accepts an AT_TIMESTAMP Timestamp in the future.
AWS: GetShardIterator with ShardIteratorType=AT_TIMESTAMP and a Timestamp greater than the current time is refused. Smithy: GetShardIterator.ShardIteratorType is the ShardIteratorType enum (TRIM_HORIZON, LATEST, AT_SEQUENCE_NUMBER, AFTER_SEQUENCE_NUMBER, AT_TIMESTAMP) and GetShardIterator.Timestamp targets Timestamp with no modeled range, so the bound is service-enforced and the wording is recorded behavior only. The recording shows HTTP 400 with __type 'com.amazon.coral.validate#ValidationException' and message 'One or more parameter values were invalid: Timestamp parameter cannot be greater than the current timestamp'. That entire message is fixed text for this error class — it has no variable parts taken from the request.
Emulator: answers HTTP 200 and mints a shard iterator; no __type and no message member are served at all.
Why: a client that computes a start point from a skewed or mis-signed clock gets an iterator instead of a refusal, then reads records from a position it never asked for; and any client-side handling of the ValidationException branch is dead code against the emulator.
Fix: GetShardIterator's input validation (the AT_TIMESTAMP branch must compare Timestamp against the current time and raise ValidationException).
Traces: dynamodbstreams-get-shard-iterator-at-timestamp-bounds#task#5

Cause 2 of 3 - GetRecords accepts a Limit above the service maximum of 1000.
AWS: Smithy: GetRecords.Limit targets PositiveIntegerObject with @range min 1 and no modeled maximum; the service additionally caps it at 1000. The recording shows HTTP 400 with __type 'com.amazon.coral.validate#ValidationException' and message 'One or more parameter values were invalid: Limit should be less than or equal to 1000'. The whole message is fixed text; 1000 is the service limit, not a value echoed from the request.
Emulator: answers HTTP 200 with a records page; no __type and no message are served.
Why: a client that over-requests silently gets a non-conforming page size instead of the refusal AWS gives, so the cap it must implement itself is never surfaced.
Fix: GetRecords' input validation (upper bound on Limit, raised as ValidationException).
Traces: dynamodbstreams-shard-iterator-request-validation#task#8

Cause 3 of 3 - DescribeStream accepts a Limit above the service maximum of 100.
AWS: Smithy: DescribeStream.Limit targets PositiveIntegerObject with @range min 1 and no modeled maximum; the service caps shard pages at 100. The recording shows HTTP 400 with __type 'com.amazon.coral.validate#ValidationException' and message 'One or more parameter values were invalid: Limit cannot be greater than the maximum: 100'. All of it is fixed text with 100 as the service limit. Note the wording is NOT the same sentence GetRecords uses for its own cap ('Limit should be less than or equal to 1000'): the two are distinct per-operation fixed texts and must not be unified into one template.
Emulator: answers HTTP 200 with a StreamDescription; no __type and no message are served.
Why: shard-pagination code that relies on the refusal to detect an over-wide page request never sees it, and the emulator's page size silently disagrees with AWS's.
Fix: DescribeStream's input validation (upper bound on Limit, raised as ValidationException).
Traces: dynamodbstreams-stream-shard-page-walk#task#4

## Evidence
Cause 1 - fixture dynamodbstreams-get-shard-iterator-at-timestamp-bounds step task#5, GetShardIterator: expected status 400, got 200. Expected $.__type 'com.amazon.coral.validate#ValidationException' -> missing (None). Expected $.message 'One or more parameter values were invalid: Timestamp parameter cannot be greater than the current timestamp' -> missing (None).

Cause 2 - fixture dynamodbstreams-shard-iterator-request-validation step task#8, GetRecords: expected status 400, got 200. Expected $.__type 'com.amazon.coral.validate#ValidationException' -> missing (None). Expected $.message 'One or more parameter values were invalid: Limit should be less than or equal to 1000' -> missing (None).

Cause 3 - fixture dynamodbstreams-stream-shard-page-walk step task#4, DescribeStream: expected status 400, got 200. Expected $.__type 'com.amazon.coral.validate#ValidationException' -> missing (None). Expected $.message 'One or more parameter values were invalid: Limit cannot be greater than the maximum: 100' -> missing (None).

Smithy (services/service-com-amazonaws-dynamodbstreams-dynamodbstreams-20120810): GetShardIterator.Timestamp: Timestamp (no range); GetShardIterator.ShardIteratorType: enum incl. AT_TIMESTAMP; GetRecords.Limit and DescribeStream.Limit: PositiveIntegerObject @range min 1, no modeled max.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Cause 1: on a stream-enabled table, call GetShardIterator with ShardIteratorType=AT_TIMESTAMP and Timestamp set past the current time (fixture dynamodbstreams-get-shard-iterator-at-timestamp-bounds, step task#5). Observe 200 + iterator instead of 400 ValidationException.
3. Cause 2: call GetRecords with a valid ShardIterator and Limit=1001 (fixture dynamodbstreams-shard-iterator-request-validation, step task#8). Observe 200 instead of 400 ValidationException.
4. Cause 3: call DescribeStream with Limit=101 (fixture dynamodbstreams-stream-shard-page-walk, step task#4). Observe 200 instead of 400 ValidationException.
