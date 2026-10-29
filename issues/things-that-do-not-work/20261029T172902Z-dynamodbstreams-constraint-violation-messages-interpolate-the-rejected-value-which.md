Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodbstreams-get-records-idempotent-replay-and-limit-floor
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodbstreams-get-records-idempotent-replay-and-limit-floor dynamodbstreams-get-shard-iterator-sequence-number-length-bounds dynamodbstreams-shard-iterator-request-validation
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodbstreams|GetRecords|4xx->4xx|$.message
Issue-Key: dynamodbstreams|GetShardIterator|4xx->4xx|$.message
Issue-Commit: 9ba218dfac78ec8aaaf32d0b8457457c6bd04b91

# Constraint-violation messages interpolate the rejected value, which DynamoDB Streams omits

> _Note: this issue was identified against the code base at commit `9ba218dfac78ec8aaaf32d0b8457457c6bd04b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecords

## Description
Both causes are the same formatter defect: the emulator's constraint-violation message builder emits the AWS 'N validation error detected' envelope but inserts the rejected request value into it ("Value '<v>' at '<member>'"), whereas DynamoDB Streams words this error class WITHOUT the value ("Value at '<member>'"). Status (400) and the error class are correct; only the message text diverges. Two causes, both must be closed.

Fixed text vs variable parts for this error class as AWS words it: the fixed text is "<n> validation error detected: Value at '<member>' failed to satisfy constraint: Member must have <constraint clause>"; the variable parts are the wire member name and the modeled constraint limit taken from the Smithy model. The rejected value from the request is not part of the template at all.

Cause 1 of 2 - GetRecords Limit below the modeled minimum.
AWS: Smithy: GetRecords.Limit targets PositiveIntegerObject with @range min 1, which supplies the constraint clause. The recording shows HTTP 400 with message "1 validation error detected: Value at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1" - 'limit' is the wire member name, 1 is the modeled minimum, and nothing from the request appears.
Emulator: HTTP 400 with "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1" - it splices the rejected request value in before "at 'limit'".
Why: a client or SDK test that matches on the refusal text sees a sentence AWS never emits, and echoing the submitted value back into the message is a shape AWS deliberately does not use for this class.
Fix: the shared constraint-violation message formatter used by DynamoDB Streams input validation - drop the value interpolation for range violations.
Traces: dynamodbstreams-get-records-idempotent-replay-and-limit-floor#task#8

Cause 2 of 2 - GetShardIterator SequenceNumber below the modeled minimum length.
AWS: Smithy: GetShardIterator.SequenceNumber targets SequenceNumber with @length min 21 max 40, which supplies the constraint clause. The recording shows HTTP 400 with message "1 validation error detected: Value at 'sequenceNumber' failed to satisfy constraint: Member must have length greater than or equal to 21" - 'sequenceNumber' is the wire member name and 21 the modeled minimum length; the submitted string does not appear.
Emulator: HTTP 400 with "1 validation error detected: Value '12345' at 'sequenceNumber' failed to satisfy constraint: Member must have length greater than or equal to 21" - again the request's own value is spliced in (the quoted literal is whatever short string the request carried, not a constant).
Why: same as cause 1 - the refusal text does not match AWS for any short sequence number, so text-matching clients mis-handle a correctly classified error.
Fix: the same constraint-violation message formatter, length-violation branch.
Traces: dynamodbstreams-get-shard-iterator-sequence-number-length-bounds#task#4 (cluster also covers task#5 and task#6 of that fixture and dynamodbstreams-shard-iterator-request-validation#task#5)

## Evidence
Cause 1 - fixture dynamodbstreams-get-records-idempotent-replay-and-limit-floor step task#8, GetRecords, status 400 both sides. $.message value_mismatch: expected "1 validation error detected: Value at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1", got "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1".

Cause 2 - fixture dynamodbstreams-get-shard-iterator-sequence-number-length-bounds step task#4, GetShardIterator, status 400 both sides. $.message value_mismatch: expected "1 validation error detected: Value at 'sequenceNumber' failed to satisfy constraint: Member must have length greater than or equal to 21", got "1 validation error detected: Value '12345' at 'sequenceNumber' failed to satisfy constraint: Member must have length greater than or equal to 21". Same cluster: dynamodbstreams-get-shard-iterator-sequence-number-length-bounds#task#5, #task#6, dynamodbstreams-shard-iterator-request-validation#task#5 (4 steps).

Smithy: GetRecords.Limit PositiveIntegerObject @range min 1; GetShardIterator.SequenceNumber @length min 21 max 40.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Cause 1: call GetRecords with a valid ShardIterator and Limit=0 (fixture dynamodbstreams-get-records-idempotent-replay-and-limit-floor, step task#8) and compare the $.message text.
3. Cause 2: call GetShardIterator with ShardIteratorType=AT_SEQUENCE_NUMBER and a SequenceNumber shorter than 21 characters (fixture dynamodbstreams-get-shard-iterator-sequence-number-length-bounds, step task#4) and compare the $.message text.
