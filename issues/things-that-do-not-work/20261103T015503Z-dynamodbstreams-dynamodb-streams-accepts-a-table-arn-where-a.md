Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodbstreams-onyx-traj-table-arn-where-the-stream-arn-belongs
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodbstreams-onyx-traj-table-arn-where-the-stream-arn-belongs
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodbstreams|DescribeStream|4xx->4xx|$.__type
Issue-Key: dynamodbstreams|DescribeStream|4xx->4xx|$.message
Issue-Key: dynamodbstreams|GetShardIterator|4xx->4xx|$.__type
Issue-Key: dynamodbstreams|ListStreams|4xx->4xx|$.message
Issue-Related: 20261029T172902Z-dynamodbstreams-constraint-violation-messages-interpolate-the-rejected-value-which.md
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# DynamoDB Streams accepts a table ARN where a stream ARN belongs instead of refusing it as invalid input

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStream

## Description
Three causes, all the same underlying fault: DynamoDB Streams does not validate the SHAPE of an ARN-ish identifier before it goes looking for the resource, so a syntactically invalid identifier is reported as a missing resource (or with a generic pattern message) instead of as invalid input. All three must be closed.

Cause 1 of 3 - DescribeStream given a TABLE ARN in StreamArn answers ResourceNotFoundException instead of ValidationException.
AWS: Smithy: DescribeStream.StreamArn is @required, targets StreamArn with @length min 37 max 1024; the ARN-shape rule itself is not modeled, so recorded behavior only. The recording shows HTTP 400 with __type 'com.amazon.coral.validate#ValidationException' and message 'Invalid StreamArn'. That message is entirely fixed text - it has no variable parts, and it does not echo the submitted value. The coral-validate ValidationException is the framework-level input refusal that runs BEFORE resource lookup; DescribeStream's only modeled client error, ResourceNotFoundException, is for a well-formed stream ARN that names no stream.
Emulator: HTTP 400 with __type 'ResourceNotFoundException' and message 'Requested resource not found: Stream: <the submitted arn> not found' - i.e. it skipped shape validation, did a lookup, missed, and reported the miss.
Why: an SDK's error-type switch takes the wrong branch. ResourceNotFoundException is a modeled, catchable DynamoDB Streams error that callers and waiters legitimately treat as 'not there yet / not there any more' and keep polling or fall back on; a malformed StreamArn is a permanent client fault that must surface as ValidationException so the request is fixed rather than retried. A caller that passed a table ARN by mistake is told the stream is absent instead of that its ARN is wrong.
Fix: DescribeStream's input validation - the StreamArn shape check must run before the stream lookup and raise ValidationException ('com.amazon.coral.validate#ValidationException', message 'Invalid StreamArn').
Traces: dynamodbstreams-onyx-traj-table-arn-where-the-stream-arn-belongs#task#5

Cause 2 of 3 - GetShardIterator given a TABLE ARN in StreamArn answers ResourceNotFoundException instead of ValidationException.
AWS: Smithy: GetShardIterator.StreamArn is @required, targets StreamArn with @length min 37 max 1024; errors are InternalServerError, ResourceNotFoundException, TrimmedDataAccessException. ARN-shape validation is recorded behavior only: HTTP 400, __type 'com.amazon.coral.validate#ValidationException', message 'Invalid StreamArn' - again all fixed text, no interpolated value.
Emulator: HTTP 400 with __type 'ResourceNotFoundException' and message 'Requested resource not found: Stream does not exist'.
Why: same as cause 1 - a permanent input fault is reported as a transient/absent-resource condition, so retry and waiter logic loops on a request that can never succeed, and no caller is told what is actually wrong with the ARN.
Fix: GetShardIterator's input validation, same StreamArn shape check, run before any shard/stream lookup.
Traces: dynamodbstreams-onyx-traj-table-arn-where-the-stream-arn-belongs#task#6

Cause 3 of 3 - ListStreams rejects an ARN-valued TableName with the wrong message template.
AWS: Smithy: ListStreams.TableName targets TableName with @length min 3 max 255 and @pattern ^[a-zA-Z0-9_.-]+$. The recording shows HTTP 400 (error class matched, only the text diverged) with message 'TableName must be at least 3 characters long and at most 255 characters long'. For this member DynamoDB Streams does NOT use the 'N validation error detected ...' envelope at all: the whole sentence is fixed text whose only variable parts are the two modeled length limits (3 and 255 from the model); the rejected value is not echoed.
Emulator: HTTP 400 with "1 validation error detected: Value '<the submitted arn>' at 'tableName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$" - the generic constraint envelope, the pattern clause instead of the length sentence, and the request's own value spliced in.
Why: a client or SDK test matching on the refusal text for an invalid table name sees a sentence AWS never emits for ListStreams, and the emulator reflects the caller's input back in an error message where AWS does not.
Fix: ListStreams input validation / the DynamoDB Streams validation-message formatter - TableName rejection must emit the dedicated length-worded message rather than the shared constraint envelope.
Traces: dynamodbstreams-onyx-traj-table-arn-where-the-stream-arn-belongs#task#7

Related but NOT covering these: open issue 20261029T172902Z-dynamodbstreams-constraint-violation-messages-interpolate-the-rejected-value-which.md is about dropping the interpolated value from the shared 'N validation error detected' envelope for GetRecords.Limit and GetShardIterator.SequenceNumber. Cause 3 here is a different template altogether (ListStreams.TableName does not use that envelope), and causes 1 and 2 are a wrong error CLASS, not a wrong message.

## Evidence
All three causes come from fixture dynamodbstreams-onyx-traj-table-arn-where-the-stream-arn-belongs, replayed against the emulator from trace set simparity-wire-aws-v1.

Cause 1 - step task#5, DescribeStream, status 400 both sides. $.__type value_mismatch: expected 'com.amazon.coral.validate#ValidationException', got 'ResourceNotFoundException'. $.message value_mismatch: expected 'Invalid StreamArn', got 'Requested resource not found: Stream: arn:aws:dynamodb:<region>:<account>:table/<table> not found'.

Cause 2 - step task#6, GetShardIterator, status 400 both sides. $.__type value_mismatch: expected 'com.amazon.coral.validate#ValidationException', got 'ResourceNotFoundException'. $.message value_mismatch: expected 'Invalid StreamArn', got 'Requested resource not found: Stream does not exist'.

Cause 3 - step task#7, ListStreams, status 400 both sides. $.message value_mismatch: expected 'TableName must be at least 3 characters long and at most 255 characters long', got "1 validation error detected: Value '<submitted table ARN>' at 'tableName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$".

Model citations: DescribeStream.StreamArn @required @length 37..1024, errors InternalServerError/ResourceNotFoundException; GetShardIterator.StreamArn @required @length 37..1024, errors InternalServerError/ResourceNotFoundException/TrimmedDataAccessException; ListStreams.TableName @length 3..255 @pattern ^[a-zA-Z0-9_.-]+$.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator; all three steps are in fixture dynamodbstreams-onyx-traj-table-arn-where-the-stream-arn-belongs.
2. Cause 1: create a table with a stream, then call DescribeStream with StreamArn set to the TABLE's ARN (no /stream/<label> suffix) - step task#5. Expect 400 ValidationException 'Invalid StreamArn'; observe ResourceNotFoundException.
3. Cause 2: call GetShardIterator with that same table ARN as StreamArn - step task#6. Expect 400 ValidationException 'Invalid StreamArn'; observe ResourceNotFoundException.
4. Cause 3: call ListStreams with TableName set to a table ARN - step task#7. Expect 400 with message 'TableName must be at least 3 characters long and at most 255 characters long'; observe the generic 'N validation error detected ... regular expression pattern' message with the submitted value interpolated.
