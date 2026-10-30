Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-role-without-stream-read-rejected
Issue-Key: cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-role-without-stream-read-rejected|_create_error#6|AssertionError|raise AssertionError(
Issue-Commit: bb4602e7ae983d0b86fd9b19deccaf384a5ade46

# cloudwatch__kinesis__lambda: CreateEventSourceMapping never authorizes the function's execution role against the Kinesis stream — a mapping for a role with no stream-read permissions is accepted 202 instead of refused InvalidParameterValueException (1 cause)

> _Note: this issue was identified against the code base at commit `bb4602e7ae983d0b86fd9b19deccaf384a5ade46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: Lambda does not evaluate the function's execution-role policy against the Kinesis event source at CreateEventSourceMapping time, so an unreadable stream is wired up successfully.

  AWS: Smithy: Lambda `CreateEventSourceMapping` models `InvalidParameterValueException` at httpError 400 (alongside ResourceConflictException/ResourceNotFoundException/TooManyRequestsException/ServiceException). At create time Lambda checks that the function's execution role can read the named stream and, when it cannot, refuses 400 `InvalidParameterValueException` with the message rule `Cannot access stream <stream ARN>. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, DescribeStreamSummary, ListShards, and ListStreams Actions on your stream.` (built from the request, so the rule is the requirement) and creates NO mapping. Granting exactly kinesis:GetRecords, GetShardIterator, DescribeStream, DescribeStreamSummary, ListShards and ListStreams to the same role — nothing else changed — makes the identical call succeed.
  Emulator: accepts the create and answers HTTP 202 with a full mapping body (UUID, State='Creating', StateTransitionReason='USER_INITIATED', EventSourceArn, FunctionArn, EventSourceMappingArn), for a function whose execution role carries only logs permissions. The refusal never happens, so the create is not atomic either — the mapping is left on the function.
  Why: the emulator is over-permissive on the exact guard that tells a customer their trigger will never fire. On AWS, a role missing the stream-read actions is caught synchronously at create; here the mapping is created and silently never delivers, so the whole class of IAM-misconfiguration failures becomes invisible and the negative half of every ESM-authorization test passes vacuously. Any emulator that treats every credential as root will also pass the positive control, so this cannot be detected from the success path.
  Fix: the CreateEventSourceMapping handler's validation — add the execution-role authorization check against the event source (and make the refusal atomic, leaving no mapping) before the mapping is persisted.
  Traces: cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-role-without-stream-read-rejected

Note for the fixer: this is the Lambda-side authorization gate, not a Kinesis data-plane gap — the stream itself was created successfully and the check never reaches PutRecord.

## Evidence
Check cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-role-without-stream-read-rejected.

Expected (real AWS): CreateEventSourceMapping is refused with code `InvalidParameterValueException` and the message rule `Cannot access stream <stream ARN>. Please ensure the role can perform the GetRecords, GetShardIterator, DescribeStream, DescribeStreamSummary, ListShards, and ListStreams Actions on your stream.`, and ListEventSourceMappings on the function returns [].

Actual (emulator): HTTP 202 with a created mapping —
  AssertionError: mapping was ACCEPTED with a role that cannot read the stream; real AWS rejects it: {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'UUID': '81d7a503-421b-42f9-aa9b-8ad54728f2f2', 'StartingPosition': 'TRIM_HORIZON', 'BatchSize': 100, 'EventSourceArn': 'arn:aws:kinesis:us-west-2:000000000000:stream/sim2real-xsvc-obs2neg-...', 'FunctionArn': 'arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-obs2neg-...', 'State': 'Creating', 'StateTransitionReason': 'USER_INITIATED', 'EventSourceMappingArn': 'arn:aws:lambda:...:event-source-mapping:81d7a503-...'}

The UUIDs, generated names and timestamps are per-run; the requirement is the 400 InvalidParameterValueException refusal and the absence of any mapping.

## Steps to Reproduce
1. Boot the emulator.
2. Create a Kinesis stream.
3. Create an IAM role trusted by lambda.amazonaws.com whose only policy grants CloudWatch Logs actions — NO kinesis:GetRecords / GetShardIterator / DescribeStream / DescribeStreamSummary / ListShards / ListStreams.
4. CreateFunction (python3.12) with that role and wait for Active.
5. Call CreateEventSourceMapping(FunctionName=fn, EventSourceArn=<stream ARN>, StartingPosition='TRIM_HORIZON', BatchSize=100, Enabled=False).
   Expected: 400 InvalidParameterValueException, 'Cannot access stream ...'; ListEventSourceMappings(FunctionName=fn) == [].
   Observed: HTTP 202 and a mapping in state 'Creating'.
6. Positive control: attach the six kinesis read actions to the same role and repeat the identical call — it must succeed and reach state Disabled.
