Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.stream-mapping-rejects-a-table-arn-in-place-of-its-stream-arn dynamodb__lambda.stream-mapping-requires-a-starting-position dynamodb__lambda.stream-mappings-round-trip-the-stack-settings-and-name-the-regression-table
Issue-Key: dynamodb__lambda.stream-mapping-rejects-a-table-arn-in-place-of-its-stream-arn|run#9|AssertionError|assert msg == want_msg, f"got {msg!r}, want {want_msg!r}"
Issue-Key: dynamodb__lambda.stream-mapping-requires-a-starting-position|run#11|AssertionError|assert (code, msg) == ("InvalidParameterValueException", expected), f"got {code!r}: {msg!r}"
Issue-Key: dynamodb__lambda.stream-mappings-round-trip-the-stack-settings-and-name-the-regression-table|run#8|AssertionError|assert got == {
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# dynamodb__lambda: 3 net-new causes in DynamoDB-stream event source mappings — two refusal messages diverge from AWS and TumblingWindowInSeconds loses its 0 default

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Three distinct root causes under the suite heading `dynamodb__lambda`, all in lambda's event-source-mapping surface for a DynamoDB stream source. A fixer must close all three before submitting.

Cause 1 of 3: CreateEventSourceMapping given a TABLE ARN where a STREAM ARN is required answers the generic "unrecognized event source" catalogue message instead of AWS's DynamoDB-specific one.
AWS: Smithy: lambda CreateEventSourceMapping models InvalidParameterValueException (400) and its `EventSourceArn` member is a free-form ARN, so the shape is enforced per source service at runtime. For a `arn:aws:dynamodb:<region>:<account>:table/<TableName>` with no `/stream/<StreamId>` suffix AWS answers InvalidParameterValueException with the message `Unrecognized event source. Expected arn of format arn:aws:dynamodb:<region>:<account-id>:table/TableName/stream/StreamId` — the fixed text is the whole sentence; only the region/account in the example template vary by caller region.
Emulator: same code, wrong message — `Unrecognized event source '<the table arn>'. An event source mapping polls an Amazon Kinesis data stream or stream consumer, an Amazon DynamoDB stream, an Amazon Simple Queue Service queue, an Amazon Managed Streaming for Apache Kafka cluster or VPC connection, an Amazon MQ broker or an Amazon DocumentDB change stream`.
Why: the message is the only thing that tells a caller WHICH ARN shape was expected; a client that pattern-matches AWS's wording (or a human reading the error) is told nothing about the missing `/stream/` segment.
Fix: the CreateEventSourceMapping validation path for DynamoDB sources — the per-source-service ARN-shape error, not the generic catalogue fallback.
Traces: dynamodb__lambda.stream-mapping-rejects-a-table-arn-in-place-of-its-stream-arn

Cause 2 of 3: CreateEventSourceMapping with a stream source and no StartingPosition answers a hand-written sentence instead of AWS's validation-error template.
AWS: Smithy: `StartingPosition` is optional in the model (it is mandatory only for stream sources), so AWS enforces it as a request validation error: InvalidParameterValueException with message `1 validation error detected: Value null at 'startingPosition' failed to satisfy constraint: Member must not be null.` — that whole sentence is fixed text; the member path `startingPosition` is the model member name.
Emulator: correct code, wrong message — `StartingPosition must be specified for the event source '<stream arn>'`.
Why: AWS's validation-error template is machine-parsed by tooling that extracts the failing member path; the emulator's sentence carries no member path and an unrelated ARN value, so such tooling misreports the fault.
Fix: the CreateEventSourceMapping input validation/serializer — emit the standard `N validation error detected: Value ... at '<member>' failed to satisfy constraint: ...` form for the missing required-for-stream member.
Traces: dynamodb__lambda.stream-mapping-requires-a-starting-position

Cause 3 of 3: a DynamoDB-stream mapping's `TumblingWindowInSeconds` reads back as absent/null instead of its 0 default.
AWS: Smithy: `EventSourceMappingConfiguration.TumblingWindowInSeconds` has range min 0 / max 900; for a stream mapping created without it AWS reports `TumblingWindowInSeconds: 0` on Create/Get/ListEventSourceMappings, alongside the other defaults the same read carries (BatchSize 1 as requested, MaximumRetryAttempts -1/3 as configured, MaximumBatchingWindowInSeconds 0, ParallelizationFactor 1, MaximumRecordAgeInSeconds -1, BisectBatchOnFunctionError false, FunctionResponseTypes []).
Emulator: the same read returns `'TumblingWindowInSeconds': None` — the member is omitted from the response — while every other field in the projection matches.
Why: a client reading the mapping back to compare with its desired state sees a missing field rather than 0, so round-trip/drift comparisons and typed SDK consumers that expect an int diverge.
Fix: the event-source-mapping state/serializer — default `TumblingWindowInSeconds` to 0 for stream sources and emit it in the Create/Get/List projection.
Traces: dynamodb__lambda.stream-mappings-round-trip-the-stack-settings-and-name-the-regression-table

## Evidence
Cause 1 — expected (code, message): ('InvalidParameterValueException', 'Unrecognized event source. Expected arn of format arn:aws:dynamodb:us-west-2:123456789012:table/TableName/stream/StreamId'). Actual: ('InvalidParameterValueException', "Unrecognized event source '<table-arn>'. An event source mapping polls an Amazon Kinesis data stream or stream consumer, an Amazon DynamoDB stream, an Amazon Simple Queue Service queue, an Amazon Managed Streaming for Apache Kafka cluster or VPC connection, an Amazon MQ broker or an Amazon DocumentDB change stream").
Cause 2 — expected: ('InvalidParameterValueException', "1 validation error detected: Value null at 'startingPosition' failed to satisfy constraint: Member must not be null."). Actual: ('InvalidParameterValueException', "StartingPosition must be specified for the event source '<stream-arn>'").
Cause 3 — expected the mapping projection to read `{'StartingPosition': 'LATEST', 'BatchSize': 1, 'MaximumRetryAttempts': 3, 'MaximumBatchingWindowInSeconds': 0, 'ParallelizationFactor': 1, 'MaximumRecordAgeInSeconds': -1, 'BisectBatchOnFunctionError': False, 'TumblingWindowInSeconds': 0, 'FunctionResponseTypes': [], 'State': 'Enabled'}`. Actual: identical except `'TumblingWindowInSeconds': None`.

## Steps to Reproduce
1. Replay the behavioral suite and re-run the three checks in trace_ids.
2. Cause 1: `CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn='arn:aws:dynamodb:<region>:<account>:table/<Table>', StartingPosition='LATEST')` and compare the message.
3. Cause 2: `CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<table LatestStreamArn>, BatchSize=1)` with no StartingPosition and compare the message.
4. Cause 3: create a stream mapping with `StartingPosition='LATEST', BatchSize=1` and read it with ListEventSourceMappings; inspect `TumblingWindowInSeconds`.
