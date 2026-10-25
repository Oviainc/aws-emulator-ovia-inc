Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodbstreams.nonexistent-stream-arn-rejected
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodbstreams.nonexistent-stream-arn-rejected|run#13|AssertionError|assert message == "Requested resource not found: Stream does not exist", (
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# dynamodbstreams GetShardIterator reuses DescribeStream's ARN-interpolating ResourceNotFoundException message instead of the operation's own constant message (1 cause)

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetShardIterator

## Description
This issue has 1 cause.

Context: this is NOT covered by the open dynamodbstreams issue 20261025T011017Z-dynamodbstreams-a-stream-enabled-table-is-never-registered-on.md, which bundles the unregistered-table PutItem/DescribeTable refusal and the stuck StreamStatus ENABLING. This check got all the way past stream creation and readiness, matched the HTTP status and the error code on both refusals, and diverged only on GetShardIterator's message rule. The suite marked it neither ALREADY FILED nor OWNED.

Cause 1 of 1: the two dynamodbstreams operations that refuse an unknown stream ARN share one message formatter, so GetShardIterator emits DescribeStream's ARN-interpolating text.
AWS: Smithy (com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810): both DescribeStream and GetShardIterator model ResourceNotFoundException as a client error with httpError 400 (defaulted); the model carries only a generic message member and does not state the text, so the text itself is recorded behavior only. Recorded behavior (the check's MEASURED note, real AWS us-west-2): for a well-formed stream ARN whose table does not exist, DescribeStream answers 400 ResourceNotFoundException with 'Requested resource not found: Stream: <arn> not found', while GetShardIterator answers the SAME code with a DIFFERENT, request-independent message 'Requested resource not found: Stream does not exist'. The rule is therefore known on both sides: DescribeStream's message interpolates the requested ARN; GetShardIterator's is a fixed constant that does not.
Emulator: status and error code are correct on both calls (400 ResourceNotFoundException), and DescribeStream's message matches exactly. GetShardIterator, however, answered the DescribeStream form — 'Requested resource not found: Stream: arn:aws:dynamodb:<region>:<account>:table/<absent-table>/stream/<label> not found' — instead of the constant 'Requested resource not found: Stream does not exist'.
Why: the two messages are the client-visible way to tell the two refusals apart on a single error code. A consumer that distinguishes 'this stream ARN is unknown to GetShardIterator' (the documented transient during stream enablement, where GetShardIterator keeps answering 'Stream does not exist' for ~2 s after DescribeStream already reports shards) from 'the stream itself does not exist' matches on that message; receiving DescribeStream's ARN-bearing text makes the readiness transient indistinguishable from a permanently absent stream, and log/alarm patterns keyed on the constant never fire. It also leaks the requested ARN into a response where AWS does not echo it.
Fix: the dynamodbstreams GetShardIterator handler's not-found path — give it its own ResourceNotFoundException message rather than delegating to the shared DescribeStream formatter, so the message is the constant form with no ARN interpolation. Status (400) and code (ResourceNotFoundException) are already correct and must not change.
Traces: dynamodbstreams.nonexistent-stream-arn-rejected.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): Total 0/17 passed (0.0%). Per-service: dynamodbstreams 0/11 (0%). No bootstrap signal — the emulator was serving.

dynamodbstreams.nonexistent-stream-arn-rejected — [mismatch]:
  File "check_dynamodbstreams_nonexistent_stream_arn_rejected.py", line 66, in run
    assert message == "Requested resource not found: Stream does not exist"
  AssertionError: GetShardIterator on a stream ARN whose table does not exist answered message 'Requested resource not found: Stream: arn:aws:dynamodb:us-west-2:000000000000:table/sim2real-behav-dbsnf-6478e5dba139-absent/stream/2026-10-25T01:52:56.749 not found'

What already matched before this assertion (so the divergence is narrowly the GetShardIterator message rule):
  - CreateTable with StreamSpecification {StreamEnabled:true, StreamViewType:KEYS_ONLY} -> 200 with LatestStreamArn.
  - The check's readiness poll succeeded: DescribeStream listed shards AND GetShardIterator(TRIM_HORIZON) returned an iterator for every shard.
  - The bogus ARN differs from the real one in the TABLE component only; region, account, partition, resource type and timestamp label are well-formed.
  - DescribeStream(bogus ARN): code ResourceNotFoundException (match) AND message 'Requested resource not found: Stream: <bogus arn> not found' (match).
  - GetShardIterator(bogus ARN): code ResourceNotFoundException (match).

Expected: 400 ResourceNotFoundException with the request-independent message 'Requested resource not found: Stream does not exist'.
Actual: 400 ResourceNotFoundException with DescribeStream's ARN-interpolating message.

The ARN, region, account id, table name and stream label in the observed message are generated per run and are examples only; the requirement is that GetShardIterator's not-found message be the constant form that does not interpolate the requested ARN, while DescribeStream's does.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Call DynamoDB CreateTable with KeySchema [{pk,HASH}], AttributeDefinitions [{pk,S}], BillingMode PAY_PER_REQUEST and StreamSpecification {StreamEnabled:true, StreamViewType:KEYS_ONLY}; take LatestStreamArn and poll dynamodbstreams DescribeStream + GetShardIterator(TRIM_HORIZON) until a shard id and an iterator are obtainable.
3. Build a bogus ARN from the real one by changing ONLY the table component (append '-absent'), leaving partition, region, account, resource type and the /stream/<label> suffix intact.
4. Call DescribeStream(StreamArn=bogus). Observe 400 ResourceNotFoundException 'Requested resource not found: Stream: <bogus arn> not found' — correct today.
5. Call GetShardIterator(StreamArn=bogus, ShardId=<the real shard id>, ShardIteratorType=TRIM_HORIZON). Observe 400 ResourceNotFoundException whose message is the ARN-interpolating DescribeStream form; expected the constant 'Requested resource not found: Stream does not exist'.
6. Re-replay dynamodbstreams.nonexistent-stream-arn-rejected.
